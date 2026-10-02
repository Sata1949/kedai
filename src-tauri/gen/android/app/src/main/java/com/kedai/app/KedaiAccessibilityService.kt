package com.kedai.app

import android.accessibilityservice.AccessibilityService
import android.content.Context
import android.graphics.Bitmap
import android.view.Display
import android.view.accessibility.AccessibilityEvent
import android.view.accessibility.AccessibilityManager
import android.accessibilityservice.AccessibilityServiceInfo
import androidx.annotation.Keep
import java.io.File
import java.io.FileOutputStream
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

/**
 * Kedai 无障碍截图服务(移动端视觉能力包 A2,2026-10-02)。
 *
 * 职责:承载 `AccessibilityService.takeScreenshot`(API 30+)——这是 Android
 * 上应用自身**无需 MediaProjection 授权弹窗**即可取屏的官方通道(用户需在
 * 系统设置显式启用本服务)。取到的画面写成 PNG 落盘,由 Rust 侧读回并送入
 * 既有视觉通道(与桌面 Windows 截图工具同一个 `screenshot` 工具名、同一条
 * `{text, images}` 约定式返回,不另建管道)。
 *
 * **前置条件(AOSP `android-14.0.0_r1` 源码实测)**:`takeScreenshot` 只要求
 * meta-data 声明 `canTakeScreenshot=true`(→ `CAPABILITY_CAN_TAKE_SCREENSHOT`);
 * 该版本 API **没有** `FLAG_REQUEST_SCREENSHOT` 标志,无需也无法额外声明 flag。
 *
 * **最小权限(拍板口径,2026-10-02)**:本服务**仅用于截图**——
 * `accessibility_service_config.xml` 里只有 `canTakeScreenshot=true`,不读窗口
 * 内容(`canRetrieveWindowContent=false`)、不做手势、不做输入注入;事件回调为空实现
 * (绑定必须声明事件类型,但我们不消费任何事件)。不做无障碍执行档,不用无障碍替代
 * Shizuku 执行命令。
 *
 * 入参/出参编码(与 Rust 侧 `server-rs/src/services/screen_capture_android.rs` 严格对齐,
 * US 分隔符 \u{1f}):
 *   capture:  path ␟ timeoutMs  →  ""(成功;PNG 已原子写入 path)或抛异常
 *   status:   ""                →  enabled␟描述 | disabled␟原因
 *
 * ⚠️ 混淆约束:本类由清单按类名声明(同 KeepAliveService 理由),release 的 R8
 * 若重命名会导致系统找不到服务。双保险:`@Keep` 注解 +
 * app/proguard-rules.pro 里 `-keep class com.kedai.app.KedaiAccessibilityService { *; }`。
 */
@Keep
class KedaiAccessibilityService : AccessibilityService() {

    override fun onServiceConnected() {
        instance = this
    }

    override fun onDestroy() {
        if (instance === this) instance = null
        super.onDestroy()
    }

    /** 仅截图,不消费事件(系统要求事件类型必须声明,见配置 xml 的占位) */
    override fun onAccessibilityEvent(event: AccessibilityEvent?) {
        // 有意为空:本服务不是无障碍工具,不为任何事件做事。
    }

    override fun onInterrupt() {
        // 有意为空:本服务不做任何持续性的无障碍动作,无需中断处理。
    }

    /**
     * 截取默认显示器整屏,PNG **原子写入** path(临时文件 + 改名,读到半个文件
     * 的可能性为零)。超时 / 系统错误码 / 编码失败一律抛 IllegalStateException,
     * 由 Rust 侧 JNI 转成可操作的中文错误。
     *
     * 阻塞语义:本函数由 Rust 的 spawn_blocking 线程调用;内部用 CountDownLatch
     * 等待系统截图回调(在私有 executor 上执行,避免主线程做位图压缩)。
     */
    fun captureToFile(path: String, timeoutMs: Long) {
        val latch = CountDownLatch(1)
        var bitmap: Bitmap? = null
        var failureCode = NO_CALLBACK

        takeScreenshot(
            Display.DEFAULT_DISPLAY,
            screenshotExecutor,
            object : AccessibilityService.TakeScreenshotCallback {
                override fun onSuccess(result: AccessibilityService.ScreenshotResult) {
                    try {
                        val buffer = result.hardwareBuffer
                        try {
                            // 硬件位图必须拷贝成 ARGB_8888 才能压缩;copy 必须在
                            // buffer 关闭之前完成。
                            bitmap = Bitmap.wrapHardwareBuffer(buffer, result.colorSpace)
                                ?.copy(Bitmap.Config.ARGB_8888, false)
                            if (bitmap == null) failureCode = ENCODE_FAILED
                        } finally {
                            buffer.close()
                        }
                    } catch (t: Throwable) {
                        failureCode = ENCODE_FAILED
                    } finally {
                        latch.countDown()
                    }
                }

                override fun onFailure(errorCode: Int) {
                    failureCode = errorCode
                    latch.countDown()
                }
            },
        )

        if (!latch.await(timeoutMs, TimeUnit.MILLISECONDS)) {
            throw IllegalStateException("截图超时(${timeoutMs}ms):系统未在时限内回调")
        }
        val bmp = bitmap ?: throw IllegalStateException(errorText(failureCode))

        val target = File(path)
        target.parentFile?.mkdirs()
        val tmp = File(target.parentFile, target.name + ".part")
        try {
            FileOutputStream(tmp).use { out ->
                if (!bmp.compress(Bitmap.CompressFormat.PNG, 100, out)) {
                    throw IllegalStateException("PNG 编码失败(系统返回了空位图)")
                }
                out.fd.sync()
            }
            if (target.exists() && !target.delete()) {
                throw IllegalStateException("截图落盘失败:无法覆盖旧文件")
            }
            if (!tmp.renameTo(target)) {
                throw IllegalStateException("截图落盘失败:改名失败")
            }
        } finally {
            // 成功时 tmp 已不存在(delete 返回 false 无害);失败时清残留
            tmp.delete()
            bmp.recycle()
        }
    }

    /** 系统错误码 → 中文可操作文案;NO_CALLBACK/ENCODE_FAILED 是本类的哨兵值 */
    private fun errorText(code: Int): String = when (code) {
        AccessibilityService.ERROR_TAKE_SCREENSHOT_INTERVAL_TIME_SHORT ->
            "截图请求过于频繁(系统限速:约每 333ms 只允许一次),请稍后重试"
        AccessibilityService.ERROR_TAKE_SCREENSHOT_INVALID_DISPLAY ->
            "无效的显示器(系统拒绝了本次截图目标)"
        AccessibilityService.ERROR_TAKE_SCREENSHOT_NO_ACCESSIBILITY_ACCESS ->
            "无障碍服务当前不可用(可能已被系统暂停,请到系统设置重新开启 Kedai 的截图服务)"
        AccessibilityService.ERROR_TAKE_SCREENSHOT_INTERNAL_ERROR ->
            "系统截图内部错误(请重试;持续失败请重新开启无障碍服务)"
        AccessibilityService.ERROR_TAKE_SCREENSHOT_SECURE_WINDOW ->
            "当前画面含受保护内容(FLAG_SECURE),系统禁止截图;请知悉该画面不可截取"
        ENCODE_FAILED -> "截图解码失败(系统返回的位图无法读取)"
        else -> "截图失败(系统错误码 $code)"
    }

    companion object {
        /** 本服务实例(onServiceConnected 置,onDestroy 清);null ≠ 未启用,服务可能尚未被系统连接 */
        @Volatile
        private var instance: KedaiAccessibilityService? = null

        /** 私有单线程 executor:截图回调(位图拷贝/裁剪)不在主线程做 */
        private val screenshotExecutor = Executors.newSingleThreadExecutor()

        /** 未收到任何回调的哨兵错误码(不与系统错误码冲突) */
        private const val NO_CALLBACK = -1
        private const val ENCODE_FAILED = -2

        fun instanceOrNull(): KedaiAccessibilityService? = instance

        /**
         * 本服务是否已被系统启用(**列表法**:查已启用无障碍服务清单,不依赖实例)。
         * instance 为空只说明服务尚未连接(或刚被断开),不等于未启用——UI 状态与
         * 可用性探测用本函数,Rust 侧取其结果做闸门。
         */
        fun isEnabled(context: Context): Boolean {
            val manager = context.getSystemService(Context.ACCESSIBILITY_SERVICE)
                as? AccessibilityManager ?: return false
            if (!manager.isEnabled) return false
            val self = KedaiAccessibilityService::class.java.name
            val pkg = context.packageName
            return manager
                .getEnabledAccessibilityServiceList(AccessibilityServiceInfo.FEEDBACK_ALL_MASK)
                .any { info ->
                    val si = info.resolveInfo?.serviceInfo
                    si != null && si.packageName == pkg && si.name == self
                }
        }
    }
}
