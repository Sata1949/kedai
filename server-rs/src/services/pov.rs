//! 人称锚定(POV-1):把「视角」从一句口号升级为一层可装配、可预览、可被反思校验的指令。
//!
//! 单一出处:引擎装配(`agents/engine/messages/build.rs`)、步骤指令(`messages/steps.rs`)、
//! 反思链路(`reflector_integration.rs`)与设置页预览(`api/settings/prompt.rs`)都经本模块
//! 取锚定文本,避免四侧各写一份而漂移。
//!
//! 文案纪律:锚定文本是**强指令**(不经 untrusted 包裹),必须保持短句、可执行;每条都针对
//! 一种已知漂移形态(叙述层与台词混用、替用户角色代言、延续上文漂移、小说化旁白)。
use std::fmt::Write;

/// 叙述人称。只认设置页下拉的三档;未知值由 [`parse_pov_mode`] 返回 `None`,调用方走兜底版。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PovMode {
    First,
    Second,
    ThirdLimited,
}

/// 解析设置页的视角字符串。trim 后精确匹配,不做别名/大小写扩展(避免把用户自定义的
/// 半成品文本误判成有效档位)。
pub fn parse_pov_mode(raw: &str) -> Option<PovMode> {
    match raw.trim() {
        "第一人称" => Some(PovMode::First),
        "第二人称" => Some(PovMode::Second),
        "第三人称" => Some(PovMode::ThirdLimited),
        _ => None,
    }
}

/// 生成本轮人称锚定文本。
///
/// - `mode = Some(..)`:`perspective` 项已启用且取值可识别 → 发显式版(强制覆盖提示词/预设/上文)。
/// - `mode = None`:未启用或取值未知 → 发兜底版(让位给会话提示词/预设/用户本轮输入)。
/// - `user_name` 为空:叙述里用「用户角色」指代,不硬塞一个空括号。
pub fn pov_anchor(mode: Option<PovMode>, char_name: &str, user_name: Option<&str>) -> String {
    let char = if char_name.trim().is_empty() {
        "角色"
    } else {
        char_name.trim()
    };
    // 有称呼时写成「用户角色（林）」;无称呼时只写「用户角色」——「你」由下文台词规则承担。
    let user_label = match user_name.map(str::trim).filter(|n| !n.is_empty()) {
        Some(name) => format!("用户角色（{name}）"),
        None => "用户角色".to_string(),
    };
    match mode {
        Some(PovMode::First) => {
            let mut s = String::new();
            s.push_str("【本轮人称锚定：第一人称】\n");
            s.push_str("本轮以此为准：覆盖提示词、预设与上文中不同的人称设定；若用户在本轮输入中明确要求改用人称，以用户要求为准。\n");
            let _ = writeln!(
                s,
                "- 叙述层统一用第一人称「我」指代 {char}，镜头只贴住 {char}，不进入其他角色的内心。"
            );
            let _ = writeln!(
                s,
                "- {user_label}在叙述中称「你」或角色名；{char} 的台词仍用「我」说话。"
            );
            let _ = writeln!(
                s,
                "- 不得中途改用第三人称全知；不得在叙述层用「你」指代 {char}。"
            );
            s.push_str("- 不替用户角色写内心、台词或决定；用户角色的行动由用户输入决定。\n");
            s.push_str("- 若上文人称已漂移，从本轮起按本锚定纠正，不得延续。\n");
            s.push_str("- 本轮以角色回应为主体；小说化描写服务于互动，不写成通篇旁白或章节。");
            s
        }
        Some(PovMode::Second) => {
            let mut s = String::new();
            s.push_str("【本轮人称锚定：第二人称】\n");
            s.push_str("本轮以此为准：覆盖提示词、预设与上文中不同的人称设定；若用户在本轮输入中明确要求改用人称，以用户要求为准。\n");
            let _ = writeln!(
                s,
                "- 叙述层统一用第二人称「你」指代{user_label}，镜头贴住用户角色；{char} 在叙述中称「他/她」或角色名。"
            );
            let _ = writeln!(
                s,
                "- {char} 的台词用第一人称「我」说话；台词不改变叙述人称。"
            );
            let _ = writeln!(
                s,
                "- 不得在叙述层用「我」指代 {char}；不得进入 {char} 或其他人物的内心（除非用户角色可观察）。"
            );
            s.push_str(
                "- 不替用户角色写内心、台词或决定；只写用户角色可观察到的他人反应与环境。\n",
            );
            s.push_str("- 若上文人称已漂移，从本轮起按本锚定纠正，不得延续。\n");
            s.push_str("- 本轮以角色回应为主体；小说化描写服务于互动，不写成通篇旁白或章节。");
            s
        }
        Some(PovMode::ThirdLimited) => {
            let mut s = String::new();
            s.push_str("【本轮人称锚定：第三人称】\n");
            s.push_str("本轮以此为准：覆盖提示词、预设与上文中不同的人称设定；若用户在本轮输入中明确要求改用人称，以用户要求为准。\n");
            let _ = writeln!(
                s,
                "- 叙述层统一用第三人称「他/她」指代 {char}，镜头贴住 {char}，不进入其他角色的内心。"
            );
            let _ = writeln!(
                s,
                "- {user_label}在叙述中称「你」或角色名；{char} 的台词仍用第一人称「我」，用中文双引号包裹。"
            );
            let _ = writeln!(
                s,
                "- 不得在叙述层用「我」指代 {char}；不得用第三人称全知视角进入用户角色的内心。"
            );
            let _ = writeln!(
                s,
                "- 不替用户角色写内心、台词或决定；只写 {char} 与其他角色对用户角色的反应。"
            );
            s.push_str("- 若上文人称已漂移，从本轮起按本锚定纠正，不得延续。\n");
            s.push_str("- 本轮以角色回应为主体；小说化描写服务于互动，不写成通篇旁白或章节。");
            s
        }
        None => {
            let mut s = String::new();
            s.push_str("【本轮人称锚定（默认）】\n");
            s.push_str(
                "若本会话的提示词、预设或用户本轮输入已明确指定叙述人称，以该指定为准；否则按以下默认：\n",
            );
            let _ = writeln!(
                s,
                "- 叙述层用第三人称限知指代 {char}，镜头贴住 {char}，不进入其他角色的内心。"
            );
            let _ = writeln!(
                s,
                "- {user_label}在叙述中称「你」或角色名；{char} 的台词仍用第一人称「我」，用中文双引号包裹。"
            );
            let _ = writeln!(
                s,
                "- 不得在叙述层用「我」指代 {char}；不替用户角色写内心、台词或决定。"
            );
            s.push_str("- 若上文人称已漂移，从本轮起按默认锚定纠正，不得延续。\n");
            s.push_str("- 本轮以角色回应为主体；小说化描写服务于互动，不写成通篇旁白或章节。");
            s
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_only_accepts_three_configured_views() {
        assert_eq!(parse_pov_mode("第一人称"), Some(PovMode::First));
        assert_eq!(parse_pov_mode(" 第二人称 "), Some(PovMode::Second));
        assert_eq!(parse_pov_mode("第三人称"), Some(PovMode::ThirdLimited));
        assert_eq!(parse_pov_mode("第三人称全知"), None);
        assert_eq!(parse_pov_mode(""), None);
    }

    #[test]
    fn first_person_anchor_covers_drift_and_voice_rules() {
        let text = pov_anchor(Some(PovMode::First), "芽衣", Some("林"));
        assert!(text.contains("【本轮人称锚定：第一人称】"), "{text}");
        assert!(text.contains("第一人称「我」指代 芽衣"), "{text}");
        assert!(text.contains("用户角色（林）"), "{text}");
        assert!(text.contains("台词仍用「我」说话"), "{text}");
        assert!(text.contains("不替用户角色写内心"), "{text}");
        assert!(text.contains("从本轮起按本锚定纠正"), "{text}");
        assert!(text.contains("以角色回应为主体"), "{text}");
        assert!(!text.contains("第三人称限知"), "{text}");
    }

    #[test]
    fn second_person_anchor_keeps_dialogue_first_person() {
        let text = pov_anchor(Some(PovMode::Second), "芽衣", Some("林"));
        assert!(text.contains("【本轮人称锚定：第二人称】"), "{text}");
        assert!(text.contains("第二人称「你」指代用户角色（林）"), "{text}");
        assert!(text.contains("台词用第一人称「我」说话"), "{text}");
        assert!(text.contains("台词不改变叙述人称"), "{text}");
    }

    #[test]
    fn third_person_anchor_is_limited_to_character_camera() {
        let text = pov_anchor(Some(PovMode::ThirdLimited), "芽衣", None);
        assert!(text.contains("【本轮人称锚定：第三人称】"), "{text}");
        assert!(text.contains("第三人称「他/她」指代 芽衣"), "{text}");
        assert!(
            text.contains("用户角色在叙述中称「你」或角色名"),
            "无称呼时不得留下空括号: {text}"
        );
        assert!(text.contains("台词仍用第一人称「我」"), "{text}");
        assert!(text.contains("不得在叙述层用「我」指代 芽衣"), "{text}");
    }

    #[test]
    fn fallback_yields_to_explicit_session_instruction() {
        let text = pov_anchor(None, "芽衣", None);
        assert!(text.contains("【本轮人称锚定（默认）】"), "{text}");
        assert!(text.contains("以该指定为准"), "{text}");
        assert!(text.contains("第三人称限知"), "{text}");
        assert!(text.contains("不替用户角色写内心"), "{text}");
    }
}
