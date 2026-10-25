// 交付物的静态接线检查(SKY-1 确定性部分,2026-10-24)。
//
// 为什么需要:2026-10-23 空岛 team 复跑里,总装入口写 `import { buildSubIslands }`,
// 而上游 `part-subislands.js` 只导出 `generateSubIslands/createSubIslands`;模型与
// 审计都靠自由文本发现,直到终审才拦下。同轮 `index.html` 仍是引用 `./src/*.js` 的
// 中间稿,而目标明确要求单文件交付——这两类问题都能在文件落盘后用静态规则判定,
// 不该消耗一整轮审计/补做。
//
// 本模块只做**只读静态检查**:
// - ES module 静态 `import ... from "./x"` 的名字是否真被目标文件导出;
// - 本地相对模块/资源是否存在;
// - 目标要求单文件交付时,声明 HTML 是否仍引用工作区本地资源或相对导入。
//
// 边界:不执行 JS、不解析完整语法、不联网;识别不了的导出形态(CommonJS、动态导出
// 等)宁可跳过名字比对,也不制造假失败。检查项进入 `delivery.content_issues`,
// 由终态门槛降 partial 并在成果卡逐项展示。
use crate::models::ownership::normalize_workspace_rel;
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

/// 单次扫描的文件数上限(声明文件 + 递归相对导入;防止异常工作区拖慢收尾)。
const MAX_SCAN_FILES: usize = 64;
/// 单文件读取上限(与占位符检查同口径)。
const MAX_SCAN_BYTES: u64 = 512 * 1024;
/// 返回的问题条数上限(错误文案与前端展示都不宜无限长)。
const MAX_ISSUES: usize = 20;

const JS_EXTENSIONS: &[&str] = &["js", "mjs", "cjs", "ts", "tsx", "jsx"];
const TEXT_EXTENSIONS: &[&str] = &[
    "html", "htm", "js", "mjs", "cjs", "ts", "tsx", "jsx", "css", "json", "svg", "md", "txt", "xml",
];

/// 目标文本是否要求「单文件交付」(决定本地外部引用是否算失败)。
///
/// 只在**正向 cue** 出现且前文 8 个字符内没有否定词时成立;避免把
/// 「不要单文件」这类反向要求误判成单文件验收。cue 使用原文与 ASCII 小写双查,
/// 大小写不敏感(`Single-File`)。
pub(crate) fn single_file_requested(goal: &str) -> bool {
    let lower = goal.to_ascii_lowercase();
    let cues = [
        "单文件",
        "single-file",
        "single file",
        "唯一交付物",
        "一个 html 文件",
        "一个html文件",
        "只交付一个 html",
        "只交付一个html",
    ];
    for cue in cues {
        let mut search_from = 0;
        while let Some(pos) = lower[search_from..].find(cue) {
            let abs = search_from + pos;
            let prefix = &lower[..abs];
            let window: String = prefix
                .chars()
                .rev()
                .take(8)
                .collect::<String>()
                .chars()
                .rev()
                .collect();
            if !contains_negation(&window) {
                return true;
            }
            search_from = abs + cue.len();
        }
    }
    false
}

fn contains_negation(window: &str) -> bool {
    ["不要", "不用", "无需", "不是", "禁止", "避免", "非单文件"]
        .iter()
        .any(|w| window.contains(w))
}

/// 从文本里提取对外接口名(SKY-1;供上游摘要与静态接线检查共用)。
///
/// 支持本项目实际出现的形态:
/// - ES module:`export const/let/var/function/class/async function`(含逗号多声明)、
///   `export {a, b as c}`、`export default`、`export * from`(以 `*` 标记,名字比对跳过)、
///   `export type/interface/enum`;
/// - 经典脚本:`window.X =` / `global.X =`(历史产物 `window.MCWorldMain` 形态)。
///
/// 只取**名字**不取签名:签名仍要求总装主 fs_read 确认(参数顺序无法靠正则可靠提取);
/// 名字清单足以拦住「导入一个根本不存在的导出」这类实测硬错配。
/// 读失败/非文本返回空;结果去重排序并截断到 24 个(展示口径)。
pub(crate) fn extract_exports(text: &str) -> Vec<String> {
    let mut out = extract_exports_all(text);
    out.truncate(24);
    out
}

/// 完整导出名集合(静态接线检查用;不截断——大模块的合法导入不能因展示上限被误判)。
pub(crate) fn extract_exports_all(text: &str) -> Vec<String> {
    let cleaned = strip_js_comments(text);
    let mut out: Vec<String> = Vec::new();
    for raw in cleaned.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        for prefix in ["window.", "global."] {
            if let Some(rest) = line.strip_prefix(prefix) {
                let name = take_ident(rest);
                if !name.is_empty() {
                    out.push(name);
                }
                break;
            }
        }
        for name in export_names_of_line(line) {
            out.push(name);
        }
    }
    out.sort();
    out.dedup();
    out
}

/// 读文件并提取导出名;非 JS/TS、超限、读失败返回空。
pub(crate) fn file_exports(path: &Path) -> Vec<String> {
    let Some(text) = read_text(path) else {
        return Vec::new();
    };
    if !is_js_path(path) {
        return Vec::new();
    }
    extract_exports(&text)
}

/// 静态接线检查主入口。
///
/// `declared` 是 `TaskStep.files` 的声明路径(已归一化);扫描范围包含这些文件及
/// 它们通过相对 `import`/`<script src>`/`<link href>` 引用到的本地文件(最多
/// [MAX_SCAN_FILES] 个)。工作区不可访问时返回空(与交付摘要同口径,不制造假失败)。
pub(crate) fn wiring_issues(workspace: &Path, declared: &[String], goal: &str) -> Vec<String> {
    if !workspace.is_dir() {
        return Vec::new();
    }
    let single_file = single_file_requested(goal);
    let mut issues: Vec<String> = Vec::new();
    let mut scanned: HashSet<PathBuf> = HashSet::new();
    let mut queue: Vec<(String, PathBuf)> = Vec::new();
    // 全局命名检查要跨文件比对「生产者/消费者」,先攒文本再统一判定。
    // 上限 8MB:异常工作区不能把收尾内存打满(声明文件本身已限 512KB/个)。
    let mut texts: Vec<(String, String)> = Vec::new();
    let mut text_bytes = 0usize;
    for raw in declared {
        let Some(rel) = normalize_workspace_rel(raw) else {
            continue;
        };
        let abs = normalize_path(&workspace.join(&rel));
        if !path_within(workspace, &abs) || !abs.is_file() {
            continue;
        }
        queue.push((rel, abs));
    }
    while let Some((rel, abs)) = queue.pop() {
        if scanned.len() >= MAX_SCAN_FILES {
            break;
        }
        if !scanned.insert(abs.clone()) {
            continue;
        }
        let Some(text) = read_text(&abs) else {
            continue;
        };
        if text_bytes < 8 * 1024 * 1024 {
            text_bytes += text.len();
            texts.push((rel.clone(), text.clone()));
        }
        if is_html_path(&abs) {
            check_html(
                workspace,
                &rel,
                &abs,
                &text,
                single_file,
                &mut issues,
                &mut queue,
            );
        } else if is_js_path(&abs) {
            check_js(workspace, &rel, &abs, &text, &mut issues, &mut queue);
        }
    }
    issues.extend(global_wiring_issues(&texts));
    issues.sort();
    issues.dedup();
    issues.truncate(MAX_ISSUES);
    issues
}

/// 已知的 CDN/运行时全局:经典脚本模式下由外部 `<script src>` 提供,不在工作区声明,
/// 不能因为「没导出」就判失败。
const AMBIENT_GLOBALS: &[&str] = &[
    "THREE",
    "React",
    "ReactDOM",
    "Vue",
    "gsap",
    "TWEEN",
    "Matter",
    "p5",
    "d3",
    "Chart",
    "ace",
    "CodeMirror",
    "monaco",
    "anime",
    "lottie",
    "Howl",
    "Tone",
    "CANNON",
    "AMMO",
    "GUI",
    "dat",
    "Stats",
    "OrbitControls",
    "MapControls",
    "PointerLockControls",
    "TransformControls",
    "GLTFLoader",
    "FBXLoader",
    "OBJLoader",
    "MTLLoader",
    "RGBELoader",
    "DRACOLoader",
    "EffectComposer",
    "RenderPass",
    "ShaderPass",
    "UnrealBloomPass",
    "Sky",
    "Water",
];

/// 浏览器内置的 `window.*` 属性(读取它们不是应用全局)。
const BUILTIN_WINDOW_PROPS: &[&str] = &[
    "location",
    "navigator",
    "document",
    "console",
    "history",
    "screen",
    "performance",
    "crypto",
    "localStorage",
    "sessionStorage",
    "innerWidth",
    "innerHeight",
    "outerWidth",
    "outerHeight",
    "devicePixelRatio",
    "requestAnimationFrame",
    "cancelAnimationFrame",
    "setTimeout",
    "clearTimeout",
    "setInterval",
    "clearInterval",
    "addEventListener",
    "removeEventListener",
    "dispatchEvent",
    "matchMedia",
    "getComputedStyle",
    "alert",
    "confirm",
    "prompt",
    "open",
    "close",
    "focus",
    "blur",
    "scrollTo",
    "scrollBy",
    "fetch",
    "atob",
    "btoa",
    "URL",
    "Blob",
    "File",
    "FileReader",
    "Image",
    "Audio",
    "FormData",
    "XMLHttpRequest",
    "WebSocket",
    "Worker",
    "CustomEvent",
    "Event",
    "HTMLElement",
    "Element",
    "Node",
    "Promise",
    "Math",
    "JSON",
    "Date",
    "Object",
    "Array",
    "String",
    "Number",
    "Boolean",
    "RegExp",
    "Error",
    "Map",
    "Set",
    "WeakMap",
    "WeakSet",
    "Symbol",
    "Intl",
    "BigInt",
    "Reflect",
    "Proxy",
    "Uint8Array",
    "Float32Array",
    "ArrayBuffer",
    "DataView",
    "TextEncoder",
    "TextDecoder",
    "WebGLRenderingContext",
    "WebGL2RenderingContext",
    "ImageData",
    "Path2D",
    "OffscreenCanvas",
];

struct GlobalPatterns {
    producer: regex::Regex,
    producer_bracket: regex::Regex,
    require: regex::Regex,
    read: regex::Regex,
}

/// 惰性编译一次;编译失败(不应发生)返回 None,让调用方跳过而不 panic。
fn global_patterns() -> Option<&'static GlobalPatterns> {
    use std::sync::OnceLock;
    static RE: OnceLock<Option<GlobalPatterns>> = OnceLock::new();
    RE.get_or_init(|| {
        Some(GlobalPatterns {
            producer: regex::Regex::new(
                r"(?:window|global|globalThis)\s*\.\s*([A-Za-z_$][A-Za-z0-9_$]*)\s*=(=)?",
            )
            .ok()?,
            producer_bracket: regex::Regex::new(
                r#"(?:window|global|globalThis)\s*\[\s*["']([A-Za-z_$][A-Za-z0-9_$]*)["']\s*\]\s*=(=)?"#,
            )
            .ok()?,
            require: regex::Regex::new(
                r#"requireGlobal\s*\(\s*["']([A-Za-z_$][A-Za-z0-9_$]*)["']\s*\)"#,
            )
            .ok()?,
            read: regex::Regex::new(
                r"(?:window|global|globalThis)\s*\.\s*([A-Za-z_$][A-Za-z0-9_$]*)",
            )
            .ok()?,
        })
    })
    .as_ref()
}

/// 全局命名接线检查(SKY-1,2026-10-25)。
///
/// 空岛复跑暴露的形态:入口 `requireGlobal('BlockEngine')`,而上游只导出
/// `window.MC_BLOCK_ENGINE`;这类错配能静态判定,不该等审计自由文本或运行期空场景。
///
/// 判定只认高信号形态,避免把 CDN/内置全局误判:
/// - `requireGlobal('X')` 是模型自己写的查找助手,返回 null 即启动失败——X 未导出就报;
/// - `window.X` 读取在 X 未导出、但与某个已导出名字**规范化后相近**时报警(命名不一致);
/// - 注释里的示例、`window.THREE` 等外部全局、浏览器内置属性不算。
///
/// 返回项带文件相对路径,进 `delivery.content_issues` 后由终态门槛降 partial。
pub(crate) fn global_wiring_issues(texts: &[(String, String)]) -> Vec<String> {
    let Some(pat) = global_patterns() else {
        return Vec::new();
    };
    let mut produced: HashSet<String> = HashSet::new();
    let mut cleaned: Vec<(&str, String)> = Vec::new();
    for (rel, text) in texts {
        let lower = rel.to_ascii_lowercase();
        if lower.ends_with(".html") || lower.ends_with(".htm") {
            let html = strip_html_comments(text);
            for body in html_inline_scripts(&html) {
                cleaned.push((rel.as_str(), strip_js_comments(&body)));
            }
        } else if is_js_rel(rel) {
            cleaned.push((rel.as_str(), strip_js_comments(text)));
        }
    }
    for (_, text) in &cleaned {
        for caps in pat.producer.captures_iter(text) {
            if caps.get(2).is_some() {
                continue; // `==` 比较,不是赋值
            }
            produced.insert(caps[1].to_string());
        }
        for caps in pat.producer_bracket.captures_iter(text) {
            if caps.get(2).is_some() {
                continue;
            }
            produced.insert(caps[1].to_string());
        }
    }
    let ambient =
        |name: &str| AMBIENT_GLOBALS.contains(&name) || BUILTIN_WINDOW_PROPS.contains(&name);
    let mut issues: Vec<String> = Vec::new();
    for (rel, text) in &cleaned {
        for caps in pat.require.captures_iter(text) {
            let name = &caps[1];
            if produced.contains(name) || ambient(name) {
                continue;
            }
            if let Some(near) = produced
                .iter()
                .filter(|p| fuzzy_global_match(name, p))
                .min_by_key(|p| p.len())
            {
                issues.push(format!(
                    "{rel}:requireGlobal('{name}') 找不到全局对象;工作区导出的相近名称是 {near}(全局命名不一致)"
                ));
            } else {
                issues.push(format!(
                    "{rel}:requireGlobal('{name}') 找不到全局对象(工作区未导出该全局)"
                ));
            }
        }
        for caps in pat.read.captures_iter(text) {
            let whole = caps.get(0).map(|m| m.end()).unwrap_or(text.len());
            let after = text[whole..].trim_start();
            if after.starts_with('=') {
                continue; // 赋值/比较,不是读取
            }
            let name = &caps[1];
            if produced.contains(name) || ambient(name) {
                continue;
            }
            if let Some(near) = produced
                .iter()
                .filter(|p| fuzzy_global_match(name, p))
                .min_by_key(|p| p.len())
            {
                issues.push(format!(
                    "{rel}:读取 window.{name},但工作区只导出了 {near}(全局命名不一致)"
                ));
            }
        }
    }
    issues.sort();
    issues.dedup();
    issues.truncate(MAX_ISSUES);
    issues
}

fn is_js_rel(rel: &str) -> bool {
    let lower = rel.to_ascii_lowercase();
    JS_EXTENSIONS
        .iter()
        .any(|ext| lower.ends_with(&format!(".{ext}")))
}

/// 规范化全局名:只保留 ASCII 字母数字并小写;导出名允许去掉 `MC` 前缀。
/// 包含匹配只在两侧都不短于 6 个字符时启用,避免 `UI`/`GameUI` 这类噪声;
/// 去前缀后的**完全相等**不受长度限制(`MC_APP` 与 `App` 是典型的命名不一致)。
fn fuzzy_global_match(consumer: &str, produced: &str) -> bool {
    let c: String = consumer
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .map(|ch| ch.to_ascii_lowercase())
        .collect();
    let p: String = produced
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .map(|ch| ch.to_ascii_lowercase())
        .collect();
    if c == p {
        return true;
    }
    if let Some(stripped) = p.strip_prefix("mc") {
        if stripped == c {
            return true;
        }
    }
    c.len() >= 6 && p.len() >= 6 && (p.contains(&c) || c.contains(&p))
}

/// 提取 HTML 里所有**内联** `<script>` 正文(带 `src` 的外链脚本不算)。
fn html_inline_scripts(text: &str) -> Vec<String> {
    let lower = text.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(rel) = lower[pos..].find("<script") {
        let open = pos + rel;
        let Some(tag_end_rel) = lower[open..].find('>') else {
            break;
        };
        let tag_end = open + tag_end_rel;
        let attrs = &lower[open..tag_end];
        let Some(close) = find_case_insensitive(&lower, tag_end + 1, "</script>") else {
            break;
        };
        let has_src = attr_value(attrs, "src").is_some();
        if !has_src {
            out.push(text[tag_end + 1..close].to_string());
        }
        pos = close + "</script>".len();
    }
    out
}

/// 对**尚未落盘**的单文件 HTML 文本做自包含检查(SKY-8,2026-10-24)。
///
/// 与 [`wiring_issues`] 的单文件分支同口径:HTML 不得引用工作区本地资源
/// (`script[src]`/`link[href]`/`img[src]`/`source[src]`),内联 module 不得相对
/// `import` 本地模块;CDN / `data:` / `blob:` / `#` 放行。`fs_inline_html` 在写入口
/// 之前调用本函数——校验失败不落盘,避免把非自包含半成品交给交付门槛。
///
/// 只做文本判定,不解析完整 HTML/JS 语法;返回项形如
/// `index.html:单文件交付不得引用本地外部资源 ./parts/a.js`。
pub(crate) fn single_file_text_issues(rel: &str, html: &str) -> Vec<String> {
    let cleaned = strip_html_comments(html);
    let mut issues: Vec<String> = Vec::new();
    for block in html_inline_modules(&cleaned) {
        for import in parse_imports(&block) {
            if is_relative_spec(&import.specifier) {
                issues.push(format!(
                    "{rel}:单文件交付不得在脚本内导入本地模块 {}",
                    import.specifier
                ));
            }
        }
    }
    for reference in html_local_refs(&cleaned) {
        issues.push(format!("{rel}:单文件交付不得引用本地外部资源 {reference}"));
    }
    issues.extend(global_wiring_issues(&[(rel.to_string(), html.to_string())]));
    issues.sort();
    issues.dedup();
    issues.truncate(MAX_ISSUES);
    issues
}

fn check_js(
    workspace: &Path,
    rel: &str,
    abs: &Path,
    text: &str,
    issues: &mut Vec<String>,
    queue: &mut Vec<(String, PathBuf)>,
) {
    for import in parse_imports(text) {
        if !is_relative_spec(&import.specifier) {
            continue;
        }
        let Some(target) = resolve_module(workspace, abs, &import.specifier) else {
            issues.push(format!("{rel}:引用了不存在的模块 {}", import.specifier));
            continue;
        };
        if is_js_path(&target) {
            queue.push((rel_of(workspace, &target), target.clone()));
        }
        check_imported_names(rel, &import, &target, issues);
    }
}

fn check_html(
    workspace: &Path,
    rel: &str,
    abs: &Path,
    text: &str,
    single_file: bool,
    issues: &mut Vec<String>,
    queue: &mut Vec<(String, PathBuf)>,
) {
    let cleaned = strip_html_comments(text);
    for block in html_inline_modules(&cleaned) {
        for import in parse_imports(&block) {
            if !is_relative_spec(&import.specifier) {
                continue;
            }
            if single_file {
                issues.push(format!(
                    "{rel}:单文件交付不得在脚本内导入本地模块 {}",
                    import.specifier
                ));
                continue;
            }
            let Some(target) = resolve_module(workspace, abs, &import.specifier) else {
                issues.push(format!(
                    "{rel}:内联脚本引用了不存在的模块 {}",
                    import.specifier
                ));
                continue;
            };
            if is_js_path(&target) {
                queue.push((rel_of(workspace, &target), target.clone()));
            }
            check_imported_names(rel, &import, &target, issues);
        }
    }
    for reference in html_local_refs(&cleaned) {
        if single_file {
            issues.push(format!("{rel}:单文件交付不得引用本地外部资源 {reference}"));
            continue;
        }
        let Some(target) = resolve_ref(workspace, abs, &reference) else {
            issues.push(format!("{rel}:引用的本地资源 {reference} 不存在"));
            continue;
        };
        if is_js_path(&target) {
            queue.push((rel_of(workspace, &target), target.clone()));
        }
    }
}

fn check_imported_names(rel: &str, import: &ImportRef, target: &Path, issues: &mut Vec<String>) {
    if import.type_only || target.extension().is_none() {
        return;
    }
    if !is_js_path(target) {
        return;
    }
    if import.named.is_empty() && !import.has_default {
        return;
    }
    let Some(text) = read_text(target) else {
        return;
    };
    let exports = extract_exports_all(&text);
    // 识别不了的导出形态(CommonJS / 动态导出):宁可跳过,不制造假失败。
    if exports.is_empty() || exports.iter().any(|e| e == "*") {
        return;
    }
    if import.has_default && !exports.iter().any(|e| e == "default") {
        issues.push(format!(
            "{rel}:从 {} 导入 default,但目标文件没有 default 导出",
            import.specifier
        ));
    }
    for name in &import.named {
        if !exports.iter().any(|e| e == name) {
            issues.push(format!(
                "{rel}:从 {} 导入的 {name} 未被目标文件导出",
                import.specifier
            ));
        }
    }
}

/// 一条静态导入描述(只保留判定所需字段)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ImportRef {
    pub specifier: String,
    pub named: Vec<String>,
    pub has_default: bool,
    pub has_namespace: bool,
    pub type_only: bool,
}

/// 提取静态 import / `export ... from` 语句。
///
/// 支持跨行 `import {\n a,\n b\n} from "./x"`;动态 `import()` 与注释里的伪导入
/// 不参与判定。`has_namespace` 仅用于语义完整,名字比对对命名空间导入自然跳过。
pub(crate) fn parse_imports(text: &str) -> Vec<ImportRef> {
    let cleaned = strip_js_comments(text);
    let lines: Vec<&str> = cleaned.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim_start();
        if !is_import_start(trimmed) && !is_export_from_start(trimmed) {
            i += 1;
            continue;
        }
        let mut stmt = String::new();
        let mut j = i;
        while j < lines.len() && j < i + 20 {
            stmt.push_str(lines[j]);
            stmt.push('\n');
            let parsed = if is_import_start(trimmed) {
                parse_import_statement(&stmt)
            } else {
                parse_export_from_statement(&stmt)
            };
            if parsed.is_some() {
                if let Some(import) = parsed {
                    out.push(import);
                }
                break;
            }
            if lines[j].trim_end().ends_with(';') {
                break;
            }
            j += 1;
        }
        i = j + 1;
    }
    out
}

fn is_import_start(line: &str) -> bool {
    line.starts_with("import ")
        || line.starts_with("import{")
        || line.starts_with("import\"")
        || line.starts_with("import'")
        || line == "import"
}

fn is_export_from_start(line: &str) -> bool {
    line.starts_with("export ")
        && (line.contains(" from ") || line.contains(" from\"") || line.contains(" from'"))
}

fn parse_import_statement(stmt: &str) -> Option<ImportRef> {
    let mut rest = stmt.trim_start().strip_prefix("import")?;
    rest = rest.trim_start();
    if rest.is_empty() || rest.starts_with('(') {
        return None;
    }
    if let Some(after) = rest.strip_prefix('"') {
        return Some(ImportRef {
            specifier: take_quoted_body(after, '"'),
            named: Vec::new(),
            has_default: false,
            has_namespace: false,
            type_only: false,
        });
    }
    if let Some(after) = rest.strip_prefix('\'') {
        return Some(ImportRef {
            specifier: take_quoted_body(after, '\''),
            named: Vec::new(),
            has_default: false,
            has_namespace: false,
            type_only: false,
        });
    }
    let type_only = rest.starts_with("type ");
    if type_only {
        rest = rest["type ".len()..].trim_start();
    }
    let (head, specifier) = find_from_spec(rest)?;
    let head = head.trim();
    let named = named_imports(head);
    let has_namespace = head.contains('*') && head.contains(" as ");
    // `import Foo, { Bar }` / `import Foo from` / `import * as ns`
    let before_brace = head.split('{').next().unwrap_or(head).trim();
    let has_default = !has_namespace
        && !before_brace.is_empty()
        && before_brace.chars().next().is_some_and(is_ident_start);
    Some(ImportRef {
        specifier,
        named,
        has_default,
        has_namespace,
        type_only,
    })
}

fn parse_export_from_statement(stmt: &str) -> Option<ImportRef> {
    let rest = stmt.trim_start().strip_prefix("export")?;
    let mut rest = rest.trim_start();
    let type_only = rest.starts_with("type ");
    if type_only {
        rest = rest["type ".len()..].trim_start();
    }
    let (head, specifier) = find_from_spec(rest)?;
    let head = head.trim();
    let named = if head.starts_with('*') {
        Vec::new()
    } else if let Some(lo) = head.find('{') {
        let hi = head.rfind('}').unwrap_or(head.len());
        parse_name_list(&head[lo + 1..hi])
    } else {
        Vec::new()
    };
    Some(ImportRef {
        specifier,
        named,
        has_default: false,
        has_namespace: head.starts_with('*'),
        type_only,
    })
}

/// 去掉语句里的 `import` 前缀后,定位 `from` 后的模块说明符。
fn find_from_spec(rest: &str) -> Option<(&str, String)> {
    for (idx, _) in rest.match_indices("from") {
        let before = rest[..idx].chars().last();
        let after = rest[idx + 4..].trim_start();
        let quote = after.chars().next()?;
        if quote != '"' && quote != '\'' {
            continue;
        }
        // `from` 必须是独立 token(前一个字符是空白/逗号/`}`)或行首
        if before.is_some_and(|c| !c.is_whitespace() && c != ',' && c != '}') {
            continue;
        }
        let body = take_quoted_body(&after[1..], quote);
        if body.is_empty() {
            continue;
        }
        return Some((&rest[..idx], body));
    }
    None
}

fn named_imports(head: &str) -> Vec<String> {
    let Some(lo) = head.find('{') else {
        return Vec::new();
    };
    let Some(hi) = head.rfind('}') else {
        return Vec::new();
    };
    if lo >= hi {
        return Vec::new();
    }
    parse_name_list(&head[lo + 1..hi])
}

fn parse_name_list(list: &str) -> Vec<String> {
    let mut out = Vec::new();
    for part in split_top_level(list, ',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let part = part.strip_prefix("type ").unwrap_or(part).trim();
        // `a as b` 取对外名 b;`a` 原样
        let name = part.split_whitespace().last().unwrap_or(part);
        if !name.is_empty() {
            out.push(name.to_string());
        }
    }
    out
}

fn export_names_of_line(line: &str) -> Vec<String> {
    let Some(rest) = line.strip_prefix("export ") else {
        return Vec::new();
    };
    let mut rest = rest.trim_start();
    if let Some(after) = rest.strip_prefix("type ") {
        rest = after.trim_start();
    }
    if rest.starts_with('*') {
        return vec!["*".to_string()];
    }
    if let Some(after) = rest.strip_prefix("default") {
        if after.is_empty() || after.chars().next().is_some_and(|c| !is_ident_char(c)) {
            return vec!["default".to_string()];
        }
    }
    let after_async = rest.strip_prefix("async ").unwrap_or(rest);
    for kw in [
        "function ",
        "const ",
        "let ",
        "var ",
        "class ",
        "interface ",
        "enum ",
    ] {
        if let Some(after) = after_async.strip_prefix(kw) {
            if matches!(kw, "const " | "let " | "var ") {
                return declarator_names(after);
            }
            let name = take_ident(after);
            return if name.is_empty() {
                Vec::new()
            } else {
                vec![name]
            };
        }
    }
    if let Some(after) = rest.strip_prefix('{') {
        if let Some(end) = after.find('}') {
            return parse_name_list(&after[..end]);
        }
    }
    Vec::new()
}

/// `export const a = 1, b = 2` / `export const { a, b } = obj` 的声明名。
fn declarator_names(after: &str) -> Vec<String> {
    let mut out = Vec::new();
    for part in split_top_level(after, ',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if part.starts_with('{') || part.starts_with('[') {
            let close = if part.starts_with('{') { '}' } else { ']' };
            if let Some(end) = part.find(close) {
                for name in split_top_level(&part[1..end], ',') {
                    let name = name.trim();
                    let name = name.split_whitespace().last().unwrap_or(name);
                    let name = name.trim_start_matches("...").trim();
                    if !name.is_empty() && is_ident_start(name.chars().next().unwrap_or(' ')) {
                        out.push(name.to_string());
                    }
                }
            }
            continue;
        }
        let name = take_ident(part);
        if !name.is_empty() {
            out.push(name);
        }
    }
    out
}

/// 按顶层分隔符切分(忽略括号/方括号/花括号与字符串内的分隔符)。
fn split_top_level(input: &str, sep: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for c in input.chars() {
        if let Some(q) = quote {
            buf.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' | '`' => {
                quote = Some(c);
                buf.push(c);
            }
            '(' | '[' | '{' => {
                depth += 1;
                buf.push(c);
            }
            ')' | ']' | '}' => {
                depth = (depth - 1).max(0);
                buf.push(c);
            }
            _ if c == sep && depth == 0 => {
                out.push(buf.trim().to_string());
                buf.clear();
            }
            _ => buf.push(c),
        }
    }
    if !buf.trim().is_empty() {
        out.push(buf.trim().to_string());
    }
    out
}

fn take_ident(input: &str) -> String {
    input
        .chars()
        .take_while(|c| is_ident_char(*c))
        .collect::<String>()
}

fn is_ident_start(c: char) -> bool {
    c == '_' || c == '$' || c.is_alphabetic()
}

fn is_ident_char(c: char) -> bool {
    c == '_' || c == '$' || c.is_alphanumeric()
}

/// 去掉 JS 注释,保留换行(避免把注释里的示例 import 误判为真实引用)。
fn strip_js_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    while let Some(c) = chars.next() {
        if let Some(q) = quote {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' | '`' => {
                quote = Some(c);
                out.push(c);
            }
            '/' if chars.peek() == Some(&'/') => {
                chars.next();
                for n in chars.by_ref() {
                    if n == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut prev = '\0';
                for n in chars.by_ref() {
                    if n == '\n' {
                        out.push('\n');
                    }
                    if prev == '*' && n == '/' {
                        break;
                    }
                    prev = n;
                }
            }
            _ => out.push(c),
        }
    }
    out
}

/// 去掉 HTML 注释(避免注释里的 `<script src>` 被当成真实引用)。
fn strip_html_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 4..];
        if let Some(end) = after.find("-->") {
            rest = &after[end + 3..];
        } else {
            return out;
        }
    }
    out.push_str(rest);
    out
}

/// 提取 `<script type="module">` 的内联正文(不含带 `src` 的外链脚本)。
pub(crate) fn html_inline_modules(text: &str) -> Vec<String> {
    let lower = text.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(rel) = lower[pos..].find("<script") {
        let open = pos + rel;
        let Some(tag_end_rel) = lower[open..].find('>') else {
            break;
        };
        let tag_end = open + tag_end_rel;
        let attrs = &lower[open..tag_end];
        let close = find_case_insensitive(&lower, tag_end + 1, "</script>");
        let Some(close) = close else {
            break;
        };
        let is_module = attr_value(attrs, "type").is_some_and(|v| v.trim() == "module");
        let has_src = attr_value(attrs, "src").is_some();
        if is_module && !has_src {
            out.push(text[tag_end + 1..close].to_string());
        }
        pos = close + "</script>".len();
    }
    out
}

/// 提取 HTML 里指向本地文件的 `script[src]` / `link[href]` / `img[src]`。
pub(crate) fn html_local_refs(text: &str) -> Vec<String> {
    let lower = text.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(rel) = lower[pos..].find('<') {
        let open = pos + rel;
        let Some(tag_end_rel) = lower[open..].find('>') else {
            break;
        };
        let tag_end = open + tag_end_rel;
        let tag = &lower[open..tag_end];
        let name = tag
            .trim_start_matches('<')
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim_end_matches('/');
        let value = match name {
            "script" => attr_value(tag, "src"),
            "link" => attr_value(tag, "href"),
            "img" => attr_value(tag, "src"),
            "source" => attr_value(tag, "src"),
            _ => None,
        };
        if let Some(v) = value {
            let v = v.trim();
            if is_local_ref(v) && !out.iter().any(|x: &String| x == v) {
                out.push(v.to_string());
            }
        }
        pos = tag_end + 1;
    }
    out
}

fn attr_value(tag: &str, attr: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut search = 0;
    while let Some(pos) = lower[search..].find(attr) {
        let abs = search + pos;
        let before = lower[..abs].chars().last();
        let after = lower[abs + attr.len()..].trim_start();
        let boundary_ok = before.is_none_or(|c| c.is_whitespace());
        if boundary_ok {
            if let Some(rest) = after.strip_prefix('=') {
                let rest = rest.trim_start();
                let quote = rest.chars().next()?;
                if quote == '"' || quote == '\'' {
                    let body = take_quoted_body(&rest[1..], quote);
                    return Some(body);
                }
                let end = rest
                    .find(|c: char| c.is_whitespace() || c == '>')
                    .unwrap_or(rest.len());
                return Some(rest[..end].to_string());
            }
        }
        search = abs + attr.len();
    }
    None
}

fn find_case_insensitive(haystack: &str, from: usize, needle: &str) -> Option<usize> {
    if from > haystack.len() {
        return None;
    }
    haystack[from..].find(needle).map(|p| from + p)
}

fn is_local_ref(value: &str) -> bool {
    let v = value.trim();
    if v.is_empty()
        || v.starts_with("http://")
        || v.starts_with("https://")
        || v.starts_with("//")
        || v.starts_with("data:")
        || v.starts_with("blob:")
        || v.starts_with('#')
        || v.starts_with("javascript:")
    {
        return false;
    }
    true
}

fn is_relative_spec(spec: &str) -> bool {
    spec.starts_with("./") || spec.starts_with("../")
}

fn is_js_path(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str(),
        "js" | "mjs" | "cjs" | "ts" | "tsx" | "jsx"
    )
}

fn is_html_path(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str(),
        "html" | "htm"
    )
}

fn read_text(path: &Path) -> Option<String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !TEXT_EXTENSIONS.contains(&ext.as_str()) {
        return None;
    }
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_SCAN_BYTES {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

fn resolve_module(workspace: &Path, from_file: &Path, spec: &str) -> Option<PathBuf> {
    if !is_relative_spec(spec) {
        return None;
    }
    // 构建工具常见的 `?v=1` / `#hash` 只影响请求,不影响磁盘文件。
    let spec = spec.split(['?', '#']).next().unwrap_or(spec);
    let base = normalize_path(&from_file.parent()?.join(spec));
    if !path_within(workspace, &base) {
        return None;
    }
    let mut candidates = Vec::new();
    if base.extension().is_some() {
        candidates.push(base.clone());
    } else {
        candidates.push(base.clone());
        for ext in JS_EXTENSIONS {
            candidates.push(base.with_extension(ext));
        }
        candidates.push(base.with_extension("json"));
        for ext in JS_EXTENSIONS {
            candidates.push(base.join(format!("index.{ext}")));
        }
    }
    candidates.into_iter().find(|p| p.is_file())
}

fn resolve_ref(workspace: &Path, from_file: &Path, reference: &str) -> Option<PathBuf> {
    let trimmed = reference.trim();
    // 根相对路径(`/parts/a.js`)按工作区根解析;`./`/`../`/裸路径按引用文件所在目录。
    let root_relative = trimmed.starts_with('/') && !trimmed.starts_with("//");
    let raw = trimmed
        .trim_start_matches('/')
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .replace('\\', "/");
    if raw.is_empty() {
        return None;
    }
    let base = if root_relative {
        normalize_path(&workspace.join(&raw))
    } else {
        normalize_path(&from_file.parent()?.join(&raw))
    };
    let base = if path_within(workspace, &base) {
        base
    } else {
        normalize_path(&workspace.join(&raw))
    };
    if !path_within(workspace, &base) {
        return None;
    }
    if base.is_file() {
        return Some(base);
    }
    if base.extension().is_none() {
        for ext in ["html", "htm", "js", "mjs", "css", "json"] {
            let candidate = base.with_extension(ext);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn rel_of(workspace: &Path, path: &Path) -> String {
    path.strip_prefix(workspace)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"))
}

fn path_within(workspace: &Path, path: &Path) -> bool {
    let ws = normalize_path(workspace);
    let p = normalize_path(path);
    p.starts_with(&ws)
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn take_quoted_body(after_quote: &str, quote: char) -> String {
    let mut out = String::new();
    let mut escaped = false;
    for c in after_quote.chars() {
        if escaped {
            out.push(c);
            escaped = false;
            continue;
        }
        if c == '\\' {
            escaped = true;
            continue;
        }
        if c == quote {
            break;
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_workspace(tag: &str) -> PathBuf {
        let ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("系统时间应晚于 epoch")
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("kedai-wiring-{tag}-{}-{ns}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("创建临时工作区失败");
        dir
    }

    #[test]
    fn single_file_requested_handles_positive_and_negated_phrases() {
        assert!(single_file_requested(
            "交付一个可直接双击运行的单文件 3D 交互场景 index.html"
        ));
        assert!(single_file_requested(
            "Final delivery: one Single-File HTML"
        ));
        assert!(single_file_requested("最终只交付一个 HTML 文件"));
        assert!(!single_file_requested("拆成多个文件维护,不要单文件交付"));
        assert!(!single_file_requested("普通多文件项目"));
    }

    #[test]
    fn extract_exports_handles_multi_declarators_and_export_star() {
        let text = "export const a = 1, b = 2;\nexport const { c, d: e } = obj;\n\
                    export function f() {}\nexport * from './x.js';\nexport default init;";
        let got = extract_exports(text);
        for name in ["a", "b", "c", "e", "f", "*", "default"] {
            assert!(
                got.iter().any(|g| g == name),
                "应提取导出名「{name}」: {got:?}"
            );
        }
    }

    #[test]
    fn commented_out_imports_are_ignored() {
        let text = "// import { missing } from './a.js'\n/* import { alsoMissing } from './b.js' */\nconst x = 1;";
        assert!(parse_imports(text).is_empty(), "注释里的伪导入不得参与判定");
    }

    #[test]
    fn named_import_mismatch_is_reported() {
        let ws = temp_workspace("mismatch");
        std::fs::create_dir_all(ws.join("parts")).unwrap();
        std::fs::write(
            ws.join("parts/a.js"),
            "export function generateSubIslands() {}\nexport const createSubIslands = 1;\n",
        )
        .unwrap();
        std::fs::write(
            ws.join("main.js"),
            "import { buildSubIslands } from './parts/a.js';\nbuildSubIslands();\n",
        )
        .unwrap();
        let issues = wiring_issues(&ws, &["main.js".into(), "parts/a.js".into()], "");
        assert!(
            issues.iter().any(|i| i.contains("main.js")
                && i.contains("buildSubIslands")
                && i.contains("未被目标文件导出")),
            "应点名未导出的导入名: {issues:?}"
        );
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn missing_module_and_asset_are_reported() {
        let ws = temp_workspace("missing");
        std::fs::write(
            ws.join("main.js"),
            "import { x } from './parts/missing.js';\nexport const y = 1;\n",
        )
        .unwrap();
        std::fs::write(
            ws.join("index.html"),
            "<html><script src=\"./missing.js\"></script></html>",
        )
        .unwrap();
        let issues = wiring_issues(&ws, &["main.js".into(), "index.html".into()], "");
        assert!(
            issues
                .iter()
                .any(|i| i.contains("main.js") && i.contains("不存在的模块")),
            "应报告缺失模块: {issues:?}"
        );
        assert!(
            issues
                .iter()
                .any(|i| i.contains("index.html") && i.contains("不存在")),
            "应报告缺失的本地资源: {issues:?}"
        );
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn healthy_wiring_and_export_star_are_not_flagged() {
        let ws = temp_workspace("healthy");
        std::fs::write(ws.join("a.js"), "export const A = 1;\n").unwrap();
        std::fs::write(ws.join("b.js"), "export * from './a.js';\n").unwrap();
        std::fs::write(
            ws.join("main.js"),
            "import { A } from './a.js';\nimport * as all from './b.js';\nimport './side.js';\n",
        )
        .unwrap();
        std::fs::write(ws.join("side.js"), "console.log(1);\n").unwrap();
        let issues = wiring_issues(
            &ws,
            &[
                "main.js".into(),
                "a.js".into(),
                "b.js".into(),
                "side.js".into(),
            ],
            "",
        );
        assert!(issues.is_empty(), "健康接线不得误伤: {issues:?}");
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn html_inline_module_imports_are_checked() {
        let ws = temp_workspace("html-inline");
        std::fs::write(ws.join("a.js"), "export const A = 1;\n").unwrap();
        std::fs::write(
            ws.join("index.html"),
            r#"<html><script type="module">import { B } from './a.js';</script></html>"#,
        )
        .unwrap();
        let issues = wiring_issues(&ws, &["index.html".into(), "a.js".into()], "");
        assert!(
            issues.iter().any(|i| i.contains("index.html")
                && i.contains("B")
                && i.contains("未被目标文件导出")),
            "HTML 内联脚本的导入也必须核对: {issues:?}"
        );
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn single_file_flags_local_refs_and_relative_imports() {
        let ws = temp_workspace("single");
        std::fs::write(ws.join("a.js"), "export const A = 1;\n").unwrap();
        std::fs::write(
            ws.join("index.html"),
            r#"<html><link rel="stylesheet" href="./a.css" />
<script src="./a.js"></script>
<script type="module">import { A } from './a.js'; console.log(A);</script></html>"#,
        )
        .unwrap();
        std::fs::write(ws.join("a.css"), "body{}\n").unwrap();
        let issues = wiring_issues(
            &ws,
            &["index.html".into(), "a.js".into(), "a.css".into()],
            "最终只交付一个 HTML 文件",
        );
        assert!(
            issues
                .iter()
                .any(|i| i.contains("不得引用本地外部资源 ./a.js")),
            "单文件交付不得外链本地脚本: {issues:?}"
        );
        assert!(
            issues
                .iter()
                .any(|i| i.contains("不得在脚本内导入本地模块 ./a.js")),
            "单文件交付不得相对导入本地模块: {issues:?}"
        );
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn single_file_allows_cdn_and_data_urls() {
        let ws = temp_workspace("single-cdn");
        std::fs::write(
            ws.join("index.html"),
            r#"<html><script type="importmap">{"imports":{"three":"https://cdn/x.js"}}</script>
<script type="module">import * as THREE from "https://cdn/x.js";</script>
<img src="data:image/png;base64,AA==" /></html>"#,
        )
        .unwrap();
        let issues = wiring_issues(&ws, &["index.html".into()], "单文件交付");
        assert!(
            issues.is_empty(),
            "CDN 与 data URL 不算本地外链: {issues:?}"
        );
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn default_import_requires_default_export() {
        let ws = temp_workspace("default");
        std::fs::write(ws.join("a.js"), "export const A = 1;\n").unwrap();
        std::fs::write(ws.join("main.js"), "import init from './a.js';\n").unwrap();
        let issues = wiring_issues(&ws, &["main.js".into(), "a.js".into()], "");
        assert!(
            issues.iter().any(|i| i.contains("default")),
            "缺少 default 导出应被报告: {issues:?}"
        );
        let _ = std::fs::remove_dir_all(&ws);
    }

    /// 展示口径会把导出名截断到 24 个;接线判定必须用完整集合,否则大模块的合法
    /// 导入（第 25 个之后）会被误判成「未导出」。
    #[test]
    fn large_module_import_is_not_truncated_for_wiring() {
        let ws = temp_workspace("large-module");
        let exports: String = (0..40)
            .map(|i| format!("export const N{i} = {i};\n"))
            .collect();
        std::fs::write(ws.join("a.js"), exports).unwrap();
        std::fs::write(ws.join("main.js"), "import { N39 } from './a.js';\n").unwrap();
        let issues = wiring_issues(&ws, &["main.js".into(), "a.js".into()], "");
        assert!(
            issues.is_empty(),
            "大模块的合法导入不得被展示截断误伤: {issues:?}"
        );
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn unresolved_exports_are_skipped_instead_of_guessed() {
        // 目标文件导出形态识别不了(CommonJS):不制造假失败。
        let ws = temp_workspace("cjs");
        std::fs::write(ws.join("a.js"), "module.exports = { A: 1 };\n").unwrap();
        std::fs::write(ws.join("main.js"), "import { A } from './a.js';\n").unwrap();
        let issues = wiring_issues(&ws, &["main.js".into(), "a.js".into()], "");
        assert!(
            issues.is_empty(),
            "识别不了的导出形态应跳过名字比对: {issues:?}"
        );
        let _ = std::fs::remove_dir_all(&ws);
    }

    /// SKY-8:落盘前的单文件文本检查——本地外链/相对 import 拦截,CDN/data 放行。
    #[test]
    fn single_file_text_issues_rejects_local_refs_and_relative_imports() {
        let clean = "<html><script>window.A=1;</script><script src=\"https://cdn.example/x.js\"></script></html>";
        assert!(single_file_text_issues("index.html", clean).is_empty());

        let local_ref = "<html><script src=\"./parts/a.js\"></script></html>";
        let issues = single_file_text_issues("index.html", local_ref);
        assert!(
            issues
                .iter()
                .any(|i| i.contains("单文件交付不得引用本地外部资源")),
            "{issues:?}"
        );

        let relative_import =
            "<html><script type=\"module\">import { A } from './a.js';</script></html>";
        let issues = single_file_text_issues("index.html", relative_import);
        assert!(
            issues
                .iter()
                .any(|i| i.contains("单文件交付不得在脚本内导入本地模块")),
            "{issues:?}"
        );

        let commented =
            "<html><!-- <script src=\"./a.js\"></script> --><script>window.A=1;</script></html>";
        assert!(
            single_file_text_issues("index.html", commented).is_empty(),
            "HTML 注释里的外链不得误伤"
        );
    }

    fn texts(items: &[(&str, &str)]) -> Vec<(String, String)> {
        items
            .iter()
            .map(|(rel, text)| (rel.to_string(), text.to_string()))
            .collect()
    }

    /// SKY-1:入口 `requireGlobal('BlockEngine')` vs 上游只导出 `MC_BLOCK_ENGINE`
    /// ——静态判定必须点名两侧名字(不靠审计自由文本)。
    #[test]
    fn global_wiring_flags_renamed_global_lookup() {
        let issues = global_wiring_issues(&texts(&[
            (
                "parts/main.js",
                "var BlockEngine = requireGlobal('BlockEngine');",
            ),
            (
                "parts/block-engine.js",
                "global.MC_BLOCK_ENGINE = API;\nglobal.MC = global.MC || {};",
            ),
        ]));
        assert!(
            issues
                .iter()
                .any(|i| i.contains("requireGlobal('BlockEngine')")
                    && i.contains("MC_BLOCK_ENGINE")
                    && i.contains("命名不一致")),
            "{issues:?}"
        );
    }

    /// 生产者/消费者同名(即使分处两文件)不得误报。
    #[test]
    fn global_wiring_accepts_matching_global() {
        let issues = global_wiring_issues(&texts(&[
            (
                "index.html",
                "<script>window.BlockEngine = Engine;</script>",
            ),
            (
                "parts/main.js",
                "var e = requireGlobal('BlockEngine'); e.addBlock();",
            ),
        ]));
        assert!(issues.is_empty(), "{issues:?}");
    }

    /// 完全没导出 → 报「工作区未导出该全局」(与命名不一致区分)。
    #[test]
    fn global_wiring_flags_missing_global() {
        let issues = global_wiring_issues(&texts(&[(
            "parts/main.js",
            "var m = requireGlobal('MissingThing');",
        )]));
        assert!(
            issues
                .iter()
                .any(|i| i.contains("MissingThing") && i.contains("未导出该全局")),
            "{issues:?}"
        );
    }

    /// CDN 全局(THREE)与浏览器内置属性不算工作区导出缺失。
    #[test]
    fn global_wiring_ignores_ambient_and_builtin_globals() {
        let issues = global_wiring_issues(&texts(&[(
            "parts/main.js",
            "var t = requireGlobal('THREE');\nwindow.innerWidth; window.location.href; window.THREE;",
        )]));
        assert!(issues.is_empty(), "{issues:?}");
    }

    /// 注释里的示例与 `==` 比较不得被当成生产者/消费者。
    #[test]
    fn global_wiring_ignores_comments_and_comparisons() {
        let issues = global_wiring_issues(&texts(&[(
            "parts/main.js",
            "// window.Ghost = 1;\nif (window.Ghost == null) { requireGlobal('Ghost'); }",
        )]));
        assert!(
            issues
                .iter()
                .any(|i| i.contains("Ghost") && i.contains("未导出该全局")),
            "比较不是赋值,仍应报未导出: {issues:?}"
        );
        assert!(
            !issues.iter().any(|i| i.contains("注释")),
            "不得出现注释噪声: {issues:?}"
        );
    }

    /// `window.X` 读取与已导出名字相近但仍不同 → 报命名不一致。
    #[test]
    fn global_wiring_flags_read_name_mismatch() {
        let issues = global_wiring_issues(&texts(&[
            ("parts/main.js", "var e = window.BlockEngine;"),
            ("parts/engine.js", "global.MC_BLOCK_ENGINE = API;"),
        ]));
        assert!(
            issues
                .iter()
                .any(|i| i.contains("读取 window.BlockEngine") && i.contains("MC_BLOCK_ENGINE")),
            "{issues:?}"
        );
    }

    /// HTML 正文里的撇号不能破坏脚本扫描;内联脚本里的全局仍然参与判定。
    #[test]
    fn global_wiring_scans_inline_scripts_of_html() {
        let issues = global_wiring_issues(&texts(&[(
            "index.html",
            "<html><body><p>don't panic</p><script>var a = requireGlobal('Absent');</script></body></html>",
        )]));
        assert!(issues.iter().any(|i| i.contains("Absent")), "{issues:?}");
    }

    /// 落盘前的单文件检查也消费全局命名规则(模型手写内联时提前反馈)。
    #[test]
    fn single_file_text_issues_include_global_naming() {
        let html =
            "<html><script>global.MC_APP = {};\nvar a = requireGlobal('App');</script></html>";
        let issues = single_file_text_issues("index.html", html);
        assert!(
            issues
                .iter()
                .any(|i| i.contains("requireGlobal('App')") && i.contains("MC_APP")),
            "{issues:?}"
        );
    }
}
