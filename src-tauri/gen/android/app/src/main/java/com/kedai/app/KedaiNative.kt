package com.kedai.app

import android.app.Activity
import android.app.AlarmManager
import android.app.PendingIntent
import android.content.ActivityNotFoundException
import android.content.ContentValues
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Environment
import android.os.Handler
import android.os.Looper
import android.os.Process
import android.provider.MediaStore
import android.provider.Settings
import android.widget.Toast
import androidx.annotation.Keep
import androidx.core.content.FileProvider
import com.google.android.material.dialog.MaterialAlertDialogBuilder
import java.io.File
import java.lang.ref.WeakReference

/**
 * Kedai 原生能力桥接(供 Rust 侧经 JNI 调用)。
 *
 * 与 [KeystoreBridge] 同一模式:Rust 只做字符串进出,Kotlin 侧封装 Android API。
 * 能力:
 *   - openExternal(url):用系统浏览器打开外链。WebView 里 `target=_blank` 在
 *     Android 上没有新窗口处理,会被当作当前页导航,把 SPA 界面顶掉且返回键无处可退;
 *     改为交给系统浏览器,行为与桌面一致。
 *   - shareFile(payload):把导出内容写入缓存目录后经 FileProvider 弹系统分享选单。
 *     Android 的文件保存是 SAF(content:// URI)语义,插件 fs 的路径写入不适用;
 *     「写缓存 + 分享」是移动端导出 JSON 的标准做法。
 *   - startKeepAlive() / stopKeepAlive():启停前台服务,避免长任务在切后台/锁屏后
 *     被系统冻结或回收。
 *   - requestAllFilesAccess():打开「所有文件访问」授权页(默认数据根在公共
 *     Download/Kedai-Data;Android 11+ 按路径读写公共目录需要该特殊权限)。
 *   - showStartupError(message):启动失败时展示原生对话框(Android 不能走
 *     std::process::exit,退出时 WebView 静态析构会触发 FORTIFY abort)。
 *   - restartApp():数据位置切换后的原生重启(安排 launcher Intent 后由 Rust `_exit`)。
 *
 * 入参约定(与 server-rs/src/services/native_bridge_android.rs 对齐):
 *   - openExternal:入参即 URL;
 *   - shareFile:入参为 "文件名\u{1f}内容"(US 单元分隔符);
 *   - saveToDownloads:入参编码同 shareFile,返回实际落盘位置的描述;
 *   - showStartupError:入参为错误正文;
 *   - keepalive_* 与 restartApp:入参为空串。
 * 全部返回 "" 表成功(除 saveToDownloads 返回位置描述);异常抛出由 Rust 侧转为错误。
 *
 * ⚠️ 混淆约束:本类被 native 按名调用(FindClass / CallStaticMethod),
 * release 的 R8 若重命名会导致运行时失败。双保险:`@Keep` 注解 +
 * app/proguard-rules.pro 里 `-keep class com.kedai.app.KedaiNative { *; }`。
 */
@Keep
object KedaiNative {
    private const val SEP = '\u001f'

    /** 应用上下文(在 MainActivity.onCreate 中注入) */
    @Volatile
    private var appContext: Context? = null

    /**
     * 当前前台 Activity 的弱引用:展示启动失败对话框必须用 Activity 作 Context,
     * applicationContext 会抛 BadTokenException。弱引用避免静态单例泄漏 Activity。
     */
    @Volatile
    private var activityRef: WeakReference<Activity>? = null

    /**
     * Rust 启动失败可能早于 MainActivity.attach(上下文尚未注入):先存下错误,
     * 等 attach 后补弹,避免「后端起不来 + 无任何提示」的空白窗口。
     */
    @Volatile
    private var pendingStartupError: String? = null

    /** 由 MainActivity 在启动时注入应用上下文(未注入时抛异常,由 Rust 侧转成可见错误) */
    fun attach(context: Context) {
        appContext = context.applicationContext
        if (context is Activity) {
            activityRef = WeakReference(context)
        }
        pendingStartupError?.let { message ->
            pendingStartupError = null
            showStartupError(message)
        }
    }

    private fun requireContext(): Context =
        appContext ?: throw IllegalStateException("KedaiNative 未初始化:MainActivity 未注入 Context")

    /** 可用于弹窗的 Activity;已 finish/destroy 时返回 null。 */
    private fun activityOrNull(): Activity? {
        val activity = activityRef?.get() ?: return null
        return if (activity.isFinishing || activity.isDestroyed) null else activity
    }

    /**
     * 取应用上下文(未注入时返回 null)。供 [ShellExecutorBridge] 等兄弟桥类做
     * 「可用性探测」——探测路径不应因上下文缺失而抛异常(抛了会让整段探测失败)。
     */
    @JvmStatic
    @Keep
    fun appContextOrNull(): Context? = appContext

    /** 用系统浏览器打开外链 */
    @JvmStatic
    @Keep
    fun openExternal(url: String): String {
        val trimmed = url.trim()
        if (trimmed.isEmpty()) throw IllegalArgumentException("外链为空")
        val intent = Intent(Intent.ACTION_VIEW, Uri.parse(trimmed)).apply {
            addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        }
        requireContext().startActivity(intent)
        return ""
    }

    /** 分享文本为文件:写缓存目录 → FileProvider URI → 系统分享选单 */
    @JvmStatic
    @Keep
    fun shareFile(payload: String): String {
        val idx = payload.indexOf(SEP)
        val name = if (idx >= 0) payload.substring(0, idx) else payload
        val content = if (idx >= 0) payload.substring(idx + 1) else ""
        if (name.isBlank()) throw IllegalArgumentException("导出文件名为空")

        val context = requireContext()
        val dir = File(context.cacheDir, "exports").apply { mkdirs() }
        // 文件名去掉路径分隔符,避免目录穿越
        val safeName = name.replace('/', '_').replace('\\', '_')
        val file = File(dir, safeName)
        file.writeText(content, Charsets.UTF_8)

        val uri: Uri = FileProvider.getUriForFile(
            context,
            context.packageName + ".fileprovider",
            file,
        )
        val intent = Intent(Intent.ACTION_SEND).apply {
            type = "application/json"
            putExtra(Intent.EXTRA_STREAM, uri)
            putExtra(Intent.EXTRA_SUBJECT, safeName)
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        }
        val chooser = Intent.createChooser(intent, "导出到").apply {
            addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        }
        context.startActivity(chooser)
        return ""
    }

    /**
     * 打开系统的「所有文件访问」授权页(Android 11+ 公共目录读写)。
     *
     * 数据根默认在 `/storage/emulated/0/Download/Kedai-Data`;Android 11 起按路径
     * 直接访问公共目录必须持有 MANAGE_EXTERNAL_STORAGE。未授权时设置页/首次引导
     * 展示授权入口,授权后返回应用重新检测(本方法只负责拉起系统页面,不做结果判定)。
     */
    @JvmStatic
    @Keep
    fun requestAllFilesAccess(): String {
        val context = requireContext()
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) return ""
        val appIntent = Intent(
            Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION,
            Uri.parse("package:" + context.packageName),
        ).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        try {
            context.startActivity(appIntent)
        } catch (_: ActivityNotFoundException) {
            // 部分厂商 ROM 没有应用级页面,回退到全局「所有文件访问」列表
            val globalIntent = Intent(Settings.ACTION_MANAGE_ALL_FILES_ACCESS_PERMISSION)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            context.startActivity(globalIntent)
        }
        return ""
    }

    /** 启动前台服务(长任务保活);重复调用幂等 */
    @JvmStatic
    @Keep
    fun startKeepAlive(): String {
        KeepAliveService.start(requireContext())
        return ""
    }

    /** 停止前台服务保活;重复调用幂等 */
    @JvmStatic
    @Keep
    fun stopKeepAlive(): String {
        KeepAliveService.stop(requireContext())
        return ""
    }

    /**
     * 启动失败时展示原生对话框。
     *
     * Rust 侧在 Android 上不能调用 AppHandle::exit:它会走 std::process::exit,
     * 退出时 hwui/WebView 的静态析构会锁一个已销毁的互斥量(Android 14/15 FORTIFY abort)。
     * 这里只负责展示;用户点「退出」时用 Process.killProcess(SIGKILL,不跑静态析构)。
     */
    @JvmStatic
    @Keep
    fun showStartupError(message: String): String {
        val activity = activityOrNull()
        if (activity == null) {
            // 极端情况(Activity 已销毁):至少用 Toast 给出原因;不抛异常,避免
            // Rust 侧在错误路径上再触发一次退出崩溃。
            val context = appContext
            if (context != null) {
                Handler(Looper.getMainLooper()).post {
                    Toast.makeText(context, "Kedai 启动失败:$message", Toast.LENGTH_LONG).show()
                }
            }
            return ""
        }
        Handler(Looper.getMainLooper()).post {
            try {
                MaterialAlertDialogBuilder(activity)
                    .setTitle("Kedai 启动失败")
                    .setMessage(message)
                    .setCancelable(false)
                    .setPositiveButton("退出") { _, _ ->
                        // 不能用 System.exit:它同样跑 atexit/静态析构,会触发同一 FORTIFY abort。
                        Process.killProcess(Process.myPid())
                    }
                    .show()
            } catch (_: Throwable) {
                Toast.makeText(activity, "Kedai 启动失败:$message", Toast.LENGTH_LONG).show()
            }
        }
        return ""
    }

    /**
     * 原生重启:安排 launcher Intent,600ms 后由系统拉起新进程。
     *
     * 调用方(Rust)紧接着会 `_exit(0)`;不能走 Tauri 的 process::restart,原因同
     * [showStartupError]。用 AlarmManager 而不是直接 startActivity,是为了让旧进程
     * 先干净退出,避免同一进程内 Activity 复用导致数据根仍是旧值。
     */
    @JvmStatic
    @Keep
    fun restartApp(): String {
        val context = requireContext()
        val launch = context.packageManager.getLaunchIntentForPackage(context.packageName)
            ?: throw IllegalStateException("找不到 Kedai 的启动入口")
        launch.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP)
        val pending = PendingIntent.getActivity(
            context,
            0,
            launch,
            PendingIntent.FLAG_CANCEL_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val alarm = context.getSystemService(Context.ALARM_SERVICE) as AlarmManager
        alarm.set(AlarmManager.RTC, System.currentTimeMillis() + 600L, pending)
        return ""
    }

    /**
     * 把最终产物写入设备的「下载」目录,返回**实际落盘位置的可读描述**。
     *
     * 入参编码与 [shareFile] 相同:`文件名␟内容`(US 分隔符)。
     *
     * 为什么需要它(2026-09-17):任务产物此前只能留在应用内的 result 文本里
     * (或经分享选单让用户手动挑落点)。在非 root / 非 Shizuku 的沙箱档下,
     * `bash` 与文件写工具都够不到 `Download`,模型没有「把成果交付成文件」的通道。
     *
     * 分层回退(按 API 级别,免存储权限):
     *   - API ≥ 29:`MediaStore.Downloads` 插入 —— 免权限写入系统「下载」目录,
     *     用户在文件管理器/下载应用里直接可见;
     *   - API < 29(历史 minSdk 24 时代的回退;2026-10-02 起 minSdk 30,本分支已不可达,
     *     保留以维持 API 语义完整):MediaStore 尚无 Downloads 集合,且写公共目录
     *     需要 `WRITE_EXTERNAL_STORAGE`(本项目**刻意不申请**任何存储权限),
     *     故回退到应用自己的外部目录 `Android/data/<pkg>/files/Download/`,
     *     并把**真实路径**回传,由 UI/模型如实告知用户去哪找。
     *
     * 返回值即上述位置描述(MediaStore 走 content URI,回退走绝对路径),
     * 供任务结果里展示——不撒谎说「已存到下载目录」,低版本用户照路径能找到。
     *
     * ⚠️ 不加存储权限是**有意取舍**:<29 的用户拿不到系统「下载」目录的写入权,
     * 与其申请一个被 Play 视为敏感的权限,不如退回应用私有目录并如实告知路径。
     */
    @JvmStatic
    @Keep
    fun saveToDownloads(payload: String): String {
        val idx = payload.indexOf(SEP)
        val rawName = if (idx >= 0) payload.substring(0, idx) else payload
        val content = if (idx >= 0) payload.substring(idx + 1) else ""
        if (rawName.isBlank()) throw IllegalArgumentException("文件名不能为空")

        val context = requireContext()
        // 文件名去掉路径分隔符,避免目录穿越(与 shareFile 同口径)
        val name = rawName.replace('/', '_').replace('\\', '_').trim()
        if (name.isEmpty()) throw IllegalArgumentException("文件名不能为空")
        val mime = guessMimeType(name)

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            val values = ContentValues().apply {
                put(MediaStore.MediaColumns.DISPLAY_NAME, name)
                put(MediaStore.MediaColumns.MIME_TYPE, mime)
                put(MediaStore.MediaColumns.RELATIVE_PATH, Environment.DIRECTORY_DOWNLOADS)
                put(MediaStore.MediaColumns.IS_PENDING, 1)
            }
            val resolver = context.contentResolver
            val uri = resolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, values)
                ?: throw IllegalStateException("系统下载目录不可写(MediaStore 插入失败)")
            try {
                resolver.openOutputStream(uri)?.use { out ->
                    out.write(content.toByteArray(Charsets.UTF_8))
                } ?: throw IllegalStateException("无法打开下载目录写入流")
                // 发布条目:清 IS_PENDING 之后文件才对其它应用可见
                values.clear()
                values.put(MediaStore.MediaColumns.IS_PENDING, 0)
                resolver.update(uri, values, null, null)
            } catch (e: Exception) {
                // 写入失败要把半截条目删掉,否则下载目录里留一个 0 字节垃圾
                runCatching { resolver.delete(uri, null, null) }
                throw e
            }
            return "下载/$name"
        }

        // API < 29 回退:应用外部私有目录(免权限),路径如实回报
        val dir = context.getExternalFilesDir(Environment.DIRECTORY_DOWNLOADS)
            ?: context.filesDir
        dir.mkdirs()
        val file = File(dir, name)
        file.writeText(content, Charsets.UTF_8)
        return file.absolutePath
    }

    /** 按扩展名猜 MIME;未知一律 text/plain(产物多为文本),不猜二进制类型 */
    private fun guessMimeType(name: String): String = when (name.substringAfterLast('.', "").lowercase()) {
        "txt", "md", "log", "csv" -> "text/plain"
        "json" -> "application/json"
        "html", "htm" -> "text/html"
        "xml" -> "text/xml"
        "png" -> "image/png"
        "jpg", "jpeg" -> "image/jpeg"
        "pdf" -> "application/pdf"
        else -> "text/plain"
    }
}
