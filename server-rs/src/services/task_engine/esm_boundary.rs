// 单文件交付的 ESM/模块边界判定(SKY-10,2026-10-26)。
//
// 产品边界:单文件交付**不建立本地模块图**。
// - 引擎内联的分片(sources / 预置骨架)必须是经典脚本;ESM 分片被跳过并点名;
// - 最终入口可以含 `<script type="module">`,但 import 只能指向远程 URL 或
//   importmap 映射到远程;不得引用/映射工作区文件。
//
// 本模块只做文本判定:不执行 JS、不解析完整语法、不联网。四类确定性判定:
// 1) 经典 <script> 里的行首 import/export(浏览器整块语法错误);
// 2) importmap 目标指向本地文件(imports / scopes 的值);
// 3) module 的裸说明符未被 importmap 映射(含前缀映射);
// 4) module 导入 /... 本地绝对路径。
use crate::services::task_engine::wiring;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

/// 单次判定的问题条数上限(与 wiring 同口径)。
const MAX_ISSUES: usize = 20;

/// 行首 import/export 启发式(与 fs_inline_html 的 sources 拒绝共用)。
///
/// 先去掉 JS 注释再按行首判定,避免 `// export ...` 误报;字符串里以 `export`
/// 开头的行仍可能误报——这是文本启发式的已知边界,不引入完整解析器。
pub(crate) fn has_top_level_esm(text: &str) -> bool {
    let cleaned = wiring::strip_js_comments(text);
    cleaned.lines().any(|line| {
        let line = line.trim_start();
        line.starts_with("import ")
            || line.starts_with("import{")
            || line.starts_with("import.")
            || line.starts_with("export ")
            || line.starts_with("export{")
            || line.starts_with("export*")
    })
}

/// 对**尚未落盘**的单文件 HTML 做完整自包含检查。
pub(crate) fn single_file_text_issues(rel: &str, html: &str) -> Vec<String> {
    let mut issues = wiring::single_file_text_issues(rel, html);
    issues.extend(boundary_issues(rel, html));
    issues.sort();
    issues.dedup();
    issues.truncate(MAX_ISSUES);
    issues
}

/// 对已落盘的单文件交付做完整静态检查(接线 + ESM 边界)。
pub(crate) fn delivery_issues(workspace: &Path, declared: &[String], goal: &str) -> Vec<String> {
    let mut issues = wiring::wiring_issues(workspace, declared, goal);
    if wiring::single_file_requested(goal) {
        for rel in declared {
            let lower = rel.to_ascii_lowercase();
            if !(lower.ends_with(".html") || lower.ends_with(".htm")) {
                continue;
            }
            let Some(text) = wiring::read_text(&workspace.join(rel)) else {
                continue;
            };
            issues.extend(boundary_issues(rel, &text));
        }
    }
    issues.sort();
    issues.dedup();
    issues.truncate(MAX_ISSUES);
    issues
}

/// 四类 ESM/模块边界判定。
fn boundary_issues(rel: &str, html: &str) -> Vec<String> {
    let cleaned = wiring::strip_html_comments(html);
    let blocks = script_blocks(&cleaned);
    let mut issues: Vec<String> = Vec::new();
    let mut maps: Vec<ImportMap> = Vec::new();
    // importmap 必须先于 module 解析(HTML 里也要求先声明),所以先收集全部映射。
    for block in &blocks {
        if block.src.is_some() || block.kind != ScriptKind::Importmap {
            continue;
        }
        if let Some(map) = parse_importmap(&block.body, rel, &mut issues) {
            maps.push(map);
        }
    }
    for block in &blocks {
        if block.src.is_some() {
            continue;
        }
        match block.kind {
            ScriptKind::Classic => {
                if has_top_level_esm(&block.body) {
                    issues.push(format!(
                        "{rel}:经典脚本块含 import/export(浏览器会按语法错误丢弃整块);单文件分片必须是经典脚本(window.*),或把该代码移入 <script type=\"module\">"
                    ));
                }
            }
            ScriptKind::Module => check_module_imports(rel, &block.body, &maps, &mut issues),
            ScriptKind::Importmap | ScriptKind::Other => {}
        }
    }
    issues.sort();
    issues.dedup();
    issues.truncate(MAX_ISSUES);
    issues
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScriptKind {
    Classic,
    Module,
    Importmap,
    Other,
}

struct ScriptBlock {
    kind: ScriptKind,
    src: Option<String>,
    body: String,
}

/// 提取所有内联 `<script>` 块及其类型;带 `src` 的外链脚本保留但由调用方跳过。
fn script_blocks(html: &str) -> Vec<ScriptBlock> {
    let lower = html.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < html.len() {
        let Some(rel_open) = lower[pos..].find("<script") else {
            break;
        };
        let open = pos + rel_open;
        let Some(rel_end) = lower[open..].find('>') else {
            break;
        };
        let tag_end = open + rel_end;
        let Some(rel_close) = lower[tag_end + 1..].find("</script>") else {
            break;
        };
        let close = tag_end + 1 + rel_close;
        let attrs = &lower[open..tag_end];
        out.push(ScriptBlock {
            kind: classify_script(attrs),
            src: wiring::attr_value(attrs, "src"),
            body: html[tag_end + 1..close].to_string(),
        });
        pos = close + "</script>".len();
    }
    out
}

fn classify_script(attrs: &str) -> ScriptKind {
    let Some(kind) = wiring::attr_value(attrs, "type") else {
        return ScriptKind::Classic;
    };
    let kind = kind.trim().to_ascii_lowercase();
    if kind.is_empty() {
        return ScriptKind::Classic;
    }
    if kind == "module" {
        return ScriptKind::Module;
    }
    if kind == "importmap" {
        return ScriptKind::Importmap;
    }
    // text/javascript / application/javascript 等按经典脚本执行;其余(如 text/plain)
    // 浏览器不执行,不参与判定。
    if kind.contains("javascript") || kind == "ecmascript" {
        return ScriptKind::Classic;
    }
    ScriptKind::Other
}

struct ImportMap {
    imports: BTreeMap<String, Option<String>>,
    has_scopes: bool,
}

fn parse_importmap(body: &str, rel: &str, issues: &mut Vec<String>) -> Option<ImportMap> {
    let value: Value = match serde_json::from_str(body.trim()) {
        Ok(value) => value,
        Err(error) => {
            issues.push(format!("{rel}:importmap 不是合法 JSON:{error}"));
            return None;
        }
    };
    let mut imports: BTreeMap<String, Option<String>> = BTreeMap::new();
    if let Some(obj) = value.get("imports").and_then(Value::as_object) {
        for (key, target) in obj {
            match target {
                Value::String(text) => {
                    if wiring::is_local_ref(text) {
                        issues.push(format!(
                            "{rel}:importmap 把 \"{key}\" 映射到本地文件 {text};单文件不得建立本地模块图"
                        ));
                    }
                    imports.insert(key.clone(), Some(text.clone()));
                }
                // null = 显式屏蔽;按未映射处理(引用时由裸说明符判定点名)。
                Value::Null => {
                    imports.insert(key.clone(), None);
                }
                _ => issues.push(format!(
                    "{rel}:importmap 的 imports.{key} 必须是字符串或 null"
                )),
            }
        }
    }
    let mut has_scopes = false;
    if let Some(scopes) = value.get("scopes").and_then(Value::as_object) {
        has_scopes = !scopes.is_empty();
        for (scope, map) in scopes {
            let Some(map) = map.as_object() else {
                continue;
            };
            for (key, target) in map {
                if let Value::String(text) = target {
                    if wiring::is_local_ref(text) {
                        issues.push(format!(
                            "{rel}:importmap 的 scopes[{scope}] 把 \"{key}\" 映射到本地文件 {text};单文件不得建立本地模块图"
                        ));
                    }
                }
            }
        }
    }
    Some(ImportMap {
        imports,
        has_scopes,
    })
}

fn check_module_imports(rel: &str, body: &str, maps: &[ImportMap], issues: &mut Vec<String>) {
    // scopes 的匹配依赖引用方 URL,文本层不猜;本地目标已由 parse_importmap 单独拦下。
    let has_scopes = maps.iter().any(|map| map.has_scopes);
    for import in wiring::parse_imports(body) {
        let spec = import.specifier.trim();
        if spec.is_empty() || wiring::is_relative_spec(spec) {
            // 相对导入由 wiring 的既有单文件判定负责,避免重复报。
            continue;
        }
        if spec.starts_with('/') {
            issues.push(format!(
                "{rel}:module 脚本导入了本地绝对路径 {spec};单文件不得建立本地模块图"
            ));
            continue;
        }
        if is_remote_spec(spec) || has_scopes {
            continue;
        }
        match mapped_target(maps, spec) {
            Some(Some(_)) => {}
            Some(None) => issues.push(format!(
                "{rel}:裸模块说明符 \"{spec}\" 被 importmap 屏蔽(值为 null);单文件无法解析"
            )),
            None => issues.push(format!(
                "{rel}:裸模块说明符 \"{spec}\" 未在 importmap 中映射;单文件只能引用远程 URL 或 importmap→远程"
            )),
        }
    }
}

fn mapped_target<'a>(maps: &'a [ImportMap], spec: &str) -> Option<&'a Option<String>> {
    let mut found = None;
    for map in maps {
        if let Some(target) = exact_or_prefix(&map.imports, spec) {
            found = Some(target);
        }
    }
    found
}

fn exact_or_prefix<'a>(
    imports: &'a BTreeMap<String, Option<String>>,
    spec: &str,
) -> Option<&'a Option<String>> {
    if let Some(target) = imports.get(spec) {
        return Some(target);
    }
    let mut best: Option<&'a Option<String>> = None;
    let mut best_len = 0;
    for (key, target) in imports {
        if key.ends_with('/') && spec.starts_with(key.as_str()) && key.len() > best_len {
            best = Some(target);
            best_len = key.len();
        }
    }
    best
}

fn is_remote_spec(spec: &str) -> bool {
    spec.starts_with("http://")
        || spec.starts_with("https://")
        || spec.starts_with("//")
        || spec.starts_with("data:")
        || spec.starts_with("blob:")
        || spec.starts_with('#')
        || has_scheme(spec)
}

fn has_scheme(spec: &str) -> bool {
    let Some((scheme, _)) = spec.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_alphabetic()
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issues(html: &str) -> Vec<String> {
        boundary_issues("index.html", html)
    }

    #[test]
    fn has_top_level_esm_detects_forms_and_ignores_comments() {
        assert!(has_top_level_esm("export const A = 1;"));
        assert!(has_top_level_esm("  import * as THREE from 'three';"));
        assert!(has_top_level_esm("export default function f() {}"));
        assert!(has_top_level_esm("export{A};"));
        assert!(!has_top_level_esm("// export const A = 1;"));
        assert!(!has_top_level_esm("const s = 'export const A = 1;';"));
        assert!(!has_top_level_esm("window.A = 1;"));
    }

    #[test]
    fn classic_script_export_is_flagged() {
        let out = issues(r#"<html><script>export const A = 1;</script></html>"#);
        assert!(
            out.iter()
                .any(|i| i.contains("经典脚本") && i.contains("import/export")),
            "{out:?}"
        );
    }

    #[test]
    fn module_script_export_is_not_flagged() {
        let out = issues(r#"<html><script type="module">export const A = 1;</script></html>"#);
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn commented_out_export_in_classic_script_is_not_flagged() {
        let out = issues("<html><script>// export const A = 1;\nconst A = 1;</script></html>");
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn local_importmap_target_is_flagged() {
        let out = issues(
            r#"<html><script type="importmap">{"imports":{"a":"./parts/a.js"}}</script><script type="module">import "a";</script></html>"#,
        );
        assert!(
            out.iter()
                .any(|i| i.contains("importmap") && i.contains("./parts/a.js")),
            "{out:?}"
        );
    }

    #[test]
    fn remote_importmap_and_prefix_mapping_are_clean() {
        let out = issues(
            r#"<html><script type="importmap">{"imports":{"three":"https://cdn.example/three.module.js","three/":"https://cdn.example/examples/jsm/"}}</script><script type="module">import * as THREE from "three"; import "three/controls/OrbitControls.js";</script></html>"#,
        );
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn unmapped_bare_specifier_is_flagged() {
        let out = issues(
            r#"<html><script type="module">import * as THREE from "three";</script></html>"#,
        );
        assert!(
            out.iter()
                .any(|i| i.contains("裸模块说明符") && i.contains("three")),
            "{out:?}"
        );
    }

    #[test]
    fn local_absolute_module_path_is_flagged() {
        let out = issues(
            r#"<html><script type="module">import { A } from "/parts/a.js";</script></html>"#,
        );
        assert!(out.iter().any(|i| i.contains("本地绝对路径")), "{out:?}");
    }

    #[test]
    fn malformed_importmap_is_flagged() {
        let out = issues(r#"<html><script type="importmap">{"imports":</script></html>"#);
        assert!(
            out.iter()
                .any(|i| i.contains("importmap") && i.contains("JSON")),
            "{out:?}"
        );
    }

    #[test]
    fn data_url_module_import_is_clean() {
        let out = issues(
            r#"<html><script type="module">import "data:text/javascript,export default 1";</script></html>"#,
        );
        assert!(out.is_empty(), "{out:?}");
    }
}
