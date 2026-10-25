// 单文件交付的运行期自检(SKY-1 机器闸门,2026-10-25)。
//
// 为什么需要:静态接线只能证明「名字/文件对得上」,证明不了「入口真的能跑起来」。
// 2026-10-24 的第三轮空岛复跑里,`index.html` 178KB、槽位 0、`missing=[]`,
// 全部静态检查通过,但审计仍只能把 `window.GameMain.ready` / `scene.children` /
// `BlockEngine.map.size` 标成「待验」——这些值只有把页面跑起来才能读到。审计原文
// 甚至怀疑 `parts/block-engine.js` 只导出 `MC_BLOCK_ENGINE` 而没有 `BlockEngine`;
// 机器加载一次就能给出确定结论(实际两处都导出了)。
//
// 实现选择(2026-10-25 实验定稿):**不引入 WebSocket/CDP 依赖**。
// Chromium 系浏览器的 `--headless=new --virtual-time-budget=N --dump-dom` 会在
// 虚拟时间推进到预算后把序列化 DOM 打到 stdout;我们把两段探针脚本注入交付 HTML
// 的副本(头:捕获 error/unhandledrejection/console.error/资源加载失败;尾:在
// DOMContentLoaded 后采样 window 全局,把快照写进 `data-kedai-probe` 属性),
// 再从 stdout 里取属性值。无端口、无 WS、无新依赖,一次 3~6 秒。
//
// 判据(确定性,不做语义猜测):
// - 未捕获脚本错误、入口报错日志 → 失败;
// - 任一应用命名空间 `ready=false` → 失败;
// - `scene.children` 为空 / `map.size=0` / `renderer=null` / `getState()` 里
//   场景计数为 0 → 失败;
// - 暴露了 THREE 或目标含 3D 线索却没有 canvas → 失败;
// - 外部资源(CDN)加载失败只记提示,不单独判失败(离线/网络问题不是代码缺陷);
// - 没有任何可判定信号(纯静态页)→ 未判定,不判失败。
//
// 边界:不做交互、不测帧率、不验证视觉;`requestAnimationFrame` 在无头环境不推进,
// 因此只采信装配完成后的同步状态(本模块也刻意不把 `fps/draws` 当判据)。
use serde::Deserialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

/// 探针报告写回 DOM 的属性名。
const PROBE_ATTR: &str = "data-kedai-probe";
/// 尾部脚本里的固定标记:stdout 有 DOM 但没属性时,用它区分「页面脚本中断」与
/// 「浏览器根本没输出内容」。
const PROBE_MARKER: &str = "__KEDAI_RUNTIME_PROBE__";
/// 浏览器整体超时(进程墙钟;虚拟时间预算之外的安全边界)。
const PROBE_TIMEOUT: Duration = Duration::from_secs(45);
/// 交付 HTML 读取上限(与 `fs_inline_html` 的 16MB 上限同口径)。
const MAX_HTML_BYTES: u64 = 16 * 1024 * 1024;
/// stdout 收集上限(超限继续排空管道但不保留,避免大页面把内存打满)。
const MAX_DOM_BYTES: usize = 32 * 1024 * 1024;
/// 返回给终态门槛的问题条数上限。
const MAX_ISSUES: usize = 8;

/// 运行期自检结果。
///
/// - `issues` 非空 → 终态门槛降 partial 并点名;
/// - `notes` 只进 Status 事件/日志(外部资源不可达、未判定原因等软信息);
/// - `verified=false` 表示「没有拿到可判定的运行期事实」,调用方不得把它当通过。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RuntimeOutcome {
    pub issues: Vec<String>,
    pub notes: Vec<String>,
    pub verified: bool,
}

impl RuntimeOutcome {
    fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            issues: Vec::new(),
            notes: vec![reason.into()],
            verified: false,
        }
    }
}

/// 浏览器回传的探针快照(字段名与尾部脚本的 camelCase 一致)。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProbeReport {
    #[serde(default)]
    errors: Vec<serde_json::Value>,
    #[serde(default)]
    logs: Vec<String>,
    #[serde(default)]
    canvases: usize,
    #[serde(default)]
    three: bool,
    #[serde(default)]
    namespaces: Vec<NamespaceSnapshot>,
    #[serde(default)]
    resource_errors: Vec<String>,
}

/// 一个 `window.*` 应用命名空间的可观测状态。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NamespaceSnapshot {
    name: String,
    #[serde(default)]
    ready: Option<bool>,
    #[serde(default)]
    scene_children: Option<i64>,
    #[serde(default)]
    map_size: Option<i64>,
    #[serde(default)]
    renderer_null: Option<bool>,
    /// 核心对象里为 null/undefined 的字段名(renderer/scene/camera/engine)。
    #[serde(default)]
    nulls: Vec<String>,
    #[serde(default)]
    state: Option<serde_json::Value>,
}

/// 尾部探针:DOMContentLoaded 后两次采样(1.2s / 3.2s),后一次覆盖前一次。
/// 两次采样是为了容纳「先注册 load、再异步 boot」的入口形态;`requestAnimationFrame`
/// 在无头环境不推进,所以这里只用 `setTimeout`。
const PROBE_TAIL: &str = r#"<script>(function(){
/* __KEDAI_RUNTIME_PROBE__ */
function report(){
  var r={errors:(window.__kv&&window.__kv.errors)||[],logs:(window.__kv&&window.__kv.logs)||[],resources:(window.__kv&&window.__kv.resources)||[],canvases:0,three:!!window.THREE,namespaces:[]};
  try{var els=document.querySelectorAll('canvas');r.canvases=els.length;}catch(_){}
  try{
    for(var k in window){
      if(k==='window'||k==='self'||k==='top'||k==='parent'||k==='document'||k==='location'||k==='navigator'||k==='console'||k==='history'||k==='screen'||k==='performance'||k==='crypto'||k==='localStorage'||k==='sessionStorage'||k==='frames'||k==='customElements'||k==='speechSynthesis'||k==='indexedDB'||k==='caches'||k==='visualViewport'||k==='origin'||k==='name'||k==='status'||k==='closed'||k==='length'||k==='chrome'||k==='crashReport'||k==='cookieStore')continue;
      var v=null;try{v=window[k]}catch(_){continue}
      if(!v)continue;
      var t=typeof v;if(t!=='object'&&t!=='function')continue;
      var item={name:k,ready:null,sceneChildren:null,mapSize:null,rendererNull:null,nulls:[],getState:typeof v.getState==='function'};
      try{if('ready' in v&&typeof v.ready==='boolean')item.ready=v.ready;else if('isReady' in v&&typeof v.isReady==='boolean')item.ready=v.isReady;}catch(_){}
      try{if(v.scene&&v.scene.children&&typeof v.scene.children.length==='number')item.sceneChildren=v.scene.children.length;}catch(_){}
      try{if(v.engine&&v.engine.map&&typeof v.engine.map.size==='number')item.mapSize=v.engine.map.size;else if(v.map&&typeof v.map.size==='number')item.mapSize=v.map.size;}catch(_){}
      try{if('renderer' in v)item.rendererNull=(v.renderer===null||typeof v.renderer==='undefined');}catch(_){}
      var core=['renderer','scene','camera','engine'];
      for(var ci=0;ci<core.length;ci++){var cp=core[ci];try{if(cp in v&&(v[cp]===null||typeof v[cp]==='undefined'))item.nulls.push(cp);}catch(_){}}
      if(item.getState){try{var s=v.getState();item.state={};for(var sk in s){var sv=s[sk];if(sv===null||typeof sv!=='object')item.state[sk]=sv;}}catch(e){item.stateError=String(e);}}
      if(item.ready!==null||item.sceneChildren!==null||item.mapSize!==null||item.rendererNull!==null||item.nulls.length||item.getState)r.namespaces.push(item);
    }
  }catch(_){}
  document.documentElement.setAttribute('data-kedai-probe',encodeURIComponent(JSON.stringify(r)));
}
function go(){setTimeout(report,1200);setTimeout(report,3200);}
if(document.readyState!=='loading')go();else{document.addEventListener('DOMContentLoaded',go);setTimeout(go,5000);}
})();</script>"#;

/// 头部探针:在页面自己的脚本之前安装错误/日志/资源失败捕获。
const PROBE_HEAD: &str = r#"<script>(function(){
window.__kv={errors:[],logs:[],resources:[]};
function err(m){try{window.__kv.errors.push(m)}catch(_){}}
window.addEventListener('error',function(e){
  try{
    var t=e&&e.target;
    if(t&&t!==window&&(t.src||t.href)){window.__kv.resources.push(String(t.src||t.href));return;}
    err({message:String((e&&e.message)||'unknown'),file:String((e&&e.filename)||''),line:(e&&e.lineno)||0,col:(e&&e.colno)||0});
  }catch(_){}
},true);
window.addEventListener('unhandledrejection',function(e){try{err({message:'unhandledrejection: '+String(e&&e.reason)})}catch(_){}});
var ce=console.error;
console.error=function(){try{window.__kv.logs.push(Array.prototype.slice.call(arguments).map(String).join(' ').slice(0,300))}catch(_){};return ce.apply(console,arguments)};
})();</script>"#;

/// 把探针注入交付 HTML 的副本(只用于自检,不改原文件)。
///
/// 插入点:头部探针放在第一个 `<head...>` 之后(没有 head 就放最前);尾部探针放在
/// **最后一个** `</body>` 之前(避免命中脚本字符串里的同名文本)。大小写不敏感。
pub(crate) fn inject_probe(html: &str) -> String {
    let mut out = String::with_capacity(html.len() + PROBE_HEAD.len() + PROBE_TAIL.len());
    let head_insert =
        find_head_tag(html).and_then(|start| html[start..].find('>').map(|rel| start + rel + 1));
    let body_insert = rfind_ci(html, "</body>");
    match (head_insert, body_insert) {
        (Some(head_at), Some(body_at)) if body_at >= head_at => {
            out.push_str(&html[..head_at]);
            out.push_str(PROBE_HEAD);
            out.push_str(&html[head_at..body_at]);
            out.push_str(PROBE_TAIL);
            out.push_str(&html[body_at..]);
        }
        (Some(head_at), _) => {
            out.push_str(&html[..head_at]);
            out.push_str(PROBE_HEAD);
            out.push_str(&html[head_at..]);
            out.push_str(PROBE_TAIL);
        }
        (None, Some(body_at)) => {
            out.push_str(PROBE_HEAD);
            out.push_str(&html[..body_at]);
            out.push_str(PROBE_TAIL);
            out.push_str(&html[body_at..]);
        }
        (None, None) => {
            out.push_str(PROBE_HEAD);
            out.push_str(html);
            out.push_str(PROBE_TAIL);
        }
    }
    out
}

/// 从 dump-dom 输出里取出探针属性并解码为 JSON 文本。
///
/// 属性值是 `encodeURIComponent(JSON.stringify(...))`,因此不会出现 `"` 或 `&`,
/// 直接截到下一个引号即可。取**最后一个**出现位置(防御页面自身文本碰巧含同名字样)。
pub(crate) fn extract_report(dom: &str) -> Option<String> {
    let marker = format!("{PROBE_ATTR}=\"");
    let start = dom.rfind(&marker)? + marker.len();
    let rest = &dom[start..];
    let end = rest.find('"')?;
    percent_decode(&rest[..end])
}

fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return None;
            }
            let hi = hex_val(bytes[i + 1])?;
            let lo = hex_val(bytes[i + 2])?;
            out.push(hi * 16 + lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// 对一份探针快照做确定性判定(纯函数,便于逐条钉住规则)。
pub(crate) fn evaluate(report: &ProbeReport, goal: &str) -> RuntimeOutcome {
    let mut issues: Vec<String> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for value in report.errors.iter().take(3) {
        let message = error_message(value);
        let text = if message.trim() == "Script error." {
            "页面有未捕获的脚本错误(跨域脚本已屏蔽详情)".to_string()
        } else {
            format!("页面有未捕获的脚本错误:{message}")
        };
        push_issue(&mut issues, &mut seen, text);
    }
    for log in report.logs.iter().take(3) {
        push_issue(
            &mut issues,
            &mut seen,
            format!("入口输出错误日志:{}", truncate(log, 160)),
        );
    }
    for ns in &report.namespaces {
        if ns.ready == Some(false) {
            push_issue(
                &mut issues,
                &mut seen,
                format!("{}.ready=false(入口未完成初始化)", ns.name),
            );
        }
        if ns.scene_children == Some(0) {
            push_issue(
                &mut issues,
                &mut seen,
                format!("{}.scene.children 为空(场景未装配)", ns.name),
            );
        }
        if ns.map_size == Some(0) {
            push_issue(
                &mut issues,
                &mut seen,
                format!("{}:世界数据为空(map.size=0)", ns.name),
            );
        }
        if ns.renderer_null == Some(true) {
            push_issue(
                &mut issues,
                &mut seen,
                format!("{}.renderer 为 null(渲染器未创建)", ns.name),
            );
        }
        for field in &ns.nulls {
            push_issue(
                &mut issues,
                &mut seen,
                format!("{}.{} 为 null(核心对象未装配)", ns.name, field),
            );
        }
        if let Some(state) = &ns.state {
            check_state(&ns.name, state, &mut issues, &mut seen);
        }
    }

    if !report.resource_errors.is_empty() {
        let list: Vec<String> = report
            .resource_errors
            .iter()
            .take(3)
            .map(|u| truncate(u, 120))
            .collect();
        notes.push(format!(
            "外部资源加载失败(不作为失败判据,可能是网络不可达):{}",
            list.join("、")
        ));
    }

    let lower_goal = goal.to_ascii_lowercase();
    let visual_cue = ["3d", "three", "webgl", "canvas", "画布", "三维"]
        .iter()
        .any(|cue| lower_goal.contains(cue));
    let visual_expected = report.three
        || report.canvases > 0
        || visual_cue
        || report
            .namespaces
            .iter()
            .any(|n| n.scene_children.is_some() || n.map_size.is_some());
    if visual_expected && report.canvases == 0 {
        push_issue(
            &mut issues,
            &mut seen,
            "页面没有 canvas 画布(3D/可视化交付未渲染)".to_string(),
        );
    }

    let has_signal = report.canvases > 0
        || report.three
        || !report.namespaces.is_empty()
        || !report.errors.is_empty()
        || !report.logs.is_empty();
    if !has_signal {
        notes.push("未发现可判定的运行期信号(可能是纯静态页面)".to_string());
    } else if issues.is_empty() {
        notes.push(format!(
            "运行期自检通过:canvas={}, 可观测命名空间={}",
            report.canvases,
            report.namespaces.len()
        ));
    }
    if issues.len() > MAX_ISSUES {
        let more = issues.len() - MAX_ISSUES;
        issues.truncate(MAX_ISSUES);
        notes.push(format!("另有 {more} 项运行期问题未展开"));
    }
    RuntimeOutcome {
        verified: has_signal,
        issues,
        notes,
    }
}

fn check_state(
    name: &str,
    state: &serde_json::Value,
    issues: &mut Vec<String>,
    seen: &mut HashSet<String>,
) {
    let Some(obj) = state.as_object() else {
        return;
    };
    if obj.get("ready").and_then(|v| v.as_bool()) == Some(false) {
        push_issue(
            issues,
            seen,
            format!("{name}.getState().ready=false(入口未完成初始化)"),
        );
    }
    for key in [
        "sceneChildren",
        "childrenCount",
        "mapSize",
        "meshes",
        "blocks",
        "blockCount",
    ] {
        if obj.get(key).and_then(|v| v.as_i64()) == Some(0) {
            push_issue(
                issues,
                seen,
                format!("{name}.getState().{key}=0(场景/内容为空)"),
            );
        }
    }
    if let Some(err) = obj
        .get("error")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        push_issue(
            issues,
            seen,
            format!("{name}.getState().error={}", truncate(err, 160)),
        );
    }
}

fn error_message(value: &serde_json::Value) -> String {
    if let Some(s) = value.as_str() {
        return s.to_string();
    }
    let message = value
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let file = value.get("file").and_then(|v| v.as_str()).unwrap_or("");
    let line = value.get("line").and_then(|v| v.as_i64()).unwrap_or(0);
    // 探针副本位于系统临时目录且行号被注入脚本改变;把该路径写进用户可见错误
    // 只会泄露本机路径、不提供有效定位,直接省略。CDN/真实文件位置仍然保留。
    let probe_copy = file.contains("kedai-runtime-check-") || file.ends_with("/probe.html");
    if file.is_empty() || probe_copy {
        message.to_string()
    } else {
        format!("{message} ({file}:{line})")
    }
}

fn push_issue(issues: &mut Vec<String>, seen: &mut HashSet<String>, text: String) {
    if seen.insert(text.clone()) {
        issues.push(text);
    }
}

fn truncate(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (i, ch) in text.chars().enumerate() {
        if i >= max_chars {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

/// 对一次单文件交付跑运行期自检。
///
/// 测试构建(`cfg(test)` 或 `test-support`)直接跳过:CI 没有浏览器,真实浏览器
/// 路径由 `#[ignore]` 的手工用例与真机实测覆盖;判定规则由 [`evaluate`] 单测覆盖。
pub(crate) async fn probe(workspace: &Path, entry_rel: &str, goal: &str) -> RuntimeOutcome {
    if cfg!(test) || cfg!(feature = "test-support") {
        return RuntimeOutcome::unavailable("测试构建跳过运行期自检");
    }
    let Some(browser) = browser_path() else {
        return RuntimeOutcome::unavailable(
            "未找到可用的无头浏览器(Edge/Chrome),运行期自检跳过;可用 KEDAI_HEADLESS_BROWSER 指定路径",
        );
    };
    probe_with(&browser, workspace, entry_rel, goal).await
}

/// 指定浏览器执行自检(供手工验证与 `#[ignore]` 用例使用)。
pub(crate) async fn probe_with(
    browser: &Path,
    workspace: &Path,
    entry_rel: &str,
    goal: &str,
) -> RuntimeOutcome {
    let entry = workspace.join(entry_rel);
    let Some(html) = read_text_capped(&entry, MAX_HTML_BYTES) else {
        return RuntimeOutcome::unavailable(format!("无法读取交付入口 {entry_rel}"));
    };
    let dir = probe_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return RuntimeOutcome::unavailable(format!("创建运行期自检临时目录失败:{e}"));
    }
    let outcome = run_probe_in_dir(browser, &dir, &html, goal).await;
    // 浏览器可能还攥着 profile 里的锁文件;清理失败不影响结论,重试交给系统临时目录。
    let _ = std::fs::remove_dir_all(&dir);
    outcome
}

async fn run_probe_in_dir(browser: &Path, dir: &Path, html: &str, goal: &str) -> RuntimeOutcome {
    let probe_path = dir.join("probe.html");
    if let Err(e) = std::fs::write(&probe_path, inject_probe(html)) {
        return RuntimeOutcome::unavailable(format!("写入运行期自检副本失败:{e}"));
    }
    let url = file_url(&probe_path);
    let dom = match run_browser(browser, dir, &url).await {
        Ok(dom) => dom,
        Err(e) => return RuntimeOutcome::unavailable(e),
    };
    match extract_report(&dom) {
        Some(json) => match serde_json::from_str::<ProbeReport>(&json) {
            Ok(report) => evaluate(&report, goal),
            Err(e) => RuntimeOutcome::unavailable(format!("运行期探针报告无法解析:{e}")),
        },
        None if dom.contains(PROBE_MARKER) => RuntimeOutcome {
            issues: vec![
                "运行期探针未执行(页面脚本可能中断或页面被重定向,入口未能完成加载)".to_string(),
            ],
            notes: Vec::new(),
            verified: true,
        },
        None => RuntimeOutcome::unavailable("无头浏览器没有返回页面 DOM"),
    }
}

async fn run_browser(browser: &Path, dir: &Path, url: &str) -> Result<String, String> {
    use tokio::io::AsyncReadExt;

    let profile = dir.join("profile");
    let budget = "15000";
    let mut cmd = tokio::process::Command::new(browser);
    cmd.arg("--headless=new")
        .arg("--disable-gpu")
        .arg("--enable-unsafe-swiftshader")
        .arg("--use-angle=swiftshader")
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-extensions")
        .arg("--disable-background-networking")
        .arg("--disable-sync")
        .arg("--disable-default-apps")
        .arg("--disable-component-update")
        .arg("--mute-audio")
        .arg("--window-size=1280,800")
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg(format!("--virtual-time-budget={budget}"))
        .arg("--dump-dom")
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        // 无控制台窗口(任务引擎可能跑在 GUI 进程里)。
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("启动无头浏览器失败({}):{e}", browser.display()))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| "无法读取无头浏览器 stdout".to_string())?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| "无法读取无头浏览器 stderr".to_string())?;
    let out_task = tokio::spawn(async move {
        let mut out: Vec<u8> = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            match stdout.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let room = MAX_DOM_BYTES.saturating_sub(out.len());
                    out.extend_from_slice(&buf[..n.min(room)]);
                }
            }
        }
        out
    });
    let err_task = tokio::spawn(async move {
        let mut out: Vec<u8> = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            match stderr.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if out.len() < 8192 {
                        out.extend_from_slice(&buf[..n.min(8192 - out.len())]);
                    }
                }
            }
        }
        out
    });
    let status = match tokio::time::timeout(PROBE_TIMEOUT, child.wait()).await {
        Ok(Ok(status)) => status,
        Ok(Err(e)) => {
            let _ = child.start_kill();
            return Err(format!("等待无头浏览器失败:{e}"));
        }
        Err(_) => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err(format!(
                "无头浏览器超时({}s),已终止",
                PROBE_TIMEOUT.as_secs()
            ));
        }
    };
    let out = out_task.await.unwrap_or_default();
    let err = err_task.await.unwrap_or_default();
    if !status.success() && out.is_empty() {
        let detail = String::from_utf8_lossy(&err);
        return Err(format!(
            "无头浏览器退出码 {:?}:{}",
            status.code(),
            truncate(detail.trim(), 300)
        ));
    }
    Ok(String::from_utf8_lossy(&out).into_owned())
}

fn read_text_capped(path: &Path, cap: u64) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > cap {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

/// 每次自检独立的临时目录(profile 也放里面,避免与用户浏览器实例互相影响)。
fn probe_dir() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("kedai-runtime-check-{}-{seq}", std::process::id()))
}

/// 绝对路径 → `file:///...` URL(Windows 反斜杠、中文与空格都要正确编码)。
pub(crate) fn file_url(path: &Path) -> String {
    let mut raw = path.to_string_lossy().replace('\\', "/");
    if !raw.starts_with('/') {
        raw.insert(0, '/');
    }
    let mut out = String::with_capacity(raw.len() + 16);
    out.push_str("file://");
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// 查找可用的 Chromium 系浏览器;`KEDAI_HEADLESS_BROWSER=off` 可显式关闭。
pub(crate) fn browser_path() -> Option<PathBuf> {
    if let Ok(raw) = std::env::var("KEDAI_HEADLESS_BROWSER") {
        let value = raw.trim();
        if matches!(
            value.to_ascii_lowercase().as_str(),
            "off" | "none" | "0" | "false"
        ) {
            return None;
        }
        if !value.is_empty() {
            let path = PathBuf::from(value);
            return path.is_file().then_some(path);
        }
    }
    browser_candidates()
        .into_iter()
        .find(|candidate| candidate.is_file())
}

fn browser_candidates() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    #[cfg(windows)]
    {
        let mut roots: Vec<PathBuf> = Vec::new();
        for key in ["ProgramFiles(x86)", "ProgramFiles", "LOCALAPPDATA"] {
            if let Ok(value) = std::env::var(key) {
                if !value.trim().is_empty() {
                    roots.push(PathBuf::from(value));
                }
            }
        }
        for root in roots {
            out.push(root.join("Microsoft/Edge/Application/msedge.exe"));
            out.push(root.join("Google/Chrome/Application/chrome.exe"));
        }
    }
    #[cfg(target_os = "macos")]
    {
        out.push(PathBuf::from(
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        ));
        out.push(PathBuf::from(
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        ));
        out.push(PathBuf::from(
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
        ));
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        for path in [
            "/usr/bin/google-chrome",
            "/usr/bin/google-chrome-stable",
            "/usr/bin/chromium",
            "/usr/bin/chromium-browser",
            "/usr/bin/microsoft-edge",
            "/usr/bin/microsoft-edge-stable",
            "/snap/bin/chromium",
        ] {
            out.push(PathBuf::from(path));
        }
    }
    out
}

/// 查找真正的 `<head>` 开始位置(`<header>` 不算;标签名后必须是空白或 `>`)。
fn find_head_tag(html: &str) -> Option<usize> {
    let bytes = html.as_bytes();
    let mut from = 0;
    while let Some(rel) = find_ci(&html[from..], "<head") {
        let start = from + rel;
        match bytes.get(start + 5) {
            Some(b'>') | Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'\r') => {
                return Some(start)
            }
            _ => from = start + 5,
        }
    }
    None
}

/// 大小写不敏感的正向查找(HTML 标签都是 ASCII;非 ASCII 字节按原样比较)。
fn find_ci(hay: &str, needle: &str) -> Option<usize> {
    let h = hay.as_bytes();
    let n = needle.as_bytes();
    if n.is_empty() || h.len() < n.len() {
        return None;
    }
    (0..=h.len() - n.len()).find(|&i| h[i..i + n.len()].eq_ignore_ascii_case(n))
}

/// 大小写不敏感的逆向查找。
fn rfind_ci(hay: &str, needle: &str) -> Option<usize> {
    let h = hay.as_bytes();
    let n = needle.as_bytes();
    if n.is_empty() || h.len() < n.len() {
        return None;
    }
    (0..=h.len() - n.len())
        .rev()
        .find(|&i| h[i..i + n.len()].eq_ignore_ascii_case(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(json: &str) -> ProbeReport {
        serde_json::from_str(json).expect("测试报告应为合法 JSON")
    }

    #[test]
    fn inject_probe_uses_first_head_and_last_body() {
        let html = "<html><head><title>t</title></head><body>\
                    <script>var s = '</body>';</script></body></html>";
        let out = inject_probe(html);
        let head_at = out.find(PROBE_MARKER).expect("尾部探针标记应存在");
        let prelude_at = out.find("window.__kv={errors").expect("头部探针应存在");
        assert!(prelude_at < head_at, "头部探针必须先于尾部探针");
        assert!(
            out.ends_with("</body></html>"),
            "尾部探针必须插在最后一个 </body> 之前: {out}"
        );
        assert_eq!(
            out.matches("</body>").count(),
            html.matches("</body>").count(),
            "不得凭空增加或删除 body 结束标签"
        );
        assert!(
            out.contains("'</body>'"),
            "脚本字符串里的 </body> 不得被破坏"
        );
    }

    #[test]
    fn inject_probe_handles_missing_head_and_body() {
        let out = inject_probe("<!doctype html><div>hi</div>");
        assert!(out.starts_with("<script>"), "无 head 时头部探针放在最前");
        assert!(out.contains(PROBE_MARKER), "无 body 时尾部探针追加在末尾");
        assert!(out.ends_with("</script>"));
    }

    #[test]
    fn extract_report_decodes_percent_encoded_utf8() {
        let json = r#"{"errors":[],"logs":[],"canvases":1,"namespaces":[{"name":"游戏入口","ready":true}]}"#;
        let encoded = encode_uri_component(json);
        let dom = format!("<html data-kedai-probe=\"{encoded}\"></html>");
        let decoded = extract_report(&dom).expect("应能解码探针属性");
        assert_eq!(decoded, json);
    }

    #[test]
    fn extract_report_takes_last_attribute_and_ignores_others() {
        let dom = "<div data-kedai-probe=\"%7B%7D\"></div><html data-kedai-probe=\"%7B%22canvases%22%3A2%7D\"></html>";
        assert_eq!(extract_report(dom).as_deref(), Some(r#"{"canvases":2}"#));
        assert!(extract_report("<html></html>").is_none());
    }

    #[test]
    fn percent_decode_rejects_truncated_escape() {
        assert!(percent_decode("%E4").is_none());
        assert_eq!(percent_decode("a%20b").as_deref(), Some("a b"));
    }

    #[test]
    fn file_url_encodes_spaces_and_chinese() {
        let url = file_url(Path::new("D:/工作 区/index.html"));
        assert!(url.starts_with("file:///D:/"), "{url}");
        assert!(!url.contains(' '), "{url}");
        assert!(
            url.contains("%E5%B7%A5"),
            "中文必须按 UTF-8 百分号编码: {url}"
        );
    }

    /// 健康入口:ready=true、场景与地图非空 → 无问题且已判定。
    #[test]
    fn evaluate_healthy_entry_passes() {
        let rep = report(
            r#"{"errors":[],"logs":[],"canvases":1,"three":true,
                "namespaces":[{"name":"GameMain","ready":true,"sceneChildren":12,"mapSize":900,
                "rendererNull":false,"nulls":[],"getState":true,
                "state":{"ready":true,"sceneChildren":12,"mapSize":900}}]}"#,
        );
        let outcome = evaluate(&rep, "做一个 3D 空岛场景,单文件交付");
        assert!(outcome.verified, "{outcome:?}");
        assert!(outcome.issues.is_empty(), "{outcome:?}");
    }

    /// 入口未就绪 → 点名 ready=false(实测 `GameMain` 的 boot 抛错形态)。
    #[test]
    fn evaluate_flags_not_ready_entry() {
        let rep = report(
            r#"{"errors":[],"logs":[],"canvases":1,"three":true,
                "namespaces":[{"name":"GameMain","ready":false,"getState":true,
                "state":{"ready":false,"version":"1.0.0"}}]}"#,
        );
        let outcome = evaluate(&rep, "3D 场景");
        assert!(
            outcome
                .issues
                .iter()
                .any(|i| i.contains("GameMain.ready=false")),
            "{outcome:?}"
        );
        assert!(
            outcome
                .issues
                .iter()
                .any(|i| i.contains("getState().ready=false")),
            "{outcome:?}"
        );
    }

    /// 空场景 / 空地图 / renderer=null 都要点名,并保留命名空间名。
    #[test]
    fn evaluate_flags_empty_scene_map_and_null_renderer() {
        let rep = report(
            r#"{"errors":[],"canvases":1,"three":true,
                "namespaces":[{"name":"App","ready":true,"sceneChildren":0,"mapSize":0,
                "rendererNull":true,"nulls":["scene"],"getState":true,
                "state":{"meshes":0,"blocks":0}}]}"#,
        );
        let outcome = evaluate(&rep, "3D 场景");
        for needle in [
            "App.scene.children 为空",
            "世界数据为空(map.size=0)",
            "App.renderer 为 null",
            "App.scene 为 null",
            "getState().meshes=0",
            "getState().blocks=0",
        ] {
            assert!(
                outcome.issues.iter().any(|i| i.contains(needle)),
                "缺少「{needle}」: {outcome:?}"
            );
        }
    }

    /// 未捕获错误与入口错误日志都要进失败项;跨域屏蔽文案不暴露空消息。
    #[test]
    fn evaluate_flags_errors_and_console_errors() {
        let rep = report(
            r#"{"errors":[{"message":"Script error.","file":"","line":0},
                {"message":"boom","file":"app.js","line":9}],
                "logs":["缺少模块 BlockEngine"],"canvases":1}"#,
        );
        let outcome = evaluate(&rep, "3D 场景");
        assert!(
            outcome
                .issues
                .iter()
                .any(|i| i.contains("跨域脚本已屏蔽详情")),
            "{outcome:?}"
        );
        assert!(
            outcome
                .issues
                .iter()
                .any(|i| i.contains("boom") && i.contains("app.js:9")),
            "{outcome:?}"
        );
        assert!(
            outcome
                .issues
                .iter()
                .any(|i| i.contains("缺少模块 BlockEngine")),
            "{outcome:?}"
        );
    }

    /// 3D 目标但页面没有 canvas → 失败(渲染器根本没挂到 DOM)。
    #[test]
    fn evaluate_requires_canvas_for_3d_goal() {
        let rep = report(r#"{"errors":[],"canvases":0,"three":true,"namespaces":[]}"#);
        let outcome = evaluate(&rep, "用 Three.js 做一个 3D 场景");
        assert!(
            outcome
                .issues
                .iter()
                .any(|i| i.contains("没有 canvas 画布")),
            "{outcome:?}"
        );
    }

    /// 纯静态页没有任何可判定信号 → 未判定,不误判失败。
    #[test]
    fn evaluate_marks_signal_less_page_unverified() {
        let rep = report(r#"{"errors":[],"canvases":0,"three":false,"namespaces":[]}"#);
        let outcome = evaluate(&rep, "写一个说明页");
        assert!(!outcome.verified, "{outcome:?}");
        assert!(outcome.issues.is_empty(), "{outcome:?}");
        assert!(
            outcome
                .notes
                .iter()
                .any(|n| n.contains("未发现可判定的运行期信号")),
            "{outcome:?}"
        );
    }

    /// 外部 CDN 加载失败只记提示(离线环境的网络问题不是交付缺陷)。
    #[test]
    fn evaluate_treats_external_resource_failure_as_note() {
        let rep = report(
            r#"{"errors":[],"resourceErrors":["https://cdn.example.com/three.min.js"],
                "canvases":1,"three":false,"namespaces":[]}"#,
        );
        let outcome = evaluate(&rep, "写一个页面");
        assert!(outcome.issues.is_empty(), "{outcome:?}");
        assert!(
            outcome.notes.iter().any(|n| n.contains("外部资源加载失败")),
            "{outcome:?}"
        );
    }

    /// 问题条数有上限,超出部分折成提示(不无限拉长 error 文案)。
    #[test]
    fn evaluate_caps_issue_count() {
        let mut names = Vec::new();
        for i in 0..12 {
            names.push(format!(
                r#"{{"name":"App{i}","ready":false,"sceneChildren":0,"mapSize":0}}"#
            ));
        }
        let rep = report(&format!(
            r#"{{"errors":[],"canvases":1,"namespaces":[{}]}}"#,
            names.join(",")
        ));
        let outcome = evaluate(&rep, "3D 场景");
        assert_eq!(outcome.issues.len(), MAX_ISSUES);
        assert!(
            outcome.notes.iter().any(|n| n.contains("未展开")),
            "{outcome:?}"
        );
    }

    /// 探针副本的临时路径与注入后的行号不进入用户错误;真实文件位置保留。
    #[test]
    fn evaluate_hides_probe_copy_path_but_keeps_real_locations() {
        let rep = report(
            r#"{"errors":[
                {"message":"boom","file":"file:///C:/Users/x/AppData/Local/Temp/kedai-runtime-check-1-0/probe.html","line":30},
                {"message":"cdn boom","file":"https://cdn.example.com/app.js","line":7}],
                "canvases":1}"#,
        );
        let outcome = evaluate(&rep, "写一个页面");
        let joined = outcome.issues.join("\n");
        assert!(joined.contains("boom"), "{joined}");
        assert!(
            !joined.contains("kedai-runtime-check"),
            "不得泄露临时探针路径: {joined}"
        );
        assert!(
            joined.contains("cdn.example.com"),
            "真实文件位置应保留: {joined}"
        );
    }

    /// 真实浏览器手工用例(CI/测试构建不跑):
    /// `cargo test runtime_probe_real_browser -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "需要本机安装 Edge/Chrome,手工验收运行期自检"]
    async fn runtime_probe_real_browser() {
        let browser = browser_path().expect("本机应能找到 Edge/Chrome");
        let dir = std::env::temp_dir().join(format!("kedai-probe-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("创建手工用例目录失败");
        let healthy = r#"<!doctype html><html><head></head><body><canvas id="c" width="320" height="200"></canvas>
<script>
window.App={ready:false,scene:{children:[]},engine:{map:new Map()},renderer:{}};
window.App.scene.children.push({});
window.App.engine.map.set("0,0,0",1);
window.App.ready=true;
window.App.getState=function(){return {ready:true,sceneChildren:1,mapSize:1}};
</script></body></html>"#;
        std::fs::write(dir.join("ok.html"), healthy).expect("写健康用例失败");
        let outcome = probe_with(&browser, &dir, "ok.html", "3D 场景").await;
        assert!(outcome.verified, "{outcome:?}");
        assert!(outcome.issues.is_empty(), "{outcome:?}");

        let broken = r#"<!doctype html><html><head></head><body><script>
window.App={ready:false,scene:{children:[]},engine:{map:new Map()},renderer:null};
window.App.getState=function(){return {ready:false,sceneChildren:0,mapSize:0}};
</script></body></html>"#;
        std::fs::write(dir.join("bad.html"), broken).expect("写坏用例失败");
        let outcome = probe_with(&browser, &dir, "bad.html", "3D 场景").await;
        assert!(outcome.verified, "{outcome:?}");
        assert!(!outcome.issues.is_empty(), "{outcome:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn encode_uri_component(text: &str) -> String {
        let mut out = String::new();
        for byte in text.bytes() {
            match byte {
                b'A'..=b'Z'
                | b'a'..=b'z'
                | b'0'..=b'9'
                | b'-'
                | b'.'
                | b'_'
                | b'~'
                | b'!'
                | b'*'
                | b'\''
                | b'('
                | b')' => out.push(byte as char),
                _ => out.push_str(&format!("%{byte:02X}")),
            }
        }
        out
    }
}
