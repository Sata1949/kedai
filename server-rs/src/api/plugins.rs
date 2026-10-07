// 插件 API:/api/plugins/tools(列出已加载工具插件 + 热重载 + 导入)
//
// 代际位置:本层是**组合根的一部分**(L2 · 暴露面)。插件解析在 L3(`plugins/`),
// 但**注册是装配动作**,按三结合纪律由宿主(本层)执行——L3 不得依赖 L2 的工具注册表。
// 故下方统一走「解析(L3) → 宿主全量同步(L2)」两段式:解析 `parse_all` +
// 同步 `sync_plugins`(启动装配与 reload/upload/delete 四个入口共用同一条路径)。
use crate::api::app_state::AppState;
use crate::api::{err_status, internal, validation, ErrorCode, WithStatus};
use crate::plugins::ToolPluginLoader;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use std::sync::Arc;

/// 把插件目录的解析结果**全量同步**进工具表(宿主侧装配动作;PLGM 1.1,2026-10-07)。
///
/// 语义 = 「差集注销 + 注册」两段,替代旧版「只注册不注销」:
/// - **注销**:注册表中 `origin=Plugin` 且**不在本批新集合**的名字(删文件/改名的旧注册由此收敛);
/// - **注册**:逐条登记,三查防冲突且**不覆盖**——
///   ① 与内置工具重名拒绝(插件不得劫持内置,已知限制 L19);
///   ② 批内跨文件重名**先到者注册、后到者进 errors**(文件按名排序,确定性);
///   ③ 同名插件重载 = 覆盖「自己」的旧注册(允许,即热更新语义)。
///
/// `pub(crate)`:启动装配(`api::app_state`)与 reload/upload/delete 四个入口全部走本函数
/// ——同一条路径即同一口径。返回 `(注册数, 注销数, 逐项错误)`。
pub(crate) fn sync_plugins(
    loaded: Vec<crate::plugins::LoadedToolPlugin>,
    registry: &crate::tools::registry::ToolRegistry,
) -> (usize, usize, Vec<String>) {
    let mut errors = Vec::new();
    // 1) 差集注销:现存 Plugin 名字 - 本批名字
    let incoming: std::collections::HashSet<&str> =
        loaded.iter().map(|p| p.definition.name.as_str()).collect();
    let mut removed = 0usize;
    for name in registry.names_by_origin(crate::models::tool_policy::ToolOrigin::Plugin) {
        if !incoming.contains(name.as_str()) {
            registry.unregister(&name);
            removed += 1;
        }
    }
    // 2) 注册:先到者生效;同批内同名到第二次即报错并跳过(不覆盖先到者)
    let mut registered = 0usize;
    let mut first_file: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for plugin in loaded {
        let name = plugin.definition.name.clone();
        if registry.is_builtin(&name) {
            errors.push(format!(
                "{}: 工具名 {name} 与内置工具重名，已拒绝注册（插件不得劫持内置工具）",
                plugin.file
            ));
            continue;
        }
        if let Some(other) = first_file.get(&name) {
            errors.push(format!(
                "{}: 工具名 {name} 与 {other} 重复，已跳过（同一批次先到者生效）",
                plugin.file
            ));
            continue;
        }
        first_file.insert(name.clone(), plugin.file.clone());
        registry.register_external(
            plugin.definition,
            crate::plugins::plugin_executor(&plugin.script),
            None,
            crate::models::tool_policy::ToolOrigin::Plugin,
        );
        registered += 1;
    }
    (registered, removed, errors)
}

/// GET /api/plugins/tools:列出**插件来源**工具(每项带来源文件)+ 磁盘插件文件
///
/// 口径(PLGM 1.4):`tools` 只回 `origin=Plugin` 的已注册工具——内置/MCP 工具不属本面板
/// (此前回全量注册表,与面板文案「内置不在此列表」自相矛盾)。`file` 取目录解析结果
/// (与注册同目录同快照):文件被删/解析失败而尚未 reload 的旧注册项不回列,reload 即收敛。
pub async fn list_tools(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let dir = state.config.data_dir.join("plugins").join("tools");
    let registry = state.tool_registry.clone();
    // list_files/parse_all 是同步目录遍历 + 逐文件解析,挪阻塞线程池(B-1:不在 tokio worker 上做同步文件 IO)
    let (files, parsed) = tokio::task::spawn_blocking(move || {
        let loader = ToolPluginLoader::new(dir);
        let files = loader.list_files();
        let (loaded, _errors) = loader.parse_all();
        (files, loaded)
    })
    .await
    .unwrap_or_default();
    let tools: Vec<serde_json::Value> = parsed
        .into_iter()
        .filter(|p| {
            registry.origin_of(&p.definition.name)
                == Some(crate::models::tool_policy::ToolOrigin::Plugin)
        })
        .map(|p| {
            json!({
                "name": p.definition.name,
                "description": p.definition.description,
                "origin": "plugin",
                "file": p.file,
            })
        })
        .collect();
    Json(json!({
        "tools": tools,
        "files": files,
    }))
}

/// POST /api/plugins/tools/reload:重新加载磁盘上的工具插件(全量同步:增删都收敛)
pub async fn reload_tools(State(state): State<Arc<AppState>>) -> Response {
    let dir = state.config.data_dir.join("plugins").join("tools");
    let registry = state.tool_registry.clone();
    // parse_all 是同步目录遍历 + 逐文件解析,挪阻塞线程池(B-1);
    // 注册在 spawn_blocking 之外由本层执行(注册表是同步锁操作,无需阻塞池)。
    let (loaded, mut errors) =
        tokio::task::spawn_blocking(move || ToolPluginLoader::new(dir).parse_all())
            .await
            .unwrap_or_else(|e| {
                // 泄露封堵(批次 1):JoinError 原文(线程池内部细节)只进日志,
                // 面向用户的错误项换成固定文案。
                tracing::error!(error = %e, "插件重载任务失败");
                (
                    Vec::new(),
                    vec!["插件重载任务失败,详情见服务端日志".to_string()],
                )
            });
    let (count, removed, reg_errors) = sync_plugins(loaded, &registry);
    errors.extend(reg_errors);
    if errors.is_empty() {
        Json(json!({ "ok": true, "loaded": count, "removed": removed }))
            .into_response()
            .with_status(StatusCode::OK)
    } else {
        // 形状(批次 1):`loaded`(已注册数)/`removed`(注销数)与 `errors`(逐项失败原因)是
        // **业务数据**,必须保留;仅补 `code` 供前端统一分支。errors 原文由解析器产生(面向插件的
        // 文件名/语法错误,不含密钥/路径绝对化),属用户排障必需信息,保留透出。
        Json(json!({
            "ok": false,
            "code": ErrorCode::Validation.as_str(),
            "error": "部分插件加载失败",
            "loaded": count,
            "removed": removed,
            "errors": errors,
        }))
        .into_response()
        .with_status(StatusCode::BAD_REQUEST)
    }
}

/// 插件文件名净化(上传/删除统一策略):仅保留 ASCII 字母数字、-、_、.;
/// 净化结果必须与原始文件名逐字符一致,否则视为含路径分隔符/非法字符,返回 None 拒绝。
/// 不一致即拒绝而不是静默改写:静默改写会让「上传 a/b.json 落盘为 ab.json」产生歧义,
/// 也让路径穿越输入(../x.json)以改写后的形态落盘,口径与删除入口的既有比较保持一致。
fn sanitize_plugin_filename(name: &str) -> Option<String> {
    let safe: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_' || *c == '.')
        .collect();
    if safe.is_empty() || safe != name {
        None
    } else {
        Some(safe)
    }
}

/// POST /api/plugins/tools/upload:导入工具插件 JSON 文件(写入 data/plugins/tools 并注册)。
/// multipart 字段名 file,复用 characters 的通用 multipart 解析。
pub async fn upload_tool(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let Some(boundary) = crate::api::characters::parse_boundary(content_type) else {
        return err_status("缺少文件字段(file)", StatusCode::BAD_REQUEST);
    };
    let parts = crate::api::characters::parse_multipart(&body, &boundary);
    let Some(file) = parts.into_iter().find(|p| p.filename.is_some()) else {
        return err_status("缺少文件字段(file)", StatusCode::BAD_REQUEST);
    };
    let file_name = file.filename.unwrap_or_else(|| "plugin.json".to_string());
    if !file_name.to_lowercase().ends_with(".json") {
        return err_status("插件文件须为 .json 格式", StatusCode::BAD_REQUEST);
    }
    let file_bytes = file.content;
    // 校验 JSON 结构(至少含 name / script)
    let cfg: Result<crate::plugins::ToolPluginConfig, _> = serde_json::from_slice(&file_bytes);
    match cfg {
        Ok(c) => {
            if c.name.trim().is_empty() || c.script.trim().is_empty() {
                return err_status("插件须包含 name 与 script 字段", StatusCode::BAD_REQUEST);
            }
            // 文件名净化(与删除入口统一策略):净化结果不等于原始名 → 拒绝
            let Some(safe) = sanitize_plugin_filename(&file_name) else {
                return err_status("文件名含非法字符", StatusCode::BAD_REQUEST);
            };
            let dir = state.config.data_dir.join("plugins").join("tools");
            // B-1:文件 IO 走 tokio::fs / 阻塞线程池,不在 tokio worker 上同步读写
            tokio::fs::create_dir_all(&dir).await.ok();
            let path = dir.join(&safe);
            if let Err(e) = tokio::fs::write(&path, &file_bytes).await {
                // 500:入参含 io 错误原文(可能带盘符路径),按泄露策略只进日志
                return internal(format!("写文件失败: {e}"));
            }
            // 全量同步(与启动/reload/delete 同一条路径):先解析整目录,再 diff 注册。
            // 本文件若解析失败(含加载期语法校验)或注册冲突,以其归属错误回 400;
            // 其余文件的既有错误只进日志(它们属于那些文件的 reload 反馈)。
            let registry = state.tool_registry.clone();
            let sync_dir = dir.clone();
            let (loaded, parse_errors) =
                tokio::task::spawn_blocking(move || ToolPluginLoader::new(sync_dir).parse_all())
                    .await
                    .unwrap_or_default();
            let mine = format!("{safe}:");
            if let Some(err) = parse_errors.iter().find(|e| e.starts_with(&mine)) {
                return validation(format!("插件注册失败: {err}"));
            }
            let (count, removed, reg_errors) = sync_plugins(loaded, &registry);
            if let Some(e) = reg_errors.iter().find(|e| e.starts_with(&mine)) {
                return validation(format!("插件注册失败: {e}"));
            }
            for e in parse_errors.iter().chain(reg_errors.iter()) {
                tracing::warn!(plugin_error = %e, "插件目录存在其他加载错误");
            }
            Json(json!({
                "ok": true,
                "name": c.name,
                "file": safe,
                "loaded": count,
                "removed": removed,
            }))
            .into_response()
            .with_status(StatusCode::CREATED)
        }
        Err(e) => validation(format!("插件 JSON 解析失败: {e}")),
    }
}

/// DELETE /api/plugins/tools/{name}:删除已导入的工具插件文件(工具随之注销)
pub async fn delete_tool(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Response {
    // 文件名净化(与上传入口统一策略):净化结果不等于原始名 → 拒绝
    let Some(safe) = sanitize_plugin_filename(&name) else {
        return err_status("非法文件名", StatusCode::BAD_REQUEST);
    };
    if !safe.to_lowercase().ends_with(".json") {
        return err_status("非法文件名", StatusCode::BAD_REQUEST);
    }
    let dir = state.config.data_dir.join("plugins").join("tools");
    let path = dir.join(&safe);
    // 删文件后全量同步:B-1 文件 IO 走 tokio::fs;同步里 parse_all 是同步遍历,挪阻塞线程池。
    // 工具的注销不再手工「删前解析反查名字」——差集注销天然覆盖(含「删掉的是重名先到者、
    // 后到者应补位」这类情形)。
    if tokio::fs::try_exists(&path).await.unwrap_or(false) {
        let _ = tokio::fs::remove_file(&path).await;
    }
    let registry = state.tool_registry.clone();
    let (loaded, _) = tokio::task::spawn_blocking(move || ToolPluginLoader::new(dir).parse_all())
        .await
        .unwrap_or_default();
    let (count, _unregistered, _) = sync_plugins(loaded, &registry);
    Json(json!({ "ok": true, "removed": name, "reloaded": count })).into_response()
}

#[cfg(test)]
mod tests {
    use super::sanitize_plugin_filename;
    use super::sync_plugins;

    /// 净化策略统一(优化项 B-4):上传与删除入口同一判定——
    /// 净化结果与原始文件名不一致即拒绝,不再静默改写/回退默认名。
    #[test]
    fn sanitize_rejects_traversal_and_illegal_chars() {
        // 路径穿越:含目录分隔符的「..」组合被拒
        assert_eq!(sanitize_plugin_filename("../evil.json"), None);
        assert_eq!(sanitize_plugin_filename("..\\evil.json"), None);
        // 目录分隔符(子目录前缀)
        assert_eq!(sanitize_plugin_filename("sub/tool.json"), None);
        // 非法字符:空格、中文、冒号、星号等
        assert_eq!(sanitize_plugin_filename("my tool.json"), None);
        assert_eq!(sanitize_plugin_filename("插件.json"), None);
        assert_eq!(sanitize_plugin_filename("a:b.json"), None);
        assert_eq!(sanitize_plugin_filename("a*.json"), None);
        // 净化后为空(原名全是非法字符)
        assert_eq!(sanitize_plugin_filename("///"), None);
        assert_eq!(sanitize_plugin_filename(""), None);
    }

    /// 合法文件名原样通过(逐字符一致)
    #[test]
    fn sanitize_accepts_legal_names() {
        assert_eq!(
            sanitize_plugin_filename("weather.json"),
            Some("weather.json".to_string())
        );
        // 含 . - _ 的合法组合(净化策略允许点号;无分隔符的「..」开头按普通字符放行,
        // 与既有过滤字符集一致;上传/删除两入口同一函数判定,策略天然一致)
        assert_eq!(
            sanitize_plugin_filename("my-tool_v2.1.json"),
            Some("my-tool_v2.1.json".to_string())
        );
        assert_eq!(
            sanitize_plugin_filename("..json"),
            Some("..json".to_string())
        );
    }

    fn plugin(name: &str, file: &str, desc: &str) -> crate::plugins::LoadedToolPlugin {
        crate::plugins::LoadedToolPlugin {
            definition: crate::models::types::ToolDefinition {
                name: name.into(),
                description: desc.into(),
                parameters: serde_json::json!({}),
            },
            script: "return args.x".into(),
            file: file.into(),
        }
    }

    /// 全量同步语义(PLGM 1.1):删文件 → 差集注销;跨文件重名 → 先到者生效且不覆盖;
    /// 同名重载 → 覆盖自己;内置重名 → 拒绝(不劫持)。
    #[test]
    fn sync_plugins_removes_stale_and_keeps_first_wins() {
        use crate::models::tool_policy::ToolOrigin;
        let reg = crate::tools::registry::ToolRegistry::new();
        // 初始:两个插件 → 全部注册
        let (n, removed, errs) = sync_plugins(
            vec![
                plugin("a_tool", "a.json", "v1"),
                plugin("b_tool", "b.json", "旧"),
            ],
            &reg,
        );
        assert_eq!((n, removed, errs.len()), (2, 0, 0), "{errs:?}");
        assert_eq!(
            reg.names_by_origin(ToolOrigin::Plugin),
            vec!["a_tool", "b_tool"]
        );

        // b.json 被删、a.json 改名 → 差集注销两个旧名,注册新名
        let (n, removed, errs) = sync_plugins(vec![plugin("a_tool2", "a.json", "v2")], &reg);
        assert_eq!((n, removed, errs.len()), (1, 2, 0), "{errs:?}");
        assert_eq!(reg.names_by_origin(ToolOrigin::Plugin), vec!["a_tool2"]);

        // 跨文件重名:先到者(c.json)注册,后到者(d.json)进 errors 且不覆盖
        let (n, _, errs) = sync_plugins(
            vec![
                plugin("dup_tool", "c.json", "先"),
                plugin("dup_tool", "d.json", "后"),
            ],
            &reg,
        );
        assert_eq!(n, 1, "先到者注册");
        assert_eq!(errs.len(), 1, "后到者应报错: {errs:?}");
        assert!(
            errs[0].contains("d.json") && errs[0].contains("c.json"),
            "错误应指明两方文件: {errs:?}"
        );
        assert_eq!(
            reg.get("dup_tool").unwrap().definition.description,
            "先",
            "后到者不得覆盖先到者"
        );

        // 同名重载 = 覆盖自己(热更新):新描述生效、无注销
        let (n, removed, errs) = sync_plugins(vec![plugin("dup_tool", "c.json", "更新")], &reg);
        assert_eq!((n, removed, errs.len()), (1, 0, 0), "{errs:?}");
        assert_eq!(reg.get("dup_tool").unwrap().definition.description, "更新");

        // 内置重名:拒绝注册,内置注册不被顶替
        reg.register(
            crate::models::types::ToolDefinition {
                name: "read".into(),
                description: "内置".into(),
                parameters: serde_json::json!({}),
            },
            std::sync::Arc::new(|_, _| Box::pin(async { Ok(String::new()) })),
        );
        let (n, _, errs) = sync_plugins(vec![plugin("read", "evil.json", "劫持")], &reg);
        assert_eq!(n, 0);
        assert!(errs[0].contains("内置"), "{errs:?}");
        assert_eq!(
            reg.origin_of("read"),
            Some(ToolOrigin::Builtin),
            "内置工具不得被插件顶替"
        );
    }
}
