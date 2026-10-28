// 自 build.rs 的 `#[cfg(test)] mod tests` 迁入（CARD-1 文件大小治理，纯代码搬移）。
// 文件名以 `_tests.rs` 结尾，不参与超千行源码 ratchet。
use super::build::{build_llm_messages, build_llm_messages_with_position, LiteraryTexts};
use crate::parsing::assistant::AssistantVars;
use crate::services::prompt_inject_service::{FloorRole, InjectMode, PromptInjectConfig};
use std::collections::HashMap;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::prompt_inject_service::{FloorPosition, PromptFloor};

    /// build_llm_messages:world_text 注入到 system 内
    #[test]
    fn build_messages_injects_world_text() {
        let mut vars = HashMap::new();
        let (msgs, _) = build_llm_messages(
            "芽衣",
            "兔族少女。",
            "",
            "",
            Some("世界书设定:\n[地点]\n图书馆。"),
            &[],
            None,
            None,
            &mut vars,
            &mut AssistantVars::new(),
        );
        assert_eq!(msgs.len(), 1);
        assert!(msgs[0].role == "system");
        assert!(msgs[0].content.contains("世界书设定:"));
        assert!(msgs[0].content.contains("图书馆。"));
        assert!(msgs[0].content.contains("兔族少女。"));
    }
    /// build_llm_messages:自定义提示词模板替换占位符,世界书经 {{world_info}} 注入
    #[test]
    fn custom_prompt_placeholders_replaced() {
        let tpl = "你是{{character_name}},{{character_description}},以下是世界书:\n{{world_info}}\n自定义要求。";
        let mut vars = HashMap::new();
        let (msgs, _) = build_llm_messages(
            "芽衣",
            "兔族少女。",
            "",
            "",
            Some("世界书设定:\n[地点]\n图书馆。"),
            &[],
            Some(tpl),
            None,
            &mut vars,
            &mut AssistantVars::new(),
        );
        assert_eq!(msgs.len(), 1);
        assert!(msgs[0].content.contains("你是芽衣"));
        assert!(msgs[0].content.contains("兔族少女。"));
        assert!(msgs[0].content.contains("图书馆。"));
        assert!(msgs[0].content.contains("自定义要求。"));
        // 自定义提示词不再拼接内置默认要求
        assert!(!msgs[0].content.contains("始终以角色身份回复"));
    }
    /// build_llm_messages:自定义系统提示词同样走宏系统(宏在提示词中持续工作)
    #[test]
    fn custom_prompt_expands_macros() {
        // {{char}}/{{random}}/{{setvar}}/{{getvar}} 应全部展开;时序:setvar 在 getvar 前生效
        let tpl =
            "你是{{char}},随机值{{random:5,5}},{{setvar::地点::酒馆}}当前地点={{getvar::地点}}";
        let mut vars = HashMap::new();
        let (msgs, _) = build_llm_messages(
            "芽衣",
            "兔族少女。",
            "",
            "",
            None,
            &[],
            Some(tpl),
            None,
            &mut vars,
            &mut AssistantVars::new(),
        );
        let sys = &msgs[0].content;
        assert!(sys.contains("你是芽衣"), "{{char}} 应展开:{sys}");
        assert!(sys.contains("随机值5"), "{{random:5,5}} 应展开为 5:{sys}");
        assert!(
            sys.contains("当前地点=酒馆"),
            "setvar/getvar 时序错误:{sys}"
        );
        // 与楼层共享 vars:setvar 副作用写入 vars,供持久化
        assert_eq!(vars.get("地点").map(|s| s.as_str()), Some("酒馆"));
        // {{world_info}} 依旧由调用方替换后进入
        let tpl2 = "世界书:\n{{world_info}}";
        let (msgs2, _) = build_llm_messages(
            "芽衣",
            "",
            "",
            "",
            Some("世界书设定:图书馆。"),
            &[],
            Some(tpl2),
            None,
            &mut vars,
            &mut AssistantVars::new(),
        );
        assert!(msgs2[0].content.contains("图书馆。"));
    }
    /// build_llm_messages:空自定义提示词回退内置默认
    #[test]
    fn empty_custom_prompt_falls_back() {
        let mut vars = HashMap::new();
        let (msgs, _) = build_llm_messages(
            "芽衣",
            "兔族少女。",
            "",
            "",
            None,
            &[],
            Some("  "),
            None,
            &mut vars,
            &mut AssistantVars::new(),
        );
        assert!(
            msgs[0].content.contains("文学创作系统"),
            "内置默认应含创作总纲: {}",
            msgs[0].content
        );
    }
    /// 文学能力包(LIT-2):引擎兜底模板(用户清空提示词框路径)在装配层传入增强段时
    /// 追加该段;不传(None)时与加参数前逐字节一致;自定义提示词路径不受本参数影响
    /// (那是 LIT-3 两段注入的职责)。
    #[test]
    fn fallback_template_appends_literary_enhancement() {
        let mut vars = HashMap::new();
        let (msgs, _) = build_llm_messages_with_position(
            "芽衣",
            "兔族少女。",
            "",
            "",
            &[],
            &[],
            &[],
            &[],
            None,
            None,
            None,
            "user",
            None,
            "user",
            &mut vars,
            &mut AssistantVars::new(),
            None,
            LiteraryTexts::default(),
        );
        assert!(msgs[0].content.contains("文学创作系统"), "兜底模板应生效");
        assert!(
            !msgs[0].content.contains("【文学增强纪律】"),
            "未传增强段时不得出现本段(开关关逐字等同)"
        );

        let mut vars2 = HashMap::new();
        let (msgs2, _) = build_llm_messages_with_position(
            "芽衣",
            "兔族少女。",
            "",
            "",
            &[],
            &[],
            &[],
            &[],
            None,
            None,
            None,
            "user",
            None,
            "user",
            &mut vars2,
            &mut AssistantVars::new(),
            None,
            LiteraryTexts {
                default_suffix: Some("【文学增强纪律】\n测试增强段"),
                ..Default::default()
            },
        );
        let sys = &msgs2[0].content;
        assert!(sys.contains("【文学增强纪律】"), "增强段应追加进 system");
        assert!(
            sys.find("文学创作系统").unwrap() < sys.find("【文学增强纪律】").unwrap(),
            "增强段应在兜底模板之后"
        );

        let mut vars3 = HashMap::new();
        let (msgs3, _) = build_llm_messages_with_position(
            "芽衣",
            "兔族少女。",
            "",
            "",
            &[],
            &[],
            &[],
            &[],
            Some("CUSTOM"),
            None,
            None,
            "user",
            None,
            "user",
            &mut vars3,
            &mut AssistantVars::new(),
            None,
            LiteraryTexts {
                default_suffix: Some("【文学增强纪律】\n测试增强段"),
                ..Default::default()
            },
        );
        assert!(
            !msgs3[0].content.contains("【文学增强纪律】"),
            "自定义提示词路径不由本参数追加(归 LIT-3)"
        );
    }

    /// build_llm_messages:简单模式合成文本拼入 system;楼层按位置插入历史
    #[test]
    fn build_messages_injects_simple_and_floors() {
        let mut vars = HashMap::new();
        let mut cfg = PromptInjectConfig {
            mode: InjectMode::Simple,
            ..PromptInjectConfig::default()
        };
        cfg.simple.word_count_enabled = true;
        cfg.simple.word_count = 99;
        let history = vec![("user".to_string(), "你好".to_string())];
        let (msgs, _) = build_llm_messages(
            "芽衣",
            "兔族少女。",
            "",
            "",
            None,
            &history,
            None,
            Some(&cfg),
            &mut vars,
            &mut AssistantVars::new(),
        );
        assert_eq!(msgs.len(), 2);
        assert!(
            msgs[0].content.contains("99 字"),
            "msgs[0]: {}",
            msgs[0].content
        );
        assert!(
            msgs[0].content.contains("文学创作系统"),
            "内置默认应含创作总纲: {}",
            msgs[0].content
        );

        // 复杂模式:楼层统一归位位置4 —— system 角色进 system 尾部,user/assistant 紧随 system 按 order 排
        // (旧 before/after/depth 历史内散插已废弃;position 字段仅兼容解析,不再参与注入)
        cfg.mode = InjectMode::Complex;
        cfg.simple.word_count_enabled = false;
        cfg.floors = vec![
            PromptFloor {
                id: "f1".into(),
                name: "开".into(),
                content: "{{char}}的开场".into(),
                role: FloorRole::User,
                position: FloorPosition::Before,
                depth: 0,
                enabled: true,
                order: 0,
            },
            PromptFloor {
                id: "f2".into(),
                name: "变".into(),
                content: "{{setvar::地点::酒馆}}地点={{getvar::地点}}".into(),
                role: FloorRole::System,
                position: FloorPosition::System,
                depth: 0,
                enabled: true,
                order: 1,
            },
            PromptFloor {
                id: "f3".into(),
                name: "尾".into(),
                content: "尾{{getvar::地点}}".into(),
                role: FloorRole::Assistant,
                position: FloorPosition::After,
                depth: 0,
                enabled: true,
                order: 2,
            },
        ];
        let (msgs, _) = build_llm_messages(
            "芽衣",
            "兔族少女。",
            "",
            "",
            None,
            &history,
            None,
            Some(&cfg),
            &mut vars,
            &mut AssistantVars::new(),
        );
        // system(含 f2 宏)+ f1(user 楼层)+ f3(assistant 楼层)+ 历史用户消息
        assert_eq!(msgs.len(), 4, "消息序列: {msgs:?}");
        assert!(
            msgs[0].content.contains("地点=酒馆"),
            "system 宏: {}",
            msgs[0].content
        );
        assert_eq!(msgs[1].role, "user");
        assert_eq!(msgs[1].content, "芽衣的开场");
        assert_eq!(msgs[2].role, "assistant");
        assert_eq!(msgs[2].content, "尾酒馆");
        assert_eq!(msgs[3].role, "user");
        assert_eq!(msgs[3].content, "你好");
        // setvar 写入了 vars,可被持久化
        assert_eq!(vars.get("地点").map(|s| s.as_str()), Some("酒馆"));
    }

    /// 空 system 楼层不得向系统提示词注入空白(回归:此前 system 分支无空检查,
    /// 导入 ST 预设的锚点/组头空块会以 "\n\n" 形式拼进系统提示词尾部;
    /// 非 system 空楼层已有跳过,见上方 order 分支)。
    #[test]
    fn build_messages_skips_empty_system_floor() {
        let mut vars = HashMap::new();
        let mut with_empty = PromptInjectConfig {
            mode: InjectMode::Complex,
            ..PromptInjectConfig::default()
        };
        with_empty.floors = vec![
            PromptFloor {
                id: "empty".into(),
                name: "空锚点".into(),
                content: "".into(),
                role: FloorRole::System,
                position: FloorPosition::System,
                depth: 0,
                enabled: true,
                order: 0,
            },
            PromptFloor {
                id: "comment".into(),
                name: "仅注释".into(),
                content: "{{// 仅注释,展开后为空 }}".into(),
                role: FloorRole::System,
                position: FloorPosition::System,
                depth: 0,
                enabled: true,
                order: 1,
            },
            PromptFloor {
                id: "real".into(),
                name: "有内容".into(),
                content: "有效指令".into(),
                role: FloorRole::System,
                position: FloorPosition::System,
                depth: 0,
                enabled: true,
                order: 2,
            },
        ];
        let mut reference = PromptInjectConfig {
            mode: InjectMode::Complex,
            ..PromptInjectConfig::default()
        };
        reference.floors = vec![PromptFloor {
            id: "real".into(),
            name: "有内容".into(),
            content: "有效指令".into(),
            role: FloorRole::System,
            position: FloorPosition::System,
            depth: 0,
            enabled: true,
            order: 0,
        }];
        let mut vars_ref = HashMap::new();
        let (msgs, _) = build_llm_messages(
            "芽衣",
            "兔族少女。",
            "",
            "",
            None,
            &[],
            None,
            Some(&with_empty),
            &mut vars,
            &mut AssistantVars::new(),
        );
        let (base, _) = build_llm_messages(
            "芽衣",
            "兔族少女。",
            "",
            "",
            None,
            &[],
            None,
            Some(&reference),
            &mut vars_ref,
            &mut AssistantVars::new(),
        );
        assert!(
            msgs[0].content.contains("有效指令"),
            "有内容的 system 楼层仍须注入: {}",
            msgs[0].content
        );
        assert_eq!(
            msgs[0].content, base[0].content,
            "空 system 楼层不得改变系统提示词(预期与无空楼层一致)"
        );
    }
}
