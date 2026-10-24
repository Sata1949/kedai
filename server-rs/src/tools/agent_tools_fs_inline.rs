// 确定性单文件装配工具(SKY-8,2026-10-24)。
//
// 背景:2026-10-24 全题复跑里总装主 203 轮 / 201 次 fs_read / 0 次 fs_write,
// 在「逐个分片读完、手写 100KB+ 内联入口」的形态下被墙钟收走,`index.html` 未落盘。
// 本工具把「读分片 + 替换槽位 + 写入口」变成一次确定性调用:模型只提供小模板或
// 分片清单,引擎读文件、做替换、拒绝本地外链/相对 import,再原子落盘。
//
// 边界(与 `docs/遗留.md` §三十一 SKY-8 同源):
// - 模板模式:`@@NAME@@` 槽位必须与 `parts` 键一一对应;缺槽/多槽直接报错;
// - 无模板模式:只支持经典脚本(行首 `import`/`export` 拒绝,要求改用模板);
// - 落盘前用 `wiring::single_file_text_issues` 做自包含校验,失败不写文件;
// - 只做文本装配,不执行 JS、不解析完整语法(运行期自检仍是模型/审计职责)。
use super::agent_tools::ToolDeps;
use super::agent_tools_fs::{note_change, resolve};
use super::workspace_scan::display_rel;
use crate::models::types::{ExecScope, ToolContext, ToolDefinition};
use crate::services::task_change_service;
use crate::services::task_engine::wiring;
use crate::tools::registry::ToolRegistry;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

/// 工具名(任务策略按名例外、只读/写类判据、提示词条件段共用)。
pub const INLINE_TOOL: &str = "fs_inline_html";

/// 模板文件读取上限(模板应是小文件;大内容放分片)。
const MAX_TEMPLATE_BYTES: u64 = 1024 * 1024;
/// 单个分片读取上限(与 fs_read/fs_write 同口径)。
const MAX_SOURCE_BYTES: u64 = 8 * 1024 * 1024;
/// 分片数上限(与 wiring 的扫描上限同量级,防止异常工作区拖慢装配)。
const MAX_SOURCES: usize = 64;
/// 装配结果上限(超过即拒绝,避免把异常内容写成巨大 HTML)。
const MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

/// 注册确定性单文件装配工具。
pub fn register_inline_tool(registry: &ToolRegistry, deps: Arc<ToolDeps>) {
    registry.register(
        ToolDefinition {
            name: INLINE_TOOL.into(),
            description: "把工作区里的多个分片内联成一个自包含 HTML(引擎读分片、替换 @@NAME@@ 槽位并写入口);\
                          不要在正文手写大段内联脚本。输出不得引用本地文件或相对 import。"
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "输出的 HTML 工作区相对路径(如 index.html;需 .html/.htm)" },
                    "template": { "type": "string", "description": "可选模板路径;模板中以 @@NAME@@ 标记槽位(如 @@TEXTURES@@)" },
                    "parts": {
                        "type": "object",
                        "description": "模板模式必填:槽位名 → 分片路径。键必须与模板槽位一一对应",
                        "additionalProperties": { "type": "string" }
                    },
                    "sources": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "无模板模式:按顺序内联的经典脚本分片(不得含行首 import/export)"
                    },
                    "title": { "type": "string", "description": "无模板模式生成的 HTML 标题" }
                },
                "required": ["path"]
            }),
        },
        Arc::new(move |args: Value, ctx: ToolContext| {
            let deps = deps.clone();
            Box::pin(async move { run_inline(&deps, &ctx, &args) })
        }),
    );
}

fn run_inline(deps: &ToolDeps, ctx: &ToolContext, args: &Value) -> Result<String, String> {
    let raw_out = args
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if raw_out.is_empty() {
        return Err("path 不能为空:请给出输出的 HTML 工作区相对路径(如 index.html)".into());
    }
    let (scope, out_path) = resolve(deps, ctx, &raw_out)?;
    if !is_html_path(&out_path) {
        return Err(format!(
            "{raw_out} 不是 HTML 文件:输出路径需以 .html/.htm 结尾"
        ));
    }
    // 所有权检查放在读分片之前:越权写应直接告诉模型「这不是你的文件」。
    scope.reserve_write(&out_path)?;
    let out_rel = display_rel(scope.workspace(), &out_path);

    let template = args
        .get("template")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let parts = args.get("parts").and_then(|v| v.as_object());
    let sources = args.get("sources").and_then(|v| v.as_array());
    let title = args
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("Kedai 单文件交付");

    let (rendered, summary) = match template {
        Some(template) => render_template(deps, ctx, &out_path, template, parts)?,
        None => render_shell(deps, ctx, &out_path, sources, title)?,
    };
    if rendered.len() > MAX_OUTPUT_BYTES {
        return Err(format!(
            "装配结果过大({} 字节,上限 {MAX_OUTPUT_BYTES});请减少分片或拆分内容",
            rendered.len()
        ));
    }
    let issues = wiring::single_file_text_issues(&out_rel, &rendered);
    if !issues.is_empty() {
        return Err(format!(
            "装配结果不是自包含单文件,未落盘:{}",
            issues.join(";")
        ));
    }

    let baseline = task_change_service::Baseline::capture(&out_path);
    crate::utils::fs_atomic::write_atomic(&out_path, rendered.as_bytes())
        .map_err(|e| format!("写入失败({out_rel}):{e}"))?;
    if let Ok(stamp) = ExecScope::stamp_of(&out_path) {
        scope.note_read(&out_path, stamp);
    }
    note_change(deps, ctx, &out_rel, &out_path, &baseline);
    Ok(format!(
        "已装配 {out_rel}({} 字节,{summary})",
        rendered.len()
    ))
}

/// 模板模式:严格校验槽位映射,单次扫描替换(不递归展开插入内容)。
fn render_template(
    deps: &ToolDeps,
    ctx: &ToolContext,
    out_path: &Path,
    template_raw: &str,
    parts: Option<&serde_json::Map<String, Value>>,
) -> Result<(String, String), String> {
    let (_, template_path) = resolve(deps, ctx, template_raw)?;
    if same_path(&template_path, out_path) {
        return Err("template 不能与输出路径相同:请把模板放在另一个文件".into());
    }
    let template = read_text(&template_path, "模板", MAX_TEMPLATE_BYTES)?;
    let parts = parts.ok_or_else(|| {
        "template 模式必须提供 parts(槽位名 → 分片路径);无模板时改用 sources".to_string()
    })?;
    if parts.is_empty() {
        return Err("parts 不能为空:请给出「槽位名 → 分片路径」映射".into());
    }
    if parts.len() > MAX_SOURCES {
        return Err(format!(
            "分片过多({} 个,上限 {MAX_SOURCES});请合并分片",
            parts.len()
        ));
    }

    let slots = template_slots(&template)?;
    if slots.is_empty() {
        return Err("模板没有 @@NAME@@ 槽位:请在模板中标注每个分片的插入位置(如 @@MAIN@@)".into());
    }
    let mut missing: Vec<String> = slots
        .iter()
        .filter(|s| !parts.contains_key(s.as_str()))
        .cloned()
        .collect();
    let mut unused: Vec<String> = parts
        .keys()
        .filter(|k| !slots.iter().any(|s| s == *k))
        .cloned()
        .collect();
    missing.sort();
    unused.sort();
    if !missing.is_empty() || !unused.is_empty() {
        let mut detail = String::new();
        if !missing.is_empty() {
            detail.push_str(&format!("模板槽位缺少映射:{}", missing.join(",")));
        }
        if !unused.is_empty() {
            if !detail.is_empty() {
                detail.push(';');
            }
            detail.push_str(&format!("parts 多出未使用键:{}", unused.join(",")));
        }
        return Err(format!(
            "槽位映射不一致:{detail};槽位名必须与模板中的 @@NAME@@ 一一对应"
        ));
    }

    let mut contents: BTreeMap<String, String> = BTreeMap::new();
    for (key, raw) in parts {
        let raw = raw
            .as_str()
            .ok_or_else(|| format!("parts.{key} 必须是分片路径字符串"))?;
        let (_, path) = resolve(deps, ctx, raw)?;
        if same_path(&path, out_path) {
            return Err(format!("分片 {raw} 不能是输出文件本身"));
        }
        contents.insert(
            key.clone(),
            read_text(&path, "分片", MAX_SOURCE_BYTES).map_err(|e| format!("{raw}:{e}"))?,
        );
    }
    let rendered = expand_template(&template, &contents)?;
    if rendered.contains("@@") {
        return Err("装配后仍残留 @@ 槽位:请检查模板/分片里的未替换标记".into());
    }
    Ok((
        rendered,
        format!("模板 {} 个槽位 / {} 个分片", slots.len(), contents.len()),
    ))
}

/// 无模板模式:按 sources 顺序生成经典 `<script>` 外壳。
fn render_shell(
    deps: &ToolDeps,
    ctx: &ToolContext,
    out_path: &Path,
    sources: Option<&Vec<Value>>,
    title: &str,
) -> Result<(String, String), String> {
    let sources = sources.ok_or_else(|| {
        "无模板模式必须提供 sources(按顺序内联的分片路径);需要控制模块边界时改用 template"
            .to_string()
    })?;
    if sources.is_empty() {
        return Err("sources 不能为空:请给出至少一个分片路径".into());
    }
    if sources.len() > MAX_SOURCES {
        return Err(format!(
            "分片过多({} 个,上限 {MAX_SOURCES});请合并分片",
            sources.len()
        ));
    }
    let mut blocks: Vec<String> = Vec::with_capacity(sources.len());
    for value in sources {
        let raw = value
            .as_str()
            .ok_or_else(|| "sources 必须是分片路径字符串数组".to_string())?;
        let (_, path) = resolve(deps, ctx, raw)?;
        if same_path(&path, out_path) {
            return Err(format!("分片 {raw} 不能是输出文件本身"));
        }
        let content =
            read_text(&path, "分片", MAX_SOURCE_BYTES).map_err(|e| format!("{raw}:{e}"))?;
        if has_top_level_esm(&content) {
            return Err(format!(
                "分片 {raw} 含行首 import/export(ESM),无模板模式只支持经典脚本;请提供 template 显式控制模块边界"
            ));
        }
        blocks.push(format!("<script>\n{}\n</script>", content.trim_end()));
    }
    let title = if title.trim().is_empty() {
        "Kedai 单文件交付"
    } else {
        title.trim()
    };
    let html = format!(
        "<!doctype html>\n<html lang=\"zh-CN\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\n<title>{}</title>\n</head>\n<body>\n{}\n</body>\n</html>\n",
        escape_html(title),
        blocks.join("\n")
    );
    Ok((html, format!("{} 个分片按顺序内联", blocks.len())))
}

/// 提取模板槽位名;未闭合/非法字符直接报错(不在装配阶段猜)。
fn template_slots(template: &str) -> Result<Vec<String>, String> {
    let mut slots: Vec<String> = Vec::new();
    let mut rest = template;
    while let Some(start) = rest.find("@@") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("@@") else {
            return Err("模板存在未闭合的 @@ 槽位标记:请补上结尾 @@".into());
        };
        let key = &after[..end];
        if key.is_empty()
            || !key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        {
            return Err(format!("模板槽位名非法:@@{key}@@(只允许字母/数字/_/-/.)"));
        }
        if !slots.iter().any(|s| s == key) {
            slots.push(key.to_string());
        }
        rest = &after[end + 2..];
    }
    Ok(slots)
}

/// 单次扫描替换:插入的分片内容不再参与槽位解析(避免内容里的 `@@` 被二次展开)。
fn expand_template(template: &str, contents: &BTreeMap<String, String>) -> Result<String, String> {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("@@") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("@@") else {
            return Err("模板存在未闭合的 @@ 槽位标记:请补上结尾 @@".into());
        };
        let key = &after[..end];
        let Some(content) = contents.get(key) else {
            return Err(format!("模板槽位 @@{key}@@ 没有对应分片"));
        };
        out.push_str(content);
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

fn read_text(path: &Path, kind: &str, max_bytes: u64) -> Result<String, String> {
    let meta = std::fs::metadata(path).map_err(|e| {
        format!(
            "读取{kind}失败({}):{e};请先用 fs_glob 确认路径拼写",
            path.display()
        )
    })?;
    if !meta.is_file() {
        return Err(format!("{kind} {} 不是文件", path.display()));
    }
    if meta.len() > max_bytes {
        return Err(format!(
            "{kind} {} 过大({} 字节,上限 {max_bytes})",
            path.display(),
            meta.len()
        ));
    }
    let bytes =
        std::fs::read(path).map_err(|e| format!("读取{kind}失败({}):{e}", path.display()))?;
    if bytes.contains(&0) {
        return Err(format!(
            "{kind} {} 含 NUL 字节,判定为二进制",
            path.display()
        ));
    }
    String::from_utf8(bytes).map_err(|_| format!("{kind} {} 不是 UTF-8 文本", path.display()))
}

fn has_top_level_esm(text: &str) -> bool {
    text.lines().any(|line| {
        let line = line.trim_start();
        line.starts_with("import ")
            || line.starts_with("import{")
            || line.starts_with("import.")
            || line.starts_with("export ")
            || line.starts_with("export{")
            || line.starts_with("export*")
    })
}

fn is_html_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| matches!(e.to_ascii_lowercase().as_str(), "html" | "htm"))
        .unwrap_or(false)
}

fn same_path(a: &Path, b: &Path) -> bool {
    a == b
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::test_support::TempDataDir;
    use std::path::PathBuf;

    struct Fixture {
        _guard: TempDataDir,
        deps: Arc<ToolDeps>,
        ws: PathBuf,
        dir: TempDataDir,
        scope: Arc<ExecScope>,
    }

    fn fixture(tag: &str) -> Fixture {
        let (_guard, deps) = ToolDeps::dummy_for_test();
        let dir = TempDataDir::new(&format!("inline-{tag}"));
        let ws = dir.join("ws");
        std::fs::create_dir_all(ws.join("parts")).expect("创建工作区失败");
        std::fs::write(ws.join("parts/a.js"), "window.A=1;").expect("写分片 a 失败");
        std::fs::write(ws.join("parts/b.js"), "window.B=2;").expect("写分片 b 失败");
        std::fs::write(
            ws.join("index.template.html"),
            "<html><script>@@A@@</script><script>@@B@@</script></html>",
        )
        .expect("写模板失败");
        let scope = {
            let ws_str = ws.to_string_lossy().into_owned();
            crate::tools::workspace_guard::scope_for_task(Some(&ws_str))
                .expect("构造作用域失败")
                .expect("工作区应生成作用域")
        };
        Fixture {
            _guard,
            deps: Arc::new(deps),
            ws,
            dir,
            scope,
        }
    }

    impl Fixture {
        fn ctx(&self) -> ToolContext {
            ToolContext {
                session_id: "task:t1".into(),
                character_id: String::new(),
                agent_depth: 0,
                scope: Some(self.scope.clone()),
                budget: None,
            }
        }

        fn call(&self, args: Value) -> Result<String, String> {
            run_inline(&self.deps, &self.ctx(), &args)
        }

        fn registry(&self) -> ToolRegistry {
            let reg = ToolRegistry::new();
            crate::tools::agent_tools_fs::register_fs_tools(&reg, self.deps.clone());
            reg
        }
    }

    #[test]
    fn inline_tool_is_registered() {
        let f = fixture("registered");
        let names: Vec<String> = f
            .registry()
            .list_definitions()
            .into_iter()
            .map(|d| d.name)
            .collect();
        assert!(names.iter().any(|n| n == INLINE_TOOL), "{names:?}");
    }

    #[tokio::test]
    async fn template_mode_replaces_slots_and_writes_self_contained_html() {
        let f = fixture("template-ok");
        let out = f
            .call(json!({
                "path": "dist/index.html",
                "template": "index.template.html",
                "parts": { "A": "parts/a.js", "B": "parts/b.js" }
            }))
            .expect("模板装配应成功");
        assert!(out.contains("2 个槽位"), "{out}");
        let html = std::fs::read_to_string(f.ws.join("dist/index.html")).expect("输出应落盘");
        assert!(html.contains("window.A=1;"), "{html}");
        assert!(html.contains("window.B=2;"), "{html}");
        assert!(!html.contains("@@"), "不得残留槽位:{html}");
        assert!(
            !html.contains("src=\"parts/"),
            "模板装配不得留下本地外链:{html}"
        );
        let _ = std::fs::remove_dir_all(f.dir.path());
    }

    #[tokio::test]
    async fn template_mode_rejects_missing_and_unused_slot_keys() {
        let f = fixture("template-mismatch");
        let err = f
            .call(json!({
                "path": "index.html",
                "template": "index.template.html",
                "parts": { "A": "parts/a.js", "C": "parts/b.js" }
            }))
            .unwrap_err();
        assert!(err.contains("缺少映射") && err.contains("B"), "{err}");
        assert!(err.contains("多出未使用键") && err.contains("C"), "{err}");
        assert!(
            !f.ws.join("index.html").exists(),
            "槽位不一致时不得落盘半成品"
        );
        let _ = std::fs::remove_dir_all(f.dir.path());
    }

    #[tokio::test]
    async fn template_mode_rejects_local_external_refs() {
        let f = fixture("template-external");
        std::fs::write(
            f.ws.join("bad.template.html"),
            "<html><script src=\"parts/a.js\"></script>@@A@@</html>",
        )
        .expect("写模板失败");
        let err = f
            .call(json!({
                "path": "index.html",
                "template": "bad.template.html",
                "parts": { "A": "parts/a.js" }
            }))
            .unwrap_err();
        assert!(err.contains("本地外部资源"), "{err}");
        assert!(!f.ws.join("index.html").exists(), "校验失败不得落盘");
        let _ = std::fs::remove_dir_all(f.dir.path());
    }

    #[tokio::test]
    async fn shell_mode_inlines_sources_in_order() {
        let f = fixture("shell-ok");
        let out = f
            .call(json!({
                "path": "index.html",
                "sources": ["parts/a.js", "parts/b.js"],
                "title": "装配 <测试>"
            }))
            .expect("无模板装配应成功");
        assert!(out.contains("2 个分片"), "{out}");
        let html = std::fs::read_to_string(f.ws.join("index.html")).expect("输出应落盘");
        let a = html.find("window.A=1;").expect("应含 a");
        let b = html.find("window.B=2;").expect("应含 b");
        assert!(a < b, "sources 顺序必须保留:{html}");
        assert_eq!(html.matches("<script>").count(), 2, "{html}");
        assert!(html.contains("装配 &lt;测试&gt;"), "标题应转义:{html}");
        let _ = std::fs::remove_dir_all(f.dir.path());
    }

    #[tokio::test]
    async fn shell_mode_rejects_esm_source() {
        let f = fixture("shell-esm");
        std::fs::write(f.ws.join("parts/esm.js"), "export const A = 1;").expect("写 ESM 分片失败");
        let err = f
            .call(json!({
                "path": "index.html",
                "sources": ["parts/esm.js"]
            }))
            .unwrap_err();
        assert!(err.contains("ESM"), "{err}");
        assert!(!f.ws.join("index.html").exists(), "失败不得落盘");
        let _ = std::fs::remove_dir_all(f.dir.path());
    }

    #[tokio::test]
    async fn missing_source_and_self_reference_are_rejected() {
        let f = fixture("source-errors");
        let err = f
            .call(json!({
                "path": "index.html",
                "sources": ["parts/missing.js"]
            }))
            .unwrap_err();
        assert!(err.contains("读取分片失败"), "{err}");
        let err = f
            .call(json!({
                "path": "parts/a.js",
                "sources": ["parts/a.js"]
            }))
            .unwrap_err();
        assert!(err.contains(".html") || err.contains(".htm"), "{err}");
        let err = f
            .call(json!({
                "path": "index.html",
                "template": "index.template.html",
                "parts": { "A": "index.html", "B": "parts/b.js" }
            }))
            .unwrap_err();
        assert!(err.contains("不能是输出文件本身"), "{err}");
        let _ = std::fs::remove_dir_all(f.dir.path());
    }

    #[tokio::test]
    async fn unbound_scope_is_rejected() {
        let f = fixture("unbound");
        let ctx = ToolContext {
            session_id: "task:t1".into(),
            character_id: String::new(),
            agent_depth: 0,
            scope: None,
            budget: None,
        };
        let err = run_inline(
            &f.deps,
            &ctx,
            &json!({ "path": "index.html", "sources": ["parts/a.js"] }),
        )
        .unwrap_err();
        assert!(err.contains("未绑定工作区"), "{err}");
        let _ = std::fs::remove_dir_all(f.dir.path());
    }
}
