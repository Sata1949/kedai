// Android 执行桥的线格式(阶段 C/D;T3 收口)。
//
// **为什么单独成模块**:编码/解码是纯字符串变换,与 Android 目标无关——放这里
// 才能在桌面 `cargo test` 里有对拍用例(Kotlin 侧同名规则见
// `src-tauri/gen/android/app/src/main/java/com/kedai/app/ShellExecutorBridge.kt`)。
//
// **为什么换掉 US 分隔符**(原 `\u{1f}`):命令原文含该字符会让字段错位。US 是控制
// 字符,`echo $'\x1f'` 之类完全可以产出来——属可用性缺陷(不能提权,但解析会乱)。
//
// 现行格式(v2,长度无关、二义性消除):
//   exec 请求: `v2 ␟ b64(command) ␟ b64(cwd) ␟ timeoutMs`
//   exec 回执: `v2 ␟ exitCode ␟ b64(stdout) ␟ b64(stderr)`
//   b64 = 标准字母表、无换行(Rust `base64 STANDARD`,Kotlin `Base64.NO_WRAP`)。
//   base64 字母表不含 US,故分隔符永不与内容冲突;`cwd` 为空串表示"用进程默认 cwd"。
//
// **版本前缀是错误可见性的关键**:两侧必须同版本。旧 Kotlin 收到 v2 会因首个字段不是
// 整数而明确报错,新 Rust 收到旧格式会因缺 v2 前缀而明确报错——都不静默错位。

/// 线格式版本前缀。两侧必须同时升级;不匹配即报错(不做隐式兼容)。
pub const WIRE_VERSION: &str = "v2";

/// 字段分隔符。base64 字母表不含它,故内容永远不会被误切。
pub const SEP: char = '\u{1f}';

/// 编码 exec 请求。`cwd` 为空串语义 = 使用进程默认工作目录。
pub fn encode_exec_request(command: &str, cwd: Option<&str>, timeout_ms: u64) -> String {
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD;
    format!(
        "{WIRE_VERSION}{SEP}{}{SEP}{}{SEP}{timeout_ms}",
        b64.encode(command),
        b64.encode(cwd.unwrap_or("")),
    )
}

/// 解码 exec 请求的字段(T3 新增,供两侧对拍与失败诊断用)。
///
/// 返回 `(command, cwd, timeout_ms)`;`cwd` 为 `Some` 仅当原值非空。
pub fn decode_exec_request(payload: &str) -> Result<(String, Option<String>, u64), String> {
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD;
    let mut parts = payload.split(SEP);
    let version = parts.next().unwrap_or("");
    if version != WIRE_VERSION {
        return Err(format!(
            "执行器入参协议版本不匹配(期望 {WIRE_VERSION},收到 {version:?});请确认 APK 与后端同版本"
        ));
    }
    let raw_command = parts
        .next()
        .ok_or_else(|| "执行器入参缺 command 字段".to_string())?;
    let raw_cwd = parts
        .next()
        .ok_or_else(|| "执行器入参缺 cwd 字段".to_string())?;
    let raw_timeout = parts
        .next()
        .ok_or_else(|| "执行器入参缺 timeoutMs 字段".to_string())?;
    let command = b64
        .decode(raw_command)
        .map_err(|e| format!("执行器入参 command 不是合法 base64: {e}"))?;
    let cwd = b64
        .decode(raw_cwd)
        .map_err(|e| format!("执行器入参 cwd 不是合法 base64: {e}"))?;
    let timeout_ms: u64 = raw_timeout
        .trim()
        .parse()
        .map_err(|_| format!("执行器入参 timeoutMs 非法: {raw_timeout}"))?;
    Ok((
        String::from_utf8(command).map_err(|e| format!("执行器入参 command 非 UTF-8: {e}"))?,
        {
            let text =
                String::from_utf8(cwd).map_err(|e| format!("执行器入参 cwd 非 UTF-8: {e}"))?;
            if text.is_empty() {
                None
            } else {
                Some(text)
            }
        },
        timeout_ms,
    ))
}

/// 解析 exec 回执(Android 侧 Kotlin 回传)。`tier` 由调用方补。
pub fn decode_exec_result(raw: &str) -> Result<(i32, String, String), String> {
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD;
    let mut parts = raw.split(SEP);
    let version = parts.next().unwrap_or("");
    if version != WIRE_VERSION {
        return Err(format!(
            "执行器回执协议版本不匹配(期望 {WIRE_VERSION},收到 {version:?});请确认 APK 与后端同版本"
        ));
    }
    let code = parts
        .next()
        .ok_or_else(|| format!("执行器返回格式异常(缺 exitCode): {raw}"))?;
    let raw_stdout = parts
        .next()
        .ok_or_else(|| format!("执行器返回格式异常(缺 stdout): {raw}"))?;
    let raw_stderr = parts
        .next()
        .ok_or_else(|| format!("执行器返回格式异常(缺 stderr): {raw}"))?;
    // 有意丢弃 ParseIntError:唯一信息是「不是整数」,原始 code 已在消息中
    let exit_code: i32 = code
        .trim()
        .parse()
        .map_err(|_| format!("执行器返回的 exitCode 非法: {code}"))?;
    let stdout = b64
        .decode(raw_stdout)
        .map_err(|e| format!("执行器回执 stdout 不是合法 base64: {e}"))?;
    let stderr = b64
        .decode(raw_stderr)
        .map_err(|e| format!("执行器回执 stderr 不是合法 base64: {e}"))?;
    Ok((
        exit_code,
        String::from_utf8(stdout).map_err(|e| format!("执行器回执 stdout 非 UTF-8: {e}"))?,
        String::from_utf8(stderr).map_err(|e| format!("执行器回执 stderr 非 UTF-8: {e}"))?,
    ))
}

/// 编码 exec 回执(Kotlin 侧同一算法的 Rust 参照实现;供对拍用例与文档用)。
pub fn encode_exec_result(exit_code: i32, stdout: &str, stderr: &str) -> String {
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD;
    format!(
        "{WIRE_VERSION}{SEP}{exit_code}{SEP}{}{SEP}{}",
        b64.encode(stdout),
        b64.encode(stderr)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T3 的判别性用例:命令原文含旧分隔符 US 时,字段不得错位。
    /// 修复前(US 分隔)该输入会被切成 5 段,command 只剩前半截。
    #[test]
    fn exec_request_roundtrips_with_control_characters() {
        let command = "printf 'a\u{1f}b\u{1f}c'";
        let payload = encode_exec_request(command, Some("/data/local/tmp"), 60_000);
        let (got_command, got_cwd, got_timeout) = decode_exec_request(&payload).expect("应能解码");
        assert_eq!(got_command, command, "US 字符不得破坏 command");
        assert_eq!(got_cwd.as_deref(), Some("/data/local/tmp"));
        assert_eq!(got_timeout, 60_000);
        // 分隔符计数不受内容影响:恒为 3 个(v2 + 三字段)
        assert_eq!(payload.matches(SEP).count(), 3);
    }

    /// 回执侧同理:stdout/stderr 含 US 时不得错位。
    #[test]
    fn exec_result_roundtrips_with_control_characters() {
        let stdout = "line1\u{1f}line2\n第二行";
        let stderr = "err\u{1f}detail";
        let raw = encode_exec_result(1, stdout, stderr);
        let (code, got_stdout, got_stderr) = decode_exec_result(&raw).expect("应能解码");
        assert_eq!(code, 1);
        assert_eq!(got_stdout, stdout);
        assert_eq!(got_stderr, stderr);
        assert_eq!(raw.matches(SEP).count(), 3);
    }

    #[test]
    fn empty_cwd_roundtrips_as_none() {
        let payload = encode_exec_request("ls", None, 1_000);
        let (_, cwd, _) = decode_exec_request(&payload).expect("应能解码");
        assert_eq!(cwd, None, "空 cwd 语义 = 使用进程默认目录");
        let payload = encode_exec_request("ls", Some(""), 1_000);
        let (_, cwd, _) = decode_exec_request(&payload).expect("应能解码");
        assert_eq!(cwd, None);
    }

    /// 两侧版本不一致时必须报错,不得静默按旧格式解读。
    #[test]
    fn version_mismatch_is_rejected_loudly() {
        let legacy = format!("echo hi{SEP}{SEP}60000");
        let err = decode_exec_request(&legacy).expect_err("旧格式应被拒绝");
        assert!(err.contains("协议版本不匹配"), "{err}");
        let legacy_result = format!("0{SEP}out{SEP}err");
        let err = decode_exec_result(&legacy_result).expect_err("旧回执应被拒绝");
        assert!(err.contains("协议版本不匹配"), "{err}");
    }

    #[test]
    fn non_base64_field_reports_which_field() {
        let payload = format!("{WIRE_VERSION}{SEP}!!!notbase64{SEP}{SEP}1000");
        let err = decode_exec_request(&payload).expect_err("非法 base64 应报错");
        assert!(err.contains("command"), "错误应点名字段: {err}");
    }

    #[test]
    fn malformed_timeout_is_rejected() {
        let payload = format!("{WIRE_VERSION}{SEP}YQ=={SEP}{SEP}abc");
        let err = decode_exec_request(&payload).expect_err("非法 timeout 应报错");
        assert!(err.contains("timeoutMs"), "{err}");
    }
}
