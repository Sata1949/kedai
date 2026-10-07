// 后端插件:自定义工具加载器
// 约定:data/plugins/tools/*.json 定义工具,格式:
// {
//   "name": "weather",                    // 工具名(唯一)
//   "description": "查询天气",
//   "parameters": { "type": "object", ... },   // JSON Schema(OpenAI 兼容)
//   "script": "city = args.city\nresult = city ? \"查询 \" + city + \" 的天气\" : \"缺少 city 参数\"",
// }
// script 执行器为受控指令求值(2026-10-07 求值器升级):语句序列(中间变量)+ 多级成员链 +
// 下标 [n] + 算术与字符串拼接 + 白名单函数,不使用 eval。**语法形态错误在加载期**由
// `validate_script` 拦截(不再等模型调用才失败);求值期错误(未知成员/越界/类型不符)
// 显式报错回传,不再静默返回空(唯一例外:args 直接属性缺失 → 空串,兼容既有写法)。
//
// ## 代际边界(L3 青层·活;2026-09-14 依赖倒置)
//
// 本模块是**纯解析器 + 白名单求值器**:它把 JSON 文件解析成工具定义、把脚本求值成字符串,
// **不持有工具注册表**。注册动作由宿主(组合根 `api/app_state.rs` / `api/plugins.rs`)完成。
//
// 为什么这样切分(三结合「隔离」判据):L3 不得依赖 L2,而工具注册表是 L2 骨干设施。
// 若本模块直接 `registry.register_external(...)`,就构成 `L3→L2` 越代依赖。
// 倒置后本模块只依赖 L1(`models::types::ToolDefinition`),注册由组合根装配
// ——这正是「青层能力经显式接缝注入」的标准形态(参照 `task_core::TaskBackend` 先例)。
use crate::models::types::ToolDefinition;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// 工具插件定义(从 JSON 文件加载)
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ToolPluginConfig {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub parameters: Value,
    #[serde(default)]
    pub script: String,
}

/// 已解析待注册的工具插件。
///
/// 宿主拿到它之后自行调用注册表登记(执行器由 [`plugin_executor`] 构造);
/// 本模块不参与注册,故不依赖任何 L2 设施。
#[derive(Debug, Clone)]
pub struct LoadedToolPlugin {
    /// 可直接交给工具注册表的定义(name/description/parameters)
    pub definition: ToolDefinition,
    /// 原始脚本正文(交给 [`plugin_executor`] 构造执行器)
    pub script: String,
    /// 来源文件基名(供 UI「按文件分组」展示与错误定位;非完整路径)
    pub file: String,
}

pub struct ToolPluginLoader {
    dir: PathBuf,
}

impl ToolPluginLoader {
    pub fn new(dir: PathBuf) -> Self {
        ToolPluginLoader { dir }
    }

    /// 解析目录下全部工具插件(**不注册**);返回 (已解析列表, 失败列表)
    ///
    /// 宿主负责把返回的定义逐条注册进工具注册表——注册是 L2 装配动作,不属 L3 加载器职责。
    pub fn parse_all(&self) -> (Vec<LoadedToolPlugin>, Vec<String>) {
        let mut loaded = Vec::new();
        let mut errors = Vec::new();
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return (loaded, errors);
        };
        let mut files: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().map(|e| e == "json").unwrap_or(false))
            .collect();
        files.sort();
        for file in files {
            match self.parse_file(&file) {
                Ok(plugin) => loaded.push(plugin),
                Err(e) => errors.push(format!(
                    "{}: {e}",
                    file.file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default()
                )),
            }
        }
        (loaded, errors)
    }

    /// 解析单个工具插件文件(**不注册**;供导入 API 复用)
    pub fn parse_file(&self, file: &Path) -> Result<LoadedToolPlugin, String> {
        let raw = std::fs::read_to_string(file).map_err(|e| format!("读取失败: {e}"))?;
        let cfg: ToolPluginConfig =
            serde_json::from_str(&raw).map_err(|e| format!("JSON 解析失败: {e}"))?;
        if cfg.name.trim().is_empty() {
            return Err("name 不能为空".into());
        }
        if cfg.script.trim().is_empty() {
            return Err("script 不能为空".into());
        }
        // 名称格式校验（2026-09-14 安全加固，见 known-limitations L19）：
        // ① 只允许小写字母/数字/下划线，且首字符为字母——防注入怪异字符与不可见字符；
        // ② **不得占用保留前缀** `mcp_`（MCP 工具命名空间）与 `agent` 系内置域，
        //    否则插件可在授权裁决的「未知外部工具」语义上伪装成已知工具族。
        //    跨命名空间的名称伪造比单纯重名更隐蔽（重名由 register_plugins 拦），
        //    故在解析期就拒绝。
        validate_plugin_name(&cfg.name)?;
        // 加载期脚本语法校验(PLGM 1.2):语法形态错误在这里就报,不再等模型调用才失败
        validate_script(&cfg.script)?;
        Ok(LoadedToolPlugin {
            definition: ToolDefinition {
                name: cfg.name,
                description: cfg.description,
                parameters: cfg.parameters,
            },
            script: cfg.script,
            file: file
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default(),
        })
    }

    /// 列出已加载工具插件文件(供 API 展示)
    pub fn list_files(&self) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.ends_with(".json"))
            .collect();
        names.sort();
        names
    }
}

/// 把已解析的插件构造成可直接注册的执行器（**纯构造，无 L2 依赖**）。
///
/// 宿主拿到返回值后自行调 `registrar.register_external(def, exec, None, ToolOrigin::Plugin)`。
/// 之所以把「构造」与「注册」分开，是为了让本模块（L3）不依赖工具注册表（L2）——
/// 这正是依赖倒置的落点。
pub fn plugin_executor(script: &str) -> crate::models::types::ToolExecutor {
    let script = script.to_string();
    std::sync::Arc::new(
        move |args: Value, _ctx| -> futures::future::BoxFuture<'static, Result<String, String>> {
            let script = script.clone();
            Box::pin(async move { eval_tool_script(&script, args) })
        },
    )
}

/// 插件工具名格式校验(2026-09-14;安全加固,见 `docs/遗留.md` L19)。
///
/// 规则:
/// - 非空、`^[a-z][a-z0-9_]*$`、长度 ≤ 48;
/// - 不得以保留前缀开头:`mcp_`(MCP 命名空间)、`agent`(内置 agent 工具域)。
///
/// 拒绝的理由不是「不好看」,而是**命名空间伪造**:授权裁决按工具名判定风险与来源,
/// 一个叫 `mcp_fs_read` 的插件会被误认为 MCP 工具。跨命名空间的伪造比重名更隐蔽,
/// 故在解析期即拒(重名另有 `register_plugins` 的内置名校验兜底)。
fn validate_plugin_name(name: &str) -> Result<(), String> {
    const MAX_LEN: usize = 48;
    const RESERVED_PREFIXES: &[&str] = &["mcp_", "agent"];
    if name.len() > MAX_LEN {
        return Err(format!("name 过长({} > {MAX_LEN})", name.len()));
    }
    let mut chars = name.chars();
    let first = chars.next().ok_or("name 不能为空")?;
    if !first.is_ascii_lowercase() {
        return Err("name 首字符须为小写字母".into());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return Err("name 只允许小写字母/数字/下划线".into());
    }
    for p in RESERVED_PREFIXES {
        if name.starts_with(p) {
            return Err(format!("name 不得使用保留前缀 `{p}`(防命名空间伪造)"));
        }
    }
    Ok(())
}

/// 白名单指令求值入口(2026-10-07 求值器 v2):语句序列,分隔符为顶层 `;` 或换行。
/// - 中间语句:`ident = 表达式`(中间变量,可多个);
/// - 末句:`return 表达式` 或 `result = 表达式`;
/// - 返回值为末句结果(字符串原样 / null → 空串 / 其余 JSON 文本)。
///
/// 语法形态错误在加载期由 [`validate_script`] 拦截;此处求值期错误(未知成员/越界/
/// 类型不符)一律显式 `Err`(唯一例外:args 直接属性缺失 → 空串,见 [`eval_chain_with_base`])。
fn eval_tool_script(script: &str, args: Value) -> Result<String, String> {
    let mut env: HashMap<String, Value> = HashMap::new();
    env.insert("args".to_string(), args);
    let stmts = split_statements(script)?;
    let Some((last, init)) = stmts.split_last() else {
        return Err("脚本为空:末句须为 `return <表达式>` 或 `result = <表达式>`".into());
    };
    for stmt in init {
        let (name, expr) = parse_assign(stmt)
            .ok_or_else(|| format!("语句须为 `变量 = 表达式` 赋值形式: {stmt}"))?;
        if name == "args" {
            return Err("不允许给 args 赋值(args 为入参命名空间)".into());
        }
        let value =
            eval_expr(expr, &env).map_err(|e| format!("求值失败(在语句 `{stmt}` 中): {e}"))?;
        env.insert(name, value);
    }
    let (_, expr) = split_terminator(last)?;
    let value = eval_expr(expr, &env).map_err(|e| format!("求值失败(在末句 `{last}` 中): {e}"))?;
    value_to_string(&value)
}

/// 顶层切分语句(分隔符 `;` 或换行;引号/括号内不切),并做词法配平检查。
/// 语法错误(引号未闭/括号不配平)在此处即报,可由加载期校验与求值期共用——
/// 同一份切分实现,不存在「校验器与求值器两套语法」的漂移。
fn split_statements(script: &str) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut cur = String::new();
    for c in script.chars() {
        if let Some(q) = quote {
            cur.push(c);
            if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' => {
                quote = Some(c);
                cur.push(c);
            }
            '(' | '[' | '{' => {
                depth += 1;
                cur.push(c);
            }
            ')' | ']' | '}' => {
                depth -= 1;
                cur.push(c);
            }
            ';' | '\n' if depth == 0 => {
                let t = cur.trim();
                if !t.is_empty() {
                    out.push(t.to_string());
                }
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    if let Some(q) = quote {
        return Err(format!("字符串引号未闭合(缺少与 {q} 配对的引号)"));
    }
    if depth != 0 {
        return Err("括号不配平(圆括号/方括号/花括号数量不匹配)".into());
    }
    let t = cur.trim();
    if !t.is_empty() {
        out.push(t.to_string());
    }
    Ok(out)
}

/// 解构赋值语句 `ident = 表达式`(仅认顶层单个 `=`;跳过 ==/>=/<=/!= 等比较形态)。
/// 非赋值形态返回 None,由调用方给出定位错误。
fn parse_assign(stmt: &str) -> Option<(String, &str)> {
    let t = stmt.trim();
    let bytes = t.as_bytes();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut i = 0usize;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        match c {
            '"' | '\'' => quote = Some(c),
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            '=' if depth == 0 => {
                let prev = if i == 0 { ' ' } else { bytes[i - 1] as char };
                let next = bytes.get(i + 1).copied().unwrap_or(b' ') as char;
                if next == '=' || matches!(prev, '!' | '<' | '>' | '=') {
                    // 比较形态(==/!=/<=/>=):不是赋值
                } else {
                    let lhs = t[..i].trim();
                    let rhs = t[i + 1..].trim();
                    if !rhs.is_empty() && is_bound_ident(lhs) {
                        return Some((lhs.to_string(), rhs));
                    }
                    return None;
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// 末句解构:`return <表达式>` / `return(<表达式>)` / `result = <表达式>`。
fn split_terminator(stmt: &str) -> Result<(&'static str, &str), String> {
    let t = stmt.trim();
    if let Some(rest) = t.strip_prefix("return") {
        // 边界:return 后须空白/左括号/直接结束;防 `returning_x = 1` 被误当作 return 语句
        let boundary = rest.is_empty() || rest.starts_with(|c: char| c.is_whitespace() || c == '(');
        if boundary {
            let expr = rest.trim();
            if expr.is_empty() {
                return Err("`return` 后缺表达式".into());
            }
            return Ok(("return", expr));
        }
    }
    match parse_assign(t) {
        Some((lhs, expr)) if lhs == "result" => Ok(("result", expr)),
        Some((lhs, _)) => Err(format!(
            "末句须为 `return ...` 或 `result = ...`,当前为赋值给 `{lhs}`"
        )),
        None => Err("末句须为 `return <表达式>` 或 `result = <表达式>`".into()),
    }
}

/// 赋值左值形态:小写字母/下划线开头,后接小写字母/数字/下划线。
fn is_bound_ident(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_lowercase() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// 读标识符的字节长度(字母/数字/下划线;数字不可作首字符)。
/// 供后缀链的变量原子与成员名读取(变量名经 [`is_bound_ident`] 另有更严口径)。
fn ident_len(s: &str) -> usize {
    let mut n = 0usize;
    for (i, c) in s.char_indices() {
        if c.is_ascii_digit() && i == 0 {
            break;
        }
        if c.is_ascii_alphanumeric() || c == '_' {
            n = i + c.len_utf8();
        } else {
            break;
        }
    }
    n
}

fn value_to_string(v: &Value) -> Result<String, String> {
    match v {
        Value::String(s) => Ok(s.clone()),
        Value::Null => Ok(String::new()),
        other => serde_json::to_string(other).map_err(|e| format!("序列化失败: {e}")),
    }
}

/// 表达式求值(白名单,递归下降 v2;2026-10-07 求值器升级)。
///
/// 支持:三元 / 括号分组 / 顶层算术(+ - * / %,`+` 对非数值对做字符串拼接)/
/// 字面量(字符串/数字/布尔/null)/ 对象与数组字面量 / 白名单函数调用 / 多级成员链与下标。
/// 未知成员、越界、类型不符一律显式报错;唯一例外:args 直接属性缺失 → 空串语义(Null)。
fn eval_expr(input: &str, env: &HashMap<String, Value>) -> Result<Value, String> {
    let t = input.trim();
    if t.is_empty() {
        return Err("空表达式".into());
    }
    // 1) 三元(条件里的比较/逻辑由 eval_truthy 处理)
    if let Some(q) = top_level_find(t, '?') {
        if let Some(c) = top_level_find(&t[q + 1..], ':') {
            let cond = eval_truthy(t[..q].trim(), env)?;
            let when_true = t[q + 1..q + 1 + c].trim();
            let when_false = t[q + 1 + c + 1..].trim();
            return if cond {
                eval_expr(when_true, env)
            } else {
                eval_expr(when_false, env)
            };
        }
    }
    // 2) 整串括号分组:(<子表达式>) → 内层继续(如 (a + b) * 2 的左半、return(x))
    if t.starts_with('(') {
        if let Some(close) = matching_close(t, 0) {
            if close == t.len() - 1 {
                return eval_expr(&t[1..t.len() - 1], env);
            }
        }
    }
    // 3) 顶层算术:按「最低优先级在前」扫描(同旧版口径)。
    //    首位 +/- 是数字字面量的符号(如 -1),不是二元运算符,跳过交给数字解析。
    //    `+` 语义(2026-10-07):两侧均为数值 → 算术;否则字符串拼接。
    for op in ["+", "-", "*", "/", "%"] {
        if let Some(pos) = top_level_op_find(t, op) {
            if pos == 0 {
                continue;
            }
            let left_raw = t[..pos].trim();
            let right_raw = t[pos + op.len()..].trim();
            if left_raw.is_empty() || right_raw.is_empty() {
                return Err(format!("运算符 `{op}` 两侧都要有表达式: {t}"));
            }
            let left = eval_expr(left_raw, env)?;
            let right = eval_expr(right_raw, env)?;
            return apply_binary(&left, &right, op);
        }
    }
    // 4) 字符串字面量(仅当整串恰为一个带引号 token;`"a" + "b"` 归第 3 步拼接)
    if let Some(s) = parse_string_literal(t) {
        return Ok(Value::String(s));
    }
    // 5) 数字 / 布尔 / null
    if let Ok(n) = t.parse::<i64>() {
        return Ok(Value::from(n));
    }
    if let Ok(f) = t.parse::<f64>() {
        return Ok(Value::from(f));
    }
    if t == "true" {
        return Ok(Value::Bool(true));
    }
    if t == "false" {
        return Ok(Value::Bool(false));
    }
    if t == "null" || t == "undefined" {
        return Ok(Value::Null);
    }
    // 6) JSON 对象字面量
    if t.starts_with('{') && t.ends_with('}') {
        let inner = &t[1..t.len() - 1];
        if inner.trim().is_empty() {
            return Ok(Value::Object(Default::default()));
        }
        return parse_object(inner, env);
    }
    // 7) JSON 数组字面量
    if t.starts_with('[') && t.ends_with(']') {
        let inner = &t[1..t.len() - 1];
        if inner.trim().is_empty() {
            return Ok(Value::Array(Vec::new()));
        }
        return parse_array(inner, env);
    }
    // 8) 全局函数调用(白名单):`名(参数)`;后接后缀链(如 `JSON.parse(x).length`)一并处理
    if let Some(open) = top_level_find(t, '(') {
        if open > 0 {
            let fn_name = t[..open].trim();
            if is_dotted_name(fn_name) {
                let close =
                    matching_close(t, open).ok_or_else(|| format!("函数调用括号不配平: {t}"))?;
                let value = eval_call(fn_name, &t[open + 1..close], env)?;
                return eval_chain_with_base(value, &t[close + 1..], false, t, env);
            }
        }
    }
    // 9) 后缀链:变量原子 / 括号组原子,后接多级 .成员 / .方法(参数) / [下标]
    if t.starts_with('(') {
        let close = matching_close(t, 0).ok_or_else(|| format!("括号不配平: {t}"))?;
        let base = eval_expr(&t[1..close], env)?;
        return eval_chain_with_base(base, &t[close + 1..], false, t, env);
    }
    let n = ident_len(t);
    if n > 0 {
        let name = &t[..n];
        let base = env
            .get(name)
            .cloned()
            .ok_or_else(|| format!("未知变量: {name}"))?;
        return eval_chain_with_base(base, &t[n..], name == "args", t, env);
    }
    Err(format!("无法求值表达式: {t}"))
}

/// 后缀链求值:在已有 `base` 值上连续消费 `.成员` / `.方法(参数)` / `[下标]` 段。
///
/// 缺失语义(2026-10-07 裁定 1):
/// - 链首为 `args` 且其**直接属性**缺失 → 返回 Null(输出/拼接期为空串),**整链短路**不报错
///   ——兼容既有「args.x 可能缺省」的写法;
/// - 嵌套对象缺键 / 下标越界 / 类型不符 → 显式 `Err`(带可用键或长度的定位信息)。
fn eval_chain_with_base(
    mut base: Value,
    mut rest: &str,
    first_is_args: bool,
    whole: &str,
    env: &HashMap<String, Value>,
) -> Result<Value, String> {
    let mut first_segment = true;
    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            break;
        }
        if let Some(after_dot) = rest.strip_prefix('.') {
            let mlen = ident_len(after_dot);
            if mlen == 0 {
                return Err(format!("成员访问缺名字(在 `{whole}` 中)"));
            }
            let member = &after_dot[..mlen];
            let after_member = after_dot[mlen..].trim_start();
            if after_member.starts_with('(') {
                // 方法调用:沿用白名单方法集(见 apply_method)
                let close = matching_close(after_member, 0)
                    .ok_or_else(|| format!("方法 `{member}` 参数括号不配平(在 `{whole}` 中)"))?;
                let args = parse_args(&after_member[1..close], env)?;
                base = apply_method(base, member, &args)?;
                rest = &after_member[close + 1..];
                first_segment = false;
                continue;
            }
            let value = match &base {
                Value::Object(o) => match o.get(member) {
                    Some(v) => v.clone(),
                    // 缺失一级参数 → 空串(整链短路)
                    None if first_is_args && first_segment => return Ok(Value::Null),
                    None => {
                        let mut keys: Vec<&str> = o.keys().map(|k| k.as_str()).collect();
                        keys.sort();
                        return Err(format!(
                            "成员 `{member}` 不存在(可用键: {})。表达式: {whole}",
                            if keys.is_empty() {
                                "无(空对象)".to_string()
                            } else {
                                keys.join(", ")
                            }
                        ));
                    }
                },
                Value::String(s) if member == "length" => Value::from(s.chars().count()),
                Value::Array(a) if member == "length" => Value::from(a.len()),
                other => {
                    return Err(format!(
                        "{}上无成员 `{member}`(对象支持 .键,字符串/数组支持 .length): {whole}",
                        type_name(other)
                    ));
                }
            };
            base = value;
            rest = after_member;
            first_segment = false;
            continue;
        }
        if rest.starts_with('[') {
            let close = matching_close(rest, 0)
                .ok_or_else(|| format!("下标方括号不配平(在 `{whole}` 中)"))?;
            let idx_raw = rest[1..close].trim();
            let idx_val = eval_expr(idx_raw, env)?;
            let idx_f = idx_val
                .as_f64()
                .filter(|n| n.fract() == 0.0 && *n >= 0.0)
                .ok_or_else(|| format!("下标须为非负整数,当前为 `{idx_val}`(在 `{whole}` 中)"))?;
            let idx = idx_f as usize;
            base = match base {
                Value::Array(arr) => arr.get(idx).cloned().ok_or_else(|| {
                    format!("下标 {idx} 越界(数组长度 {},在 `{whole}` 中)", arr.len())
                })?,
                other => {
                    return Err(format!(
                        "{}不支持下标访问(仅数组支持 [n]): {whole}",
                        type_name(&other)
                    ));
                }
            };
            rest = &rest[close + 1..];
            first_segment = false;
            continue;
        }
        break;
    }
    if !rest.trim().is_empty() {
        return Err(format!("表达式尾有多余内容 `{}`: {whole}", rest.trim()));
    }
    Ok(base)
}

/// 二元运算(2026-10-07):`+` 两侧均为数值 → 算术;否则字符串拼接(value_to_string 语义,
/// null → 空串)。其余运算符维持数值语义,非数值操作数显式报错。
fn apply_binary(left: &Value, right: &Value, op: &str) -> Result<Value, String> {
    let lf = left.as_f64();
    let rf = right.as_f64();
    if op == "+" {
        if let (Some(a), Some(b)) = (lf, rf) {
            return Ok(Value::from(a + b));
        }
        let a = value_to_string(left)?;
        let b = value_to_string(right)?;
        return Ok(Value::String(format!("{a}{b}")));
    }
    let (Some(a), Some(b)) = (lf, rf) else {
        return Err(format!(
            "运算符 `{op}` 需要两侧均为数值:左={}({left}) 右={}({right})",
            type_name(left),
            type_name(right)
        ));
    };
    match op {
        "-" => Ok(Value::from(a - b)),
        "*" => Ok(Value::from(a * b)),
        "/" => {
            if b == 0.0 {
                return Err("除零".into());
            }
            Ok(Value::from(a / b))
        }
        "%" => Ok(Value::from(a % b)),
        _ => unreachable!(),
    }
}

/// 严格字符串字面量:整串恰为一个带引号 token(首个引号的配对引号落在串尾)才成立。
/// `"a" + "b"` 这类含运算符的串不在此消费,交由算术/拼接步处理。
fn parse_string_literal(t: &str) -> Option<String> {
    let mut it = t.char_indices();
    let (_, q) = it.next()?;
    if q != '"' && q != '\'' {
        return None;
    }
    for (i, c) in it {
        if c == q {
            if i == t.len() - 1 {
                return Some(t[1..i].to_string());
            }
            return None;
        }
    }
    None
}

/// 找 `s[open_idx]` 处开括号的配对闭括号下标(忽略引号内容;同种括号计深)。
fn matching_close(s: &str, open_idx: usize) -> Option<usize> {
    let bytes = s.as_bytes();
    let open = bytes.get(open_idx).copied()? as char;
    let close = match open {
        '(' => ')',
        '[' => ']',
        '{' => '}',
        _ => return None,
    };
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    for (i, c) in s.char_indices() {
        if i < open_idx {
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' => quote = Some(c),
            c if c == open => depth += 1,
            c if c == close => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// 点路径名(函数名/成员名用):字母开头,后接字母/数字/下划线/点。
fn is_dotted_name(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphabetic() {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
}

/// 值类型的中文名(错误定位用)
fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "空值",
        Value::Bool(_) => "布尔",
        Value::Number(_) => "数值",
        Value::String(_) => "字符串",
        Value::Array(_) => "数组",
        Value::Object(_) => "对象",
    }
}

fn apply_method(base: Value, name: &str, args: &[Value]) -> Result<Value, String> {
    match (base, name) {
        (Value::String(s), "toUpperCase") => Ok(Value::String(s.to_uppercase())),
        (Value::String(s), "toLowerCase") => Ok(Value::String(s.to_lowercase())),
        (Value::String(s), "includes") => {
            let needle = args.first().and_then(|a| a.as_str()).unwrap_or("");
            Ok(Value::Bool(s.contains(needle)))
        }
        (Value::String(s), "trim") => Ok(Value::String(s.trim().to_string())),
        (Value::String(_), "replace") => {
            // 简单字符串替换:replace(旧, 新)
            if args.len() >= 2 {
                let old = args[0].as_str().unwrap_or("");
                let new = args[1].as_str().unwrap_or("");
                let src = match args.get(2) {
                    Some(Value::String(s0)) => s0.clone(),
                    _ => return Err("replace 需传 (source, old, new)".into()),
                };
                return Ok(Value::String(src.replace(old, new)));
            }
            Err("replace 参数不足".into())
        }
        (Value::Array(a), "length") => Ok(Value::from(a.len())),
        (other, n) => Err(format!("对象 {other:?} 不支持方法 {n}")),
    }
}

fn eval_call(name: &str, args_str: &str, env: &HashMap<String, Value>) -> Result<Value, String> {
    let args = parse_args(args_str, env)?;
    match name {
        "JSON.stringify" => {
            let v = args.first().ok_or("JSON.stringify 缺参数")?;
            Ok(Value::String(
                serde_json::to_string(v).map_err(|e| format!("序列化失败: {e}"))?,
            ))
        }
        "JSON.parse" => {
            let s = args
                .first()
                .and_then(|a| a.as_str())
                .ok_or("JSON.parse 缺参数")?;
            serde_json::from_str(s).map_err(|e| format!("JSON 解析失败: {e}"))
        }
        "String" => {
            let v = args.first().ok_or("String() 缺参数")?;
            Ok(Value::String(value_to_string(v)?))
        }
        "Number" => {
            let v = args.first().ok_or("Number() 缺参数")?;
            match v {
                Value::Number(n) => Ok(Value::from(n.as_f64().unwrap_or(0.0))),
                // 有意丢弃 ParseFloatError:唯一信息是「不是数字」,原值 s 已在消息中
                Value::String(s) => s
                    .parse::<f64>()
                    .map(Value::from)
                    .map_err(|_| format!("无法转为数字: {s}")),
                _ => Ok(Value::Null),
            }
        }
        "Math.max" | "Math.min" => {
            if args.is_empty() {
                return Err("Math.max/min 缺参数".into());
            }
            let nums: Vec<f64> = args
                .iter()
                .map(|a| a.as_f64().unwrap_or(f64::NAN))
                .collect();
            let v = if name == "Math.max" {
                nums.iter().cloned().fold(f64::NEG_INFINITY, f64::max)
            } else {
                nums.iter().cloned().fold(f64::INFINITY, f64::min)
            };
            Ok(Value::from(v))
        }
        "Math.floor" | "Math.ceil" | "Math.round" => {
            let v = args.first().and_then(|a| a.as_f64()).ok_or("缺数字参数")?;
            let r = match name {
                "Math.floor" => v.floor(),
                "Math.ceil" => v.ceil(),
                _ => v.round(),
            };
            Ok(Value::from(r))
        }
        _ => Err(format!("未支持函数: {name}")),
    }
}

fn parse_args(args_str: &str, env: &HashMap<String, Value>) -> Result<Vec<Value>, String> {
    let parts = split_top_level(args_str);
    let mut out = Vec::new();
    for p in parts {
        if p.trim().is_empty() {
            continue;
        }
        out.push(eval_expr(&p, env)?);
    }
    Ok(out)
}

fn parse_object(inner: &str, env: &HashMap<String, Value>) -> Result<Value, String> {
    let parts = split_top_level(inner);
    let mut map = serde_json::Map::new();
    for p in parts {
        let p = p.trim();
        if p.is_empty() {
            continue;
        }
        let colon = p.find(':').ok_or(format!("非法对象键值对: {p}"))?;
        let key = p[..colon].trim().trim_matches('"').trim_matches('\'');
        let val = eval_expr(&p[colon + 1..], env)?;
        map.insert(key.to_string(), val);
    }
    Ok(Value::Object(map))
}

fn parse_array(inner: &str, env: &HashMap<String, Value>) -> Result<Value, String> {
    let parts = split_top_level(inner);
    let mut arr = Vec::new();
    for p in parts {
        if p.trim().is_empty() {
            continue;
        }
        arr.push(eval_expr(&p, env)?);
    }
    Ok(Value::Array(arr))
}

fn eval_truthy(expr: &str, env: &HashMap<String, Value>) -> Result<bool, String> {
    // 逻辑 || / && 先于比较处理(|| 优先级最低;2026-10-07 修复:此前比较先行会把
    // `a > 1 && b < 2` 的右操作数切进比较里导致报错)
    if let Some(pos) = top_level_op_find(expr, "||") {
        let l = eval_truthy(expr[..pos].trim(), env)?;
        if l {
            return Ok(true);
        }
        return eval_truthy(expr[pos + 2..].trim(), env);
    }
    if let Some(pos) = top_level_op_find(expr, "&&") {
        let l = eval_truthy(expr[..pos].trim(), env)?;
        if !l {
            return Ok(false);
        }
        return eval_truthy(expr[pos + 2..].trim(), env);
    }
    // 比较运算:== != > < >= <=
    for op in [">=", "<=", "==", "!=", ">", "<"] {
        if let Some(pos) = top_level_op_find(expr, op) {
            let l = eval_expr(expr[..pos].trim(), env)?;
            let r = eval_expr(expr[pos + op.len()..].trim(), env)?;
            return Ok(compare_values(&l, &r, op));
        }
    }
    let v = eval_expr(expr, env)?;
    Ok(match v {
        Value::Bool(b) => b,
        Value::Null => false,
        Value::Number(n) => n.as_f64().unwrap_or(0.0) != 0.0,
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    })
}

fn compare_values(l: &Value, r: &Value, op: &str) -> bool {
    let lf = l.as_f64().unwrap_or(f64::NAN);
    let rf = r.as_f64().unwrap_or(f64::NAN);
    match op {
        "==" => !lf.is_nan() && !rf.is_nan() && (lf == rf) || (l == r),
        "!=" => !(l == r),
        ">" => !lf.is_nan() && !rf.is_nan() && lf > rf,
        "<" => !lf.is_nan() && !rf.is_nan() && lf < rf,
        ">=" => !lf.is_nan() && !rf.is_nan() && lf >= rf,
        "<=" => !lf.is_nan() && !rf.is_nan() && lf <= rf,
        _ => false,
    }
}

/// 顶层找字符(忽略引号与括号嵌套)
/// 注意:必须先判断目标字符再更新括号深度——否则遇到 '(' 时先 depth+=1,
/// `c == ch && depth == 0` 永远不成立,函数调用分支(JSON.stringify/Math.* 等)会不可达。
fn top_level_find(s: &str, ch: char) -> Option<usize> {
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    for (i, c) in s.char_indices() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            continue;
        }
        if c == ch && depth == 0 {
            return Some(i);
        }
        match c {
            '"' | '\'' => quote = Some(c),
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            _ => {}
        }
    }
    None
}

/// 顶层找运算符(避免 `>` 与 `>=` 混淆:匹配最长优先由调用方排序)
fn top_level_op_find(s: &str, op: &str) -> Option<usize> {
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let bytes = s.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        match c {
            '"' | '\'' => quote = Some(c),
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            _ => {}
        }
        if depth == 0 && s[i..].starts_with(op) {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// 顶层逗号切分
fn split_top_level(s: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut start = 0usize;
    for (i, c) in s.char_indices() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' => quote = Some(c),
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(s[start..i].to_string());
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(s[start..].to_string());
    parts
}

/// 允许的全局函数(白名单;与 `eval_call` 的 match 清单一致)
const ALLOWED_GLOBAL_FNS: &[&str] = &[
    "JSON.stringify",
    "JSON.parse",
    "String",
    "Number",
    "Math.max",
    "Math.min",
    "Math.floor",
    "Math.ceil",
    "Math.round",
];
/// 允许的字符串/数组方法(白名单;与 `apply_method` 一致)
const ALLOWED_METHODS: &[&str] = &["toUpperCase", "toLowerCase", "includes", "trim", "replace"];

/// 加载期脚本语法校验(PLGM 1.2,2026-10-07):**纯解析,不执行**。
///
/// 只做结构校验:语句形态(中间 `ident = 表达式`、末句 `return`/`result =`)、
/// 词法配平(引号/括号)、函数与方法名白名单、表达式非空。取值期问题(缺失参数、
/// 下标越界、类型不符)不在此拦截——由求值期显式报错。
///
/// 与求值器共享 `split_statements` 词法助手(同一份切分即无两套语法漂移);
/// 表达式级检查刻意保守(允收从严:宁可求值期再报,不误杀合法脚本)。
pub fn validate_script(script: &str) -> Result<(), String> {
    let stmts = split_statements(script)?;
    let Some((last, init)) = stmts.split_last() else {
        return Err("脚本为空:末句须为 `return <表达式>` 或 `result = <表达式>`".into());
    };
    for stmt in init {
        let (name, expr) = parse_assign(stmt)
            .ok_or_else(|| format!("语句须为 `变量 = 表达式` 赋值形式: {stmt}"))?;
        if name == "args" {
            return Err("不允许给 args 赋值(args 为入参命名空间)".into());
        }
        check_expr_shape(expr)?;
    }
    let (_, expr) = split_terminator(last)?;
    check_expr_shape(expr)
}

/// 表达式结构检查(保守;见 [`validate_script`] 的口径说明)
fn check_expr_shape(expr: &str) -> Result<(), String> {
    let t = expr.trim();
    if t.is_empty() {
        return Err("空表达式".into());
    }
    // 词法配平(与 split_statements 同口径,但这里是单个表达式片段)
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    for c in t.chars() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' => quote = Some(c),
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            _ => {}
        }
    }
    if quote.is_some() {
        return Err(format!("字符串引号未闭合: {t}"));
    }
    if depth != 0 {
        return Err(format!("括号不配平: {t}"));
    }
    // 函数/方法调用名白名单:扫描 `名(`(含点路径,跳过引号内容与分组左括号)
    let bytes = t.as_bytes();
    let mut quote: Option<char> = None;
    for (i, c) in t.char_indices() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            continue;
        }
        if c == '"' || c == '\'' {
            quote = Some(c);
            continue;
        }
        if c != '(' {
            continue;
        }
        // 向前读点名路径(字母/数字/下划线/点);前面不是标识符则是分组/列表左括号,跳过
        let mut start = i;
        while start > 0 {
            let pc = bytes[start - 1] as char;
            if pc.is_ascii_alphanumeric() || pc == '_' || pc == '.' {
                start -= 1;
            } else {
                break;
            }
        }
        if start == i {
            continue;
        }
        let full = &t[start..i];
        if is_allowed_callable(full) {
            continue;
        }
        return Err(format!(
            "不支持的函数/方法 `{full}`(加载期校验)。可用:JSON.stringify/parse、String、Number、Math.max/min/floor/ceil/round、字符串方法(toUpperCase/toLowerCase/includes/trim/replace)"
        ));
    }
    Ok(())
}

/// 调用名是否在白名单内(全局函数全名匹配;或末段为允许的方法名)
fn is_allowed_callable(full: &str) -> bool {
    let name = full.trim_start_matches('.');
    if ALLOWED_GLOBAL_FNS.contains(&name) {
        return true;
    }
    name.rsplit('.')
        .next()
        .is_some_and(|m| ALLOWED_METHODS.contains(&m))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn eval(script: &str, args: Value) -> Result<String, String> {
        eval_tool_script(script, args)
    }

    /// 回归:top_level_find 必须先判断目标字符再更新括号深度,
    /// 否则 '(' 永远匹配不到,函数调用分支(JSON.stringify/Math.*/String/Number)不可达。
    #[test]
    fn tool_plugin_function_call_works() {
        // Math.max/min 返回 f64,JSON 序列化带 .0
        assert_eq!(
            eval(
                "result = Math.max(args.min, Math.min(args.max, args.value))",
                json!({"value": 150, "min": 0, "max": 100})
            )
            .unwrap(),
            "100.0"
        );
        assert_eq!(
            eval(
                "result = Math.max(args.min, Math.min(args.max, args.value))",
                json!({"value": -5, "min": 0, "max": 100})
            )
            .unwrap(),
            "0.0"
        );
        assert_eq!(
            eval("return JSON.stringify(args.text)", json!({"text": "你好"})).unwrap(),
            "\"你好\""
        );
        assert_eq!(
            eval("return String(Math.floor(args.n))", json!({"n": 3.7})).unwrap(),
            "3.0"
        );
    }

    /// 三元 + 对象字面量返回(score_eval 示范插件的脚本)
    #[test]
    fn tool_plugin_ternary_object_works() {
        let script = r#"result = args.score >= args.pass ? {"passed": true, "grade": "合格"} : {"passed": false, "grade": "待改进"}"#;
        assert!(eval(script, json!({"score": 85, "pass": 60}))
            .unwrap()
            .contains("合格"));
        assert!(eval(script, json!({"score": 40, "pass": 60}))
            .unwrap()
            .contains("待改进"));
    }

    /// 名称格式校验(安全加固,known-limitations L19):
    /// 合法名通过;**保留前缀/非法字符/大写/超长**一律拒绝。
    ///
    /// 这条约束的意义是防**命名空间伪造**:授权裁决按工具名判风险与来源,
    /// 若插件能叫 `mcp_fs_read`,它就会被误当作 MCP 工具。
    #[test]
    fn plugin_name_validation_rejects_reserved_and_invalid_names() {
        // 合法:小写字母开头 + 小写/数字/下划线
        for ok in ["weather", "score_eval", "my_tool_2", "a"] {
            assert!(
                validate_plugin_name(ok).is_ok(),
                "应接受合法名: {ok} → {:?}",
                validate_plugin_name(ok)
            );
        }
        // 保留前缀:防命名空间伪造
        assert!(
            validate_plugin_name("mcp_fs_read").is_err(),
            "不得冒用 mcp_ 前缀"
        );
        assert!(validate_plugin_name("mcp_x").is_err());
        assert!(
            validate_plugin_name("agentgo").is_err(),
            "不得冒用 agent 域前缀"
        );
        // 非法字符 / 大写 / 首字符非字母
        assert!(validate_plugin_name("").is_err());
        assert!(validate_plugin_name("Weather").is_err(), "大写应拒绝");
        assert!(validate_plugin_name("1tool").is_err(), "数字开头应拒绝");
        assert!(validate_plugin_name("my-tool").is_err(), "连字符应拒绝");
        assert!(validate_plugin_name("my tool").is_err(), "空格应拒绝");
        assert!(validate_plugin_name("工具").is_err(), "非 ASCII 应拒绝");
        assert!(
            validate_plugin_name("../evil").is_err(),
            "路径穿越样式应拒绝"
        );
        // 超长
        assert!(validate_plugin_name(&"a".repeat(49)).is_err(), "超长应拒绝");
        assert!(validate_plugin_name(&"a".repeat(48)).is_ok(), "48 恰好合法");
    }

    /// 求值器 v2(2026-10-07):多级成员链 + 下标 + 中间变量 + 混合拼接。
    #[test]
    fn tool_plugin_chain_index_statements_work() {
        // 中间变量 + 多级链(args.user.name)+ 下标(args.tags[0])+ .length + 拼接
        let script = "who = args.user.name\nfirst = args.tags[0]\ncount = args.tags.length\nresult = \"「\" + first + \"」是 \" + who + \" 的第 1/\" + count + \" 个标签\"";
        assert_eq!(
            eval(
                script,
                json!({"user": {"name": "阿明"}, "tags": ["勇气", "温柔"]})
            )
            .unwrap(),
            "「勇气」是 阿明 的第 1/2 个标签"
        );
        // 分号分隔 + 下标(变量作下标)+ 方法链
        assert_eq!(
            eval(
                "i = 1; tags = args.tags; return tags[i].toUpperCase()",
                json!({"tags": ["a", "b"]})
            )
            .unwrap(),
            "B"
        );
        // 数值与字符串混合拼接(数值沿既有 serde_json 形态)
        assert_eq!(
            eval("return \"n=\" + args.n", json!({"n": 5})).unwrap(),
            "n=5"
        );
        // 字面量拼接(旧版把整串当单一字符串字面量,v2 修复点)
        assert_eq!(eval("return \"a\" + \"b\"", json!({})).unwrap(), "ab");
        // 数值对数值仍是算术(f64 形态与旧版一致)
        assert_eq!(eval("return 1 + 2", json!({})).unwrap(), "3.0");
        // 逻辑组合(旧版比较先行会报错,v2 修复点)
        assert_eq!(
            eval(
                "result = args.a > 1 && args.b < 3 ? \"ok\" : \"no\"",
                json!({"a": 2, "b": 2})
            )
            .unwrap(),
            "ok"
        );
    }

    /// 缺失语义:args 直接属性缺失 → 空串(整链短路);嵌套缺键 → 显式报错。
    #[test]
    fn tool_plugin_missing_arg_short_circuits_but_nested_errors() {
        assert_eq!(eval("return args.city", json!({})).unwrap(), "");
        assert_eq!(
            eval("return \"城市:\" + args.city", json!({})).unwrap(),
            "城市:"
        );
        // 链中缺失一级:整链短路为空串,不报错
        assert_eq!(eval("return args.user.name", json!({})).unwrap(), "");
        // 一级存在但嵌套缺键:显式报错,含可用键提示
        let err = eval(
            "return args.user.nam",
            json!({"user": {"name": "x", "age": 1}}),
        )
        .unwrap_err();
        assert!(err.contains("nam"), "错误应含成员名: {err}");
        assert!(err.contains("age"), "错误应含可用键: {err}");
    }

    /// 显式报错:越界 / 类型不符 / 未知变量 / 尾余内容;缺失一级参与拼接仍是空串。
    #[test]
    fn tool_plugin_evaluation_errors_are_explicit() {
        let err = eval("return args.tags[5]", json!({"tags": ["a"]})).unwrap_err();
        assert!(err.contains("越界"), "越界应显式报错: {err}");
        let err = eval("return args.count.foo", json!({"count": 3})).unwrap_err();
        assert!(err.contains("无成员"), "类型不符应显式报错: {err}");
        let err = eval("return nope", json!({})).unwrap_err();
        assert!(err.contains("未知变量"), "{err}");
        let err = eval("return args.s.badMethod()", json!({"s": "x"})).unwrap_err();
        assert!(!err.is_empty(), "未知方法应报错: {err}");
        let err = eval("return args.a junk", json!({"a": 1})).unwrap_err();
        assert!(err.contains("多余内容"), "尾余内容应报错: {err}");
        assert_eq!(eval("return args.missing + \"x\"", json!({})).unwrap(), "x");
    }

    /// 加载期校验:合法脚本通过;形态错误 / 非白名单函数在加载期就拒绝(不再等调用)。
    #[test]
    fn validate_script_accepts_valid_and_rejects_broken() {
        for ok in [
            "return 1 + 2",
            "result = args.a",
            "x = 1\nreturn x + args.n",
            "who = args.user.name\nfirst = args.tags[0]\nresult = \"「\" + first + \"」\"",
            "return \"含(括号的文本\"",
            "return args.a.toUpperCase()",
            "result = args.score >= args.pass ? {\"passed\": true} : {\"passed\": false}",
        ] {
            assert!(
                validate_script(ok).is_ok(),
                "应通过: {ok} → {:?}",
                validate_script(ok)
            );
        }
        for bad in [
            "if (args.city) { return 1 }", // 不支持语句形态
            "return",                      // return 缺表达式
            "return foo(",                 // 括号不配平
            "return \"未闭合",             // 引号未闭合
            "return bar(args.x)",          // 非白名单函数
            "return args.s.substring(1)",  // 非白名单方法
            "x = 1",                       // 末句不是 return/result
            "args = 1\nreturn 1",          // 不得给 args 赋值
            "",                            // 空脚本
        ] {
            assert!(
                validate_script(bad).is_err(),
                "应拒绝: {bad} → {:?}",
                validate_script(bad)
            );
        }
    }

    /// 交叉网(PLGM 1.2 裁定 5):校验通过 ⇒ 语料求值必须成功;校验拒绝 ⇒ 求值也必须失败。
    /// 防「校验器与求值器两套语法」漂移(允收从严:校验通过但求值因取值失败不算漂移)。
    #[test]
    fn validate_and_eval_agree_on_corpus() {
        let corpus: &[(&str, serde_json::Value)] = &[
            ("return args.tags[0]", json!({"tags": ["a"]})),
            ("return args.user.name", json!({})),
            (
                "result = args.score >= args.pass ? \"合格\" : \"待改进\"",
                json!({"score": 85, "pass": 60}),
            ),
            ("x = 1\nreturn x", json!({})),
            ("return 1 + 2", json!({})),
            ("return bar(args.x)", json!({})), // 校验拒绝 → 求值也失败
            ("if (x) { return 1 }", json!({})), // 校验拒绝 → 求值也失败
            ("return \"未闭合", json!({})),    // 校验拒绝 → 求值也失败
        ];
        for (script, args) in corpus {
            let v = validate_script(script);
            let e = eval(script, args.clone());
            if v.is_ok() {
                assert!(e.is_ok(), "校验通过则应可求值: {script} → {e:?}");
            } else {
                assert!(e.is_err(), "校验拒绝则求值也必须失败: {script} → {e:?}");
            }
        }
    }

    /// 加载期校验接入 parse_file:语法坏文件在解析阶段即拒(PLGM 1.2),并记录来源文件名。
    #[test]
    fn parse_file_records_name_and_validates_script() {
        let dir = std::env::temp_dir().join(format!("kedai-plugin-parse-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let good = dir.join("good_plugin.json");
        std::fs::write(
            &good,
            r#"{"name": "good_plugin", "description": "测试", "script": "return args.a"}"#,
        )
        .unwrap();
        let loader = ToolPluginLoader::new(dir.clone());
        let loaded = loader.parse_file(&good).expect("合法插件应解析成功");
        assert_eq!(loaded.file, "good_plugin.json", "应记录来源文件名");
        assert_eq!(loaded.definition.name, "good_plugin");

        let bad = dir.join("bad_plugin.json");
        std::fs::write(
            &bad,
            r#"{"name": "bad_plugin", "description": "测试", "script": "if (args.a) { return 1 }"}"#,
        )
        .unwrap();
        let err = loader.parse_file(&bad).unwrap_err();
        assert!(
            err.contains("末句") || err.contains("语句"),
            "语法错误应在加载期报: {err}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
