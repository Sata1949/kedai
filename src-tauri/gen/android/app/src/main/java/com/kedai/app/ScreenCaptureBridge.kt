package com.kedai.app

import android.content.Intent
import android.provider.Settings
import androidx.annotation.Keep

/**
 * 无障碍截图桥(移动端视觉能力包 A2,2026-10-02):供 Rust 侧经 JNI 调用。
 *
 * 与 [KedaiNative] / [ShellExecutorBridge] 同一模式:Rust 只做字符串进出
 * (server-rs/src/services/screen_capture_android.rs),Kotlin 侧封装
 * Android API。三方法:
 *   - capture(payload):`路径␟超时ms` → 截屏 PNG 原子写入路径,返回 "";失败抛异常;
 *   - status():`enabled␟描述` / `disabled␟原因`——本服务是否已被系统启用
 *     (列表法判定,见 [KedaiAccessibilityService.isEnabled]);
 *   - openSettings():跳系统「无障碍」设置页(设置面板「前往系统设置开启」按钮)。
 *
 * 参数个数两侧一致性由 `tools/check-arch.mjs` 规则 L 把关:status / openSettings
 * 无参(Rust 侧走 `call_string_static_no_arg`),capture 一个 String 入参。
 *
 * ⚠️ 混淆约束:本类被 native 按名调用(FindClass / CallStaticMethod)。release
 * 的 R8 若重命名会导致运行时失败(KeystoreBridge 已有实测踩坑)。双保险:`@Keep`
 * 注解 + app/proguard-rules.pro 里 `-keep class com.kedai.app.ScreenCaptureBridge { *; }`。
 */
@Keep
object ScreenCaptureBridge {
    private const val SEP = '\u001f'

    /** 截图超时缺省/上限(毫秒;Rust 侧传入,非法值回落到安全默认) */
    private const val DEFAULT_TIMEOUT_MS = 8000L
    private const val MAX_TIMEOUT_MS = 30000L

    /**
     * 截取整屏到指定路径。入参 `路径␟超时ms`(US 分隔符,与 Rust 侧编码一致)。
     * 返回 "" 表成功(PNG 已原子落盘);异常(含服务未启用/未连接)由 Rust 侧转成错误。
     */
    @JvmStatic
    @Keep
    fun capture(payload: String): String {
        val idx = payload.indexOf(SEP)
        val path = (if (idx >= 0) payload.substring(0, idx) else payload).trim()
        if (path.isEmpty()) throw IllegalArgumentException("截图输出路径为空")
        val timeoutMs = if (idx >= 0) {
            payload.substring(idx + 1).trim().toLongOrNull() ?: DEFAULT_TIMEOUT_MS
        } else {
            DEFAULT_TIMEOUT_MS
        }
        val timeout = timeoutMs.coerceIn(1000L, MAX_TIMEOUT_MS)

        val service = KedaiAccessibilityService.instanceOrNull()
            ?: throw IllegalStateException(
                "无障碍截图服务未连接:请在 系统设置 → 无障碍 中开启 Kedai 的截图服务(或从应用 设置 → 视觉与截图 跳转)",
            )
        service.captureToFile(path, timeout)
        return ""
    }

    /**
     * 服务启用状态探测:enabled␟描述 | disabled␟原因。
     * 上下文缺失不抛异常——探测路径抛了会让整段状态查询失败(与 Shizuku 探测同口径)。
     */
    @JvmStatic
    @Keep
    fun status(): String {
        val context = KedaiNative.appContextOrNull()
            ?: return "disabled${SEP}应用上下文未初始化(MainActivity 尚未注入)"
        return if (KedaiAccessibilityService.isEnabled(context)) {
            "enabled${SEP}无障碍截图服务已启用"
        } else {
            "disabled${SEP}无障碍截图服务未启用(系统设置 → 无障碍)"
        }
    }

    /** 跳转系统「无障碍」设置页(由设置面板按钮触发) */
    @JvmStatic
    @Keep
    fun openSettings(): String {
        val context = KedaiNative.appContextOrNull()
            ?: throw IllegalStateException("应用上下文未初始化(MainActivity 尚未注入)")
        val intent = Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS).apply {
            addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        }
        context.startActivity(intent)
        return ""
    }
}
