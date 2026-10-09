// 导入导出路由:/api/export/chat、/api/import/chat(SillyTavern 兼容)
// + /api/export/bundle(IMPHUB 综合导出,bundle v1,2026-10-09)
use crate::api::app_state::AppState;
use crate::api::{db_err, internal, not_found, validation};
use crate::models::types::StMessage;
use crate::parsing::regex_script::RegexScript;
use crate::plugins::ToolPluginLoader;
use crate::services::agent_flow_service::FlowBundle;
use crate::services::prompt_inject_service::PromptInjectConfig;
use crate::services::settings_service::AppMode;
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

#[derive(Deserialize)]
pub struct SessionQuery {
    #[serde(default)]
    pub session_id: Option<String>,
}

#[derive(Deserialize)]
pub struct ImportBody {
    #[serde(default)]
    pub session_id: Option<String>,
    pub messages: Vec<StMessage>,
}

/// GET /api/export/chat:导出会话消息
pub async fn export_chat(
    State(state): State<Arc<AppState>>,
    Query(q): Query<SessionQuery>,
) -> Response {
    let Some(sid) = q.session_id else {
        return validation("缺少 session_id 查询参数");
    };
    if sid.trim().is_empty() {
        return validation("缺少 session_id 查询参数");
    }
    let svc = state.sessions.clone();
    let sid_q = sid.clone();
    let messages = match state
        .db_call(move || {
            // 存在性校验 + 导出合并进同一阻塞任务(两次读取一次调度)
            svc.get(&sid_q)?;
            Some(svc.export_chat(&sid_q))
        })
        .await
    {
        Err(e) => return db_err(&e),
        Ok(None) => return not_found("会话不存在"),
        Ok(Some(m)) => m,
    };
    Json(json!({ "session_id": sid, "messages": messages })).into_response()
}

/// POST /api/import/chat:导入消息(先清空再逐条追加)
pub async fn import_chat(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ImportBody>,
) -> Response {
    let Some(sid) = body.session_id else {
        return validation("缺少 session_id");
    };
    if sid.trim().is_empty() {
        return validation("缺少 session_id");
    }
    let svc = state.sessions.clone();
    let sid_q = sid.clone();
    let imported = state
        .db_call(move || {
            // 存在性校验 + 导入合并进同一阻塞任务
            svc.get(&sid_q)?;
            Some(svc.import_chat(&sid_q, body.messages))
        })
        .await;
    match imported {
        Err(e) => db_err(&e),
        Ok(None) => not_found("会话不存在"),
        Ok(Some(Ok(n))) => Json(json!({ "ok": true, "imported": n })).into_response(),
        Ok(Some(Err(e))) => validation(e),
    }
}

// ==================== 综合导出:IMPHUB bundle v1(2026-10-09) ====================
//
// 导出件是**跨机器文件**(用户会拷到另一台机器再导入),字段漂移代价最高;
// 故与流程搬运包(FlowBundle)同款纪律——**具名结构体,不用 json! 手拼**:
//   ① 段序固定:`serde_json::Map` 默认按字母序,`json!` 拼出来的段序不受控;
//      serde 按字段声明序序列化,段序由类型定义一处锁定(单测逐段断言);
//   ② 形状可机检:bundle 是前端「综合导入」预览与执行两侧共读的线格式,
//      类型化后可与前端 `ImportBundle` 做差集比对(同 FlowBundle 的登记口径);
//   ③ 模式取段单一出处:①预设条目/⑥正则是角色扮演专属,裁剪收在 [`build_bundle`] 里,
//      调用方不必各自记得裁。

/// 段键版本(`kedai_import_bundle` 的值)。导入侧 `> 1` 拒绝(「文件由更新版本导出」)。
pub const IMPORT_BUNDLE_VERSION: u32 = 1;

/// 导出件生成方声明(仅溯源展示;导入端忽略)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleGenerator {
    pub app: String,
    pub version: String,
}

/// ① 预设条目段:kedai 注入形态(判别位 `format` + 注入配置本体)。
/// `st_preset` 形态不落本结构——那是「酒馆预设原文件整段交后端解析」的另一条路,
/// 由前端按文件形状分流到 `POST /api/prompt-inject/import`(见 IMPHUB 执行稿 §2.1.5)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresetEntriesSection {
    pub format: String,
    #[serde(flatten)]
    pub config: PromptInjectConfig,
}

/// ③④ 提示词段:系统 agent 提示词(双模式变体)+ 运行时主 Agent 提示词。
/// 空串语义 = 「未自定义」(导入侧不写该变体、不覆盖本机)——契约文档同一口径。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptsSection {
    pub system: SystemPrompts,
    pub runtime: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemPrompts {
    pub roleplay: String,
    pub task: String,
}

/// ⑤ 技能段条目:与导入形状(`SkillImport`)同形——不带 id/created_at
/// (导入端按 name 覆盖,id 由本机分配),`allowed_tools` 是**数组**(不是库里的 JSON 字符串)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillEntry {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    #[serde(default)]
    pub run_as_subagent: bool,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub folder: String,
}

/// ⑦ 插件段条目:文件名 + 文件夹 + 插件配置本体(导出时已按解析后的规范形状归一)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginEntry {
    pub file: String,
    #[serde(default)]
    pub folder: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub parameters: serde_json::Value,
    pub script: String,
}

/// 6 段载荷。段缺省(`None`)→ 序列化时**整段省略**;task 模式天然不含 ①⑥。
/// 字段声明序 = 导出件的段序契约:**preset_entries → flows → prompts → skills → regex_scripts → plugins**。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BundleSections {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset_entries: Option<PresetEntriesSection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flows: Option<FlowBundle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompts: Option<PromptsSection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<SkillEntry>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regex_scripts: Option<Vec<RegexScript>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugins: Option<Vec<PluginEntry>>,
}

/// 综合导出件(信封 + 段):顶层只放元信息,**全部内容进 `sections`**
/// (未知段键天然可忽略,前向兼容规则清晰)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportBundleEnvelope {
    /// 版本键(见 [`IMPORT_BUNDLE_VERSION`])
    #[serde(rename = "kedai_import_bundle")]
    pub version: u32,
    pub exported_at: String,
    pub generator: BundleGenerator,
    /// 导出方模式(仅供人工识别;导入取段一律以本机当前模式为准)
    pub app_mode: String,
    pub sections: BundleSections,
}

/// 组装导出件(**纯函数**:单测据此锁段序/版本键/模式取段,不必起服务)。
///
/// 模式裁剪在**这里**做:roleplay 全 6 段,task 只 ②③④⑤⑦——把段都取出来交给本函数,
/// 由它按模式裁,避免「哪个调用方忘了裁」这类分叉(①⑥ 是角色扮演专属)。
pub fn build_bundle(
    mode: AppMode,
    version: &str,
    exported_at: String,
    mut sections: BundleSections,
) -> ImportBundleEnvelope {
    if mode == AppMode::Task {
        sections.preset_entries = None;
        sections.regex_scripts = None;
    }
    ImportBundleEnvelope {
        version: IMPORT_BUNDLE_VERSION,
        exported_at,
        generator: BundleGenerator {
            app: "kedai".to_string(),
            version: version.to_string(),
        },
        app_mode: mode.as_str().to_string(),
        sections,
    }
}

#[derive(Deserialize)]
pub struct ExportBundleQuery {
    /// 导出模式:`roleplay`(缺省,兼容不带 mode 的调用方)/ `task`——决定包含哪些段(①⑥ 仅角色扮演)
    #[serde(default)]
    pub mode: Option<String>,
}

/// 导出端点的响应形状(**具名结构体,不得换回 `json!`**):段序契约全靠这里——
/// `json!` 会把信封塞进 `serde_json::Value`,而 `Value` 的 Map 是 BTreeMap(键按**字母序**),
/// ①→⑦ 的段序会在 HTTP 边界被静默打乱(实测:端点级测试抓出 flows 排到 preset_entries 之前);
/// 具名结构体直序列化进响应体,字段声明序即输出序。
#[derive(Serialize)]
struct ExportBundleResponse {
    ok: bool,
    bundle: ImportBundleEnvelope,
    /// 插件目录里解析失败的文件(不导出;原因如实回报,不影响其余段)
    warnings: Vec<String>,
}

/// GET /api/export/bundle?mode=roleplay|task:组装**综合导出件**(IMPHUB bundle v1)。
///
/// 各段来源(与综合导入的逐段执行端点一一对应):
/// - ① preset_entries:内存注入配置(恒 `kedai_inject`;floors 按 order 排序并重编——导出件归一)
/// - ② flows:流程库全库搬运包(与 /api/agent-flows/export 同一函数,形状同源)
/// - ③④ prompts:设置里的双模式系统提示词(**原始值**,空串 = 未自定义)+ DATA_DIR 运行时提示词
/// - ⑤ skills:技能库全量(含停用;含 folder)
/// - ⑥ regex_scripts:全局正则库(角色扮演专属)
/// - ⑦ plugins:data/plugins/tools/*.json 的解析结果(解析失败的项**不导出**,原因随 `warnings` 回报)
pub async fn export_bundle(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ExportBundleQuery>,
) -> Response {
    let mode = AppMode::parse(q.mode.as_deref().unwrap_or(""));

    // ① 预设条目(仅角色扮演):导出件里 floors 按 order 排序并重编为 0..n
    let preset_entries = if mode == AppMode::Roleplay {
        let cfg = state
            .prompt_inject
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get()
            .clone();
        let mut floors = cfg.floors;
        floors.sort_by_key(|f| f.order);
        for (i, f) in floors.iter_mut().enumerate() {
            f.order = i;
        }
        Some(PresetEntriesSection {
            format: "kedai_inject".to_string(),
            config: PromptInjectConfig {
                mode: cfg.mode,
                simple: cfg.simple,
                floors,
            },
        })
    } else {
        None
    };

    // ② 流程:全库搬运包(持锁只读,与 export_agent_flows 同一路径)
    let flow = state.flow.clone();
    let flows = match state
        .db_call(move || {
            let svc = flow.lock().unwrap_or_else(|e| e.into_inner());
            svc.export_bundle(None)
        })
        .await
    {
        Ok(Ok(bundle)) => Some(bundle),
        Ok(Err(e)) => return validation(e),
        Err(e) => return internal(e),
    };

    // ③④ 提示词:设置(不留锁跨 await,取快照)+ 运行时提示词文件
    let settings = state.settings_snapshot();
    let runtime = match state.runtime_prompt.read() {
        Ok(text) => text,
        Err(e) => return internal(e),
    };
    let prompts = Some(PromptsSection {
        system: SystemPrompts {
            roleplay: settings.agent_system_prompt.0.clone(),
            // task 变体是覆盖层原始值(None = 未自定义 → 空串);不导出 for_mode 合并后的
            // 内置缺省词——那会把「跟随内置演进」变成「导入即钉死当前版本」
            task: settings
                .task
                .agent_system_prompt
                .as_ref()
                .map(|v| v.0.clone())
                .unwrap_or_default(),
        },
        runtime,
    });

    // ⑤ 技能:库全量(含停用)
    let skills = state.skills.clone();
    let skill_records = match state.db_call(move || skills.list(false)).await {
        Ok(list) => list,
        Err(e) => return internal(e),
    };
    let skills = Some(
        skill_records
            .into_iter()
            .map(|s| SkillEntry {
                name: s.name,
                description: s.description,
                content: s.content,
                allowed_tools: crate::services::skill_service::parse_allowed_tools(
                    &s.allowed_tools,
                ),
                run_as_subagent: s.run_as_subagent,
                model: s.model,
                folder: s.folder,
            })
            .collect::<Vec<_>>(),
    );

    // ⑥ 全局正则(仅角色扮演)
    let regex_scripts = if mode == AppMode::Roleplay {
        Some(
            state
                .regex
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get()
                .scripts
                .clone(),
        )
    } else {
        None
    };

    // ⑦ 插件:磁盘解析结果(同步遍历 + 逐文件解析,挪阻塞线程池;B-1)
    let dir = state.config.data_dir.join("plugins").join("tools");
    let (loaded, warnings) =
        tokio::task::spawn_blocking(move || ToolPluginLoader::new(dir).parse_all())
            .await
            .unwrap_or_default();
    let plugins = Some(
        loaded
            .into_iter()
            .map(|p| PluginEntry {
                file: p.file,
                folder: p.folder,
                name: p.definition.name,
                description: p.definition.description,
                parameters: p.definition.parameters,
                script: p.script,
            })
            .collect::<Vec<_>>(),
    );

    let envelope = build_bundle(
        mode,
        env!("CARGO_PKG_VERSION"),
        crate::models::db::now_iso(),
        BundleSections {
            preset_entries,
            flows,
            prompts,
            skills,
            regex_scripts,
            plugins,
        },
    );
    Json(ExportBundleResponse {
        ok: true,
        bundle: envelope,
        warnings,
    })
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 全 6 段都能填上的载荷(用于锁「模式取段」与「段序」)
    fn full_sections() -> BundleSections {
        BundleSections {
            preset_entries: Some(PresetEntriesSection {
                format: "kedai_inject".to_string(),
                config: PromptInjectConfig::default(),
            }),
            flows: Some(FlowBundle {
                kedai_flow_bundle: 1,
                exported_at: "t".to_string(),
                root_id: None,
                flows: Vec::new(),
            }),
            prompts: Some(PromptsSection {
                system: SystemPrompts {
                    roleplay: "r".to_string(),
                    task: "t".to_string(),
                },
                runtime: "x".to_string(),
            }),
            skills: Some(Vec::new()),
            regex_scripts: Some(Vec::new()),
            plugins: Some(Vec::new()),
        }
    }

    /// 段序固定:serde 按字段声明序输出 —— preset_entries → flows → prompts → skills
    /// → regex_scripts → plugins(手拼 json! 的 Map 会按字母序,故必须由类型承载)
    #[test]
    fn bundle_section_order_is_fixed() {
        let envelope = build_bundle(AppMode::Roleplay, "9.9.9", "t".into(), full_sections());
        let text = serde_json::to_string(&envelope).unwrap();
        assert!(
            text.starts_with(r#"{"kedai_import_bundle":1,"exported_at":"t","generator":"#),
            "信封键序/版本键不对: {text}"
        );
        let pos = |k: &str| text.find(&format!("\"{k}\"")).unwrap_or(usize::MAX);
        assert!(pos("preset_entries") < pos("flows"), "{text}");
        assert!(pos("flows") < pos("prompts"), "{text}");
        assert!(pos("prompts") < pos("skills"), "{text}");
        assert!(pos("skills") < pos("regex_scripts"), "{text}");
        assert!(pos("regex_scripts") < pos("plugins"), "{text}");
        assert_eq!(envelope.generator.app, "kedai");
        assert_eq!(envelope.generator.version, "9.9.9");
        assert_eq!(envelope.app_mode, "roleplay");
    }

    /// 模式取段:task 模式**一律裁掉** ①预设条目与 ⑥正则(哪怕调用方填了),
    /// 其余四段照旧;app_mode 如实记录导出方模式
    #[test]
    fn task_mode_drops_roleplay_only_sections() {
        let envelope = build_bundle(AppMode::Task, "9.9.9", "t".into(), full_sections());
        assert!(envelope.sections.preset_entries.is_none());
        assert!(envelope.sections.regex_scripts.is_none());
        assert!(envelope.sections.flows.is_some());
        assert!(envelope.sections.prompts.is_some());
        assert!(envelope.sections.skills.is_some());
        assert!(envelope.sections.plugins.is_some());
        assert_eq!(envelope.app_mode, "task");
        let text = serde_json::to_string(&envelope).unwrap();
        assert!(!text.contains("preset_entries"), "task 导出不得含①: {text}");
        assert!(!text.contains("regex_scripts"), "task 导出不得含⑥: {text}");
    }

    /// 空段省略 + 载荷全在 sections 内(顶层不得出现段键)
    #[test]
    fn envelope_keeps_payload_inside_sections() {
        let envelope = build_bundle(
            AppMode::Roleplay,
            "9.9.9",
            "t".into(),
            BundleSections::default(),
        );
        let text = serde_json::to_string(&envelope).unwrap();
        assert!(text.contains(r#""sections":{}"#), "空段应整体省略: {text}");
        // 顶层只放元信息:段键只应出现在 sections 之内(此处全空,故整体不出现)
        for key in [
            "preset_entries",
            "flows",
            "prompts",
            "skills",
            "regex_scripts",
            "plugins",
        ] {
            assert!(
                !text.contains(&format!("\"{key}\"")),
                "段键漏到顶层: {text}"
            );
        }
    }

    /// ⑤ 技能段条目形状:allowed_tools 是**数组**(不是库里的 JSON 字符串),folder 透传
    #[test]
    fn skill_entry_shape_is_import_ready() {
        let entry = SkillEntry {
            name: "写作".to_string(),
            description: "d".to_string(),
            content: "c".to_string(),
            allowed_tools: vec!["read".to_string()],
            run_as_subagent: false,
            model: String::new(),
            folder: "写作/草稿".to_string(),
        };
        let v = serde_json::to_value(&entry).unwrap();
        assert_eq!(v["allowed_tools"], json!(["read"]));
        assert_eq!(v["folder"], "写作/草稿");
        // 反向也收得下(导入端把段条目直接喂给 POST /api/skills 的 SkillImport)
        let back: SkillEntry = serde_json::from_value(v).unwrap();
        assert_eq!(back.folder, "写作/草稿");
    }
}
