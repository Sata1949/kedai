package com.kedai.app

import android.content.ContentValues
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Environment
import android.provider.MediaStore
import androidx.annotation.Keep
import androidx.core.content.FileProvider
import java.io.File

/**
 * Kedai 原生能力桥接(供 Rust 侧经 JNI 调用)。
 *
 * 与 [KeystoreBridge] 同一模式:Rust 只做字符串进出,Kotlin 侧封装 Android API。
 * 四项能力:
 *   - openExternal(url):用系统浏览器打开外链。WebView 里 `target=_blank` 在
 *     Android 上没有新窗口处理,会被当作当前页导航,把 SPA 界面顶掉且返回键无处可退;
 *     改为交给系统浏览器,行为与桌面一致。
 *   - shareFile(payload):把导出内容写入缓存目录后经 FileProvider 弹系统分享选单。
 *     Android 的文件保存是 SAF(content:// URI)语义,插件 fs 的路径写入不适用;
 *     「写缓存 + 分享」是移动端导出 JSON 的标准做法。
 *   - startKeepAlive() / stopKeepAlive():启停前台服务,避免长任务在切后台/锁屏后
 *     被系统冻结或回收。
 *
 * 入参约定(与 server-rs/src/services/native_bridge_android.rs 对齐):
 *   - openExternal:入参即 URL;
 *   - shareFile:入参为 "文件名\u{1f}内容"(US 单元分隔符);
 *   - saveToDownloads:入参编码同 shareFile,返回实际落盘位置的描述;
 *   - keepalive*:入参为空串。
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

    /** 由 MainActivity 在启动时注入应用上下文(未注入时抛异常,由 Rust 侧转成可见错误) */
    fun attach(context: Context) {
        appContext = context.applicationContext
    }

    private fun requireContext(): Context =
        appContext ?: throw IllegalStateException("KedaiNative 未初始化:MainActivity 未注入 Context")

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
