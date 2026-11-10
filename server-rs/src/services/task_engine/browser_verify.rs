//! 隔离 Chromium + 工作区 HTTP 入口；模块/资源沿真实相对路径加载，截图走图像服务。
use super::verification::BrowserAction;
use crate::models::types::ImageRef;
use crate::services::image_service::ImageService;
use axum::{
    extract::{Path as UrlPath, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Router,
};
use base64::Engine as _;
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_tungstenite::{tungstenite::Message, MaybeTlsStream, WebSocketStream};

#[derive(Debug)]
pub(crate) struct BrowserEvidence {
    pub checks: Vec<String>,
    pub issues: Vec<String>,
    pub screenshots: Vec<ImageRef>,
    pub resources: Vec<(String, String)>,
}

struct ProbeProfile(PathBuf);
impl ProbeProfile {
    fn new_in(parent: &Path) -> Result<Self, String> {
        let path = parent.join(format!("kedai-browser-verify-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).map_err(|e| format!("创建浏览器验收目录失败：{e}"))?;
        Ok(Self(path))
    }
}
impl Drop for ProbeProfile {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_dir_all(&self.0) {
            tracing::warn!("浏览器验收临时目录清理失败：{e}");
        }
    }
}

struct BrowserProcess {
    child: tokio::process::Child,
    #[cfg(windows)]
    job: crate::services::exec::JobTree,
    _profile: ProbeProfile,
}

struct ProbeResources {
    process: Arc<tokio::sync::Mutex<BrowserProcess>>,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for ProbeResources {
    fn drop(&mut self) {
        self.server.abort();
        let process = self.process.clone();
        // 清理任务持有完整进程资源，先终止/回收浏览器，再由 RAII 删除已释放句柄的 profile。
        // 取消 drop 时不从结构体逐个取 Option，进程与目录的所有权不会拆散。
        tokio::spawn(async move {
            let mut browser = process.lock().await;
            #[cfg(windows)]
            browser.job.terminate();
            if let Err(error) = browser.child.start_kill() {
                tracing::warn!(%error, "浏览器验收进程终止失败");
            }
            match tokio::time::timeout(Duration::from_secs(5), browser.child.wait()).await {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => tracing::warn!(%error, "浏览器验收进程回收失败"),
                Err(_) => tracing::warn!("浏览器验收进程回收超时"),
            }
        });
    }
}

#[derive(Default)]
struct ServedResources {
    hashes: BTreeMap<String, String>,
    changed: BTreeSet<String>,
}
struct FileServerState {
    root: PathBuf,
    resources: Mutex<ServedResources>,
}
async fn serve_file(
    State(state): State<Arc<FileServerState>>,
    UrlPath(path): UrlPath<String>,
) -> Response {
    let root = &state.root;
    let resolved = match tokio::fs::canonicalize(root.join(path)).await {
        Ok(p) => p,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };
    let Ok(relative) = resolved.strip_prefix(root) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !tokio::fs::metadata(&resolved)
        .await
        .is_ok_and(|m| m.is_file() && m.len() <= 32 * 1024 * 1024)
    {
        return StatusCode::NOT_FOUND.into_response();
    }
    let mime = match resolved
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "svg" => "image/svg+xml",
        "wasm" => "application/wasm",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    };
    match tokio::fs::read(&resolved).await {
        Ok(bytes) => {
            let relative = relative.to_string_lossy().replace('\\', "/");
            let digest = format!("{:x}", Sha256::digest(&bytes));
            let mut resources = state.resources.lock().unwrap_or_else(|e| e.into_inner());
            match resources.hashes.get(&relative) {
                Some(first) if first != &digest => {
                    resources.changed.insert(relative);
                }
                Some(_) => {}
                None => {
                    resources.hashes.insert(relative, digest);
                }
            }
            drop(resources);
            (
                [
                    (header::CONTENT_TYPE, mime),
                    (header::CACHE_CONTROL, "no-store"),
                ],
                bytes,
            )
                .into_response()
        }
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

struct Cdp {
    socket: WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    next: u64,
}
impl Cdp {
    async fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.next += 1;
        let id = self.next;
        self.socket
            .send(Message::Text(
                json!({"id":id,"method":method,"params":params})
                    .to_string()
                    .into(),
            ))
            .await
            .map_err(|e| format!("浏览器调试发送失败：{e}"))?;
        while let Some(message) = self.socket.next().await {
            let message = message.map_err(|e| format!("浏览器调试读取失败：{e}"))?;
            if let Message::Text(text) = message {
                let value: Value =
                    serde_json::from_str(&text).map_err(|e| format!("浏览器调试响应无效：{e}"))?;
                if value["id"] == id {
                    return if value.get("error").is_some() {
                        Err(format!("浏览器 {method} 失败：{}", value["error"]))
                    } else {
                        Ok(value["result"].clone())
                    };
                }
            }
        }
        Err("浏览器调试连接已关闭".into())
    }
    async fn evaluate(&mut self, expression: String) -> Result<Value, String> {
        let result = self
            .call(
                "Runtime.evaluate",
                json!({"expression":expression,"returnByValue":true,"awaitPromise":true}),
            )
            .await?;
        if result.get("exceptionDetails").is_some() {
            return Err(format!(
                "浏览器验收脚本异常：{}",
                result["exceptionDetails"]
            ));
        }
        Ok(result["result"]["value"].clone())
    }
    async fn screenshot(&mut self, images: &ImageService, name: &str) -> Result<ImageRef, String> {
        let result = self
            .call(
                "Page.captureScreenshot",
                json!({"format":"png","captureBeyondViewport":false}),
            )
            .await?;
        let encoded = result["data"].as_str().ok_or("浏览器截图没有图像数据")?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|e| format!("浏览器截图解码失败：{e}"))?;
        images.save(name, "image/png", &bytes)
    }
}

pub(crate) async fn probe(
    workspace: &Path,
    entry: &str,
    goal: &str,
    actions: &[BrowserAction],
    screenshot: bool,
    images: &ImageService,
) -> Result<BrowserEvidence, String> {
    let browser = super::runtime_check::browser_path()
        .ok_or("未找到 Chromium/Edge，浏览器验收不可用；可通过 KEDAI_HEADLESS_BROWSER 指定")?;
    tokio::time::timeout(
        Duration::from_secs(45),
        probe_with(
            &browser, workspace, entry, goal, actions, screenshot, images,
        ),
    )
    .await
    .map_err(|_| "浏览器验收超过 45 秒，未完成检查".to_string())?
}

pub(crate) async fn probe_with(
    browser: &Path,
    workspace: &Path,
    entry: &str,
    goal: &str,
    actions: &[BrowserAction],
    screenshot: bool,
    images: &ImageService,
) -> Result<BrowserEvidence, String> {
    let profile = ProbeProfile::new_in(&std::env::temp_dir())?;
    probe_in_profile(
        browser, workspace, entry, goal, actions, screenshot, images, profile,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn probe_in_profile(
    browser: &Path,
    workspace: &Path,
    entry: &str,
    goal: &str,
    actions: &[BrowserAction],
    screenshot: bool,
    images: &ImageService,
    profile: ProbeProfile,
) -> Result<BrowserEvidence, String> {
    let root = std::fs::canonicalize(workspace).map_err(|e| format!("验收工作区不可用：{e}"))?;
    let target =
        std::fs::canonicalize(root.join(entry)).map_err(|e| format!("验收入口不可用：{e}"))?;
    if !target.starts_with(&root) || !target.is_file() {
        return Err("验收入口必须是工作区内文件".into());
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| format!("验收 HTTP 服务启动失败：{e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let state = Arc::new(FileServerState {
        root,
        resources: Mutex::new(ServedResources::default()),
    });
    let app = Router::new()
        .route("/{*path}", axum::routing::get(serve_file))
        .with_state(state.clone());
    #[cfg(windows)]
    let job =
        crate::services::exec::JobTree::new().ok_or("验收浏览器无法创建进程树，取消清理无保证")?;
    let mut command = tokio::process::Command::new(browser);
    command
        .args([
            "--headless=new",
            "--remote-debugging-port=0",
            "--remote-debugging-address=127.0.0.1",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-extensions",
            "--disable-background-networking",
            "--disable-component-update",
            "--disable-sync",
            "--enable-unsafe-swiftshader",
            "--use-angle=swiftshader",
            "--window-size=1280,800",
        ])
        .arg(format!("--user-data-dir={}", profile.0.display()))
        .arg("about:blank")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let child = command
        .spawn()
        .map_err(|e| format!("启动验收浏览器失败：{e}"))?;
    let server = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!("验收文件服务异常：{e}");
        }
    });
    let port_file = profile.0.join("DevToolsActivePort");
    let resources = ProbeResources {
        process: Arc::new(tokio::sync::Mutex::new(BrowserProcess {
            child,
            _profile: profile,
            #[cfg(windows)]
            job,
        })),
        server,
    };
    #[cfg(windows)]
    {
        let browser = resources.process.lock().await;
        let handle = browser
            .child
            .raw_handle()
            .ok_or("验收浏览器已退出，无法取得进程句柄")?;
        if !browser.job.assign(handle as _) {
            return Err("验收浏览器无法加入进程树，取消清理无保证".into());
        }
    }
    let debug_port = loop {
        if let Ok(text) = tokio::fs::read_to_string(&port_file).await {
            if let Some(port) = text.lines().next().and_then(|s| s.parse::<u16>().ok()) {
                break port;
            }
        }
        if let Some(status) = resources
            .process
            .lock()
            .await
            .child
            .try_wait()
            .map_err(|e| e.to_string())?
        {
            return Err(format!("验收浏览器启动即退出：{status}"));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let pages: Value = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{debug_port}/json/list"))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    let ws = pages
        .as_array()
        .and_then(|list| list.iter().find(|p| p["type"] == "page"))
        .and_then(|p| p["webSocketDebuggerUrl"].as_str())
        .ok_or("浏览器没有可验收页面")?;
    let (socket, _) = tokio_tungstenite::connect_async(ws)
        .await
        .map_err(|e| e.to_string())?;
    let mut cdp = Cdp { socket, next: 0 };
    cdp.call("Page.enable", json!({})).await?;
    cdp.call(
        "Page.addScriptToEvaluateOnNewDocument",
        json!({"source":super::runtime_check::install_script()}),
    )
    .await?;
    let mut url =
        reqwest::Url::parse(&format!("http://127.0.0.1:{port}/")).map_err(|e| e.to_string())?;
    url.set_path(entry);
    cdp.call("Page.navigate", json!({"url":url.as_str()}))
        .await?;
    loop {
        if cdp
            .evaluate("document.readyState==='complete' && !!window.__kv".into())
            .await?
            == true
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // 隔离验收只获准操作本工作区页面；跨站跳转不能扩大声明式交互的范围。
    let origin = url.origin().ascii_serialization();
    if cdp.evaluate("location.origin".into()).await? != origin {
        return Err("验收入口跨站跳转，未执行外部页面交互".into());
    }
    cdp.evaluate("new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(()=>setTimeout(()=>r(true),150))))".into()).await?;
    let mut evidence = BrowserEvidence {
        checks: vec![format!("HTTP 入口 {entry} 完成加载及两帧渲染")],
        issues: vec![],
        screenshots: vec![],
        resources: vec![],
    };
    if screenshot {
        evidence
            .screenshots
            .push(cdp.screenshot(images, "入口加载.png").await?);
    }
    for action in actions {
        if cdp.evaluate("location.origin".into()).await? != origin {
            return Err("验收过程中跨站跳转，已停止后续交互".into());
        }
        let (selector, description) = match action {
            BrowserAction::Click { selector } => (selector, format!("点击 {selector}")),
            BrowserAction::Fill { selector, .. } => (selector, format!("填写 {selector}")),
            BrowserAction::AssertText { selector, contains } => {
                (selector, format!("断言 {selector} 包含 {contains}"))
            }
            BrowserAction::AssertVisible { selector } => {
                (selector, format!("断言 {selector} 可见"))
            }
        };
        let select = serde_json::to_string(selector).map_err(|e| e.to_string())?;
        let node = format!("document.querySelector({select})");
        let result=match action {
            BrowserAction::Click{..}=>{
                let rect=cdp.evaluate(format!("(()=>{{const e={node};if(!e)return null;const r=e.getBoundingClientRect();return r.width>0&&r.height>0?{{x:r.x+r.width/2,y:r.y+r.height/2}}:null}})()")).await?;
                if rect.is_null(){false}else{for kind in ["mousePressed","mouseReleased"]{cdp.call("Input.dispatchMouseEvent",json!({"type":kind,"x":rect["x"],"y":rect["y"],"button":"left","clickCount":1})).await?;}true}
            },
            BrowserAction::Fill{value,..}=>{let found=cdp.evaluate(format!("(()=>{{const e={node};if(!e||!(e instanceof HTMLInputElement||e instanceof HTMLTextAreaElement))return false;e.focus();e.value='';e.dispatchEvent(new Event('input',{{bubbles:true}}));return true}})()")).await?;
                if found==true{cdp.call("Input.insertText",json!({"text":value})).await?;true}else{false}},
            BrowserAction::AssertText{contains,..}=>{let text=serde_json::to_string(contains).map_err(|e|e.to_string())?;cdp.evaluate(format!("(()=>{{const e={node};return !!e&&e.textContent.includes({text})}})()")).await?==true},
            BrowserAction::AssertVisible{..}=>cdp.evaluate(format!("(()=>{{const e={node};if(!e)return false;const r=e.getBoundingClientRect();const s=getComputedStyle(e);return r.width>0&&r.height>0&&s.visibility!=='hidden'&&s.display!=='none'}})()")).await?==true,
        };
        if result {
            evidence.checks.push(description);
        } else {
            evidence
                .issues
                .push(format!("交互/断言失败：{description}"));
            break;
        }
        cdp.evaluate(
            "new Promise(r=>requestAnimationFrame(()=>setTimeout(()=>r(true),50)))".into(),
        )
        .await?;
    }
    let sampled = cdp.evaluate(super::runtime_check::sample_script()).await?;
    // CDN 不可达仍走原环境提示；本 HTTP 根的模块/资源缺失是交付错误，不能只记提示。
    evidence
        .issues
        .extend(local_resource_failures(&sampled, &origin));
    let report: super::runtime_check::ProbeReport =
        serde_json::from_value(sampled).map_err(|e| format!("运行期采样报告无效：{e}"))?;
    let runtime = super::runtime_check::evaluate(&report, goal);
    evidence.issues.extend(runtime.issues);
    evidence.checks.extend(runtime.notes);
    if screenshot && !actions.is_empty() {
        evidence
            .screenshots
            .push(cdp.screenshot(images, "交互结束.png").await?);
    }
    cdp.call("Browser.close", json!({})).await?;
    resources
        .process
        .lock()
        .await
        .child
        .wait()
        .await
        .map_err(|e| e.to_string())?;
    let served = state.resources.lock().unwrap_or_else(|e| e.into_inner());
    evidence.resources = served
        .hashes
        .iter()
        .map(|(path, digest)| (path.clone(), digest.clone()))
        .collect();
    evidence.issues.extend(
        served
            .changed
            .iter()
            .map(|path| format!("验收过程中资源内容发生变化：{path}")),
    );
    Ok(evidence)
}

fn local_resource_failures(sample: &Value, origin: &str) -> Vec<String> {
    sample["resourceErrors"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|value| {
            let url = reqwest::Url::parse(value.as_str()?).ok()?;
            (url.origin().ascii_serialization() == origin)
                .then(|| format!("本地资源加载失败：{}", url.path()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::test_support::TempDataDir;

    #[test]
    fn local_resource_errors_do_not_confuse_other_origins() {
        let sample = json!({"resourceErrors":["http://127.0.0.1:3000/app.js","http://127.0.0.1:30001/other.js","https://cdn.example/site.js"]});
        assert_eq!(
            local_resource_failures(&sample, "http://127.0.0.1:3000"),
            vec!["本地资源加载失败：/app.js"]
        );
    }

    #[tokio::test]
    #[ignore = "需要本机 Chromium；真实浏览器验收单独留证"]
    async fn missing_local_resource_fails_even_if_body_visible() {
        let dir = TempDataDir::new("browser-missing-module");
        std::fs::write(
            dir.join("index.html"),
            "<html><body>页面壳<script type='module' src='./missing.mjs'></script></body></html>",
        )
        .unwrap();
        let images = ImageService::new(dir.join("images"));
        let browser = super::super::runtime_check::browser_path().expect("需要 Chromium");
        let actions = vec![BrowserAction::AssertVisible {
            selector: "body".into(),
        }];
        let result = tokio::time::timeout(
            Duration::from_secs(45),
            probe_with(
                &browser,
                dir.path(),
                "index.html",
                "网页",
                &actions,
                false,
                &images,
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            result
                .issues
                .iter()
                .any(|issue| issue.contains("本地资源加载失败")),
            "可见页面壳不能掩盖本地模块缺失：{result:?}"
        );
    }

    #[tokio::test]
    #[ignore = "需要本机 Chromium；真实浏览器验收单独留证"]
    async fn visible_body_does_not_hide_unready_runtime() {
        let dir = TempDataDir::new("browser-unready");
        std::fs::write(
            dir.join("index.html"),
            "<body>入口可见<script>window.Game={ready:false,renderer:null};</script></body>",
        )
        .unwrap();
        let browser = super::super::runtime_check::browser_path().expect("需要 Chromium");
        let images = ImageService::new(dir.join("images"));
        let actions = [BrowserAction::AssertVisible {
            selector: "body".into(),
        }];
        let out = tokio::time::timeout(
            Duration::from_secs(45),
            probe_with(
                &browser,
                dir.path(),
                "index.html",
                "3D 场景",
                &actions,
                false,
                &images,
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            out.issues.iter().any(|s| s.contains("ready=false")),
            "{:?}",
            out.issues
        );
        assert!(
            out.issues.iter().any(|s| s.contains("renderer 为 null")),
            "{:?}",
            out.issues
        );
    }

    #[tokio::test]
    async fn failed_browser_start_removes_profile_directory() {
        let dir = TempDataDir::new("browser-profile-failure");
        std::fs::write(dir.join("index.html"), "<body>ok</body>").unwrap();
        let profile = ProbeProfile::new_in(dir.path()).unwrap();
        let path = profile.0.clone();
        let result = probe_in_profile(
            &dir.join("missing-browser"),
            dir.path(),
            "index.html",
            "页面",
            &[],
            false,
            &ImageService::new(dir.join("images")),
            profile,
        )
        .await;
        assert!(result.unwrap_err().contains("启动验收浏览器失败"));
        assert!(!path.exists(), "启动失败不得遗留浏览器 profile");
    }

    #[tokio::test]
    async fn served_resources_keep_first_hash_and_report_changed_response() {
        let dir = TempDataDir::new("browser-resource-version");
        std::fs::write(dir.join("app.js"), "first").unwrap();
        let state = Arc::new(FileServerState {
            root: dir.path().canonicalize().unwrap(),
            resources: Mutex::new(ServedResources::default()),
        });
        assert_eq!(
            serve_file(State(state.clone()), UrlPath("app.js".into()))
                .await
                .status(),
            StatusCode::OK
        );
        std::fs::write(dir.join("app.js"), "second").unwrap();
        assert_eq!(
            serve_file(State(state.clone()), UrlPath("app.js".into()))
                .await
                .status(),
            StatusCode::OK
        );
        let served = state.resources.lock().unwrap();
        assert_eq!(
            served.hashes.get("app.js"),
            Some(&format!("{:x}", Sha256::digest(b"first")))
        );
        assert!(served.changed.contains("app.js"));
    }
    #[tokio::test]
    #[ignore = "需要本机 Chromium；真实浏览器验收单独留证"]
    async fn multi_file_click_and_screenshot_survive_probe_cleanup() {
        let dir = TempDataDir::new("browser-verify");
        let ws = dir.join("workspace");
        std::fs::create_dir(&ws).unwrap();
        std::fs::write(ws.join("index.html"),"<button id='go'>加一</button><output id='count'>0</output><script type='module' src='./app.js'></script>").unwrap();
        std::fs::write(ws.join("app.js"),"import {next} from './counter.js';document.querySelector('#go').onclick=()=>document.querySelector('#count').textContent=next();").unwrap();
        std::fs::write(ws.join("counter.js"), "export const next=()=>1;").unwrap();
        let images = ImageService::new(dir.join("data"));
        let actions = vec![
            BrowserAction::Click {
                selector: "#go".into(),
            },
            BrowserAction::AssertText {
                selector: "#count".into(),
                contains: "1".into(),
            },
        ];
        let browser = super::super::runtime_check::browser_path().expect("需要 Chromium");
        let out = tokio::time::timeout(
            Duration::from_secs(45),
            probe_with(
                &browser,
                &ws,
                "index.html",
                "计数页面",
                &actions,
                true,
                &images,
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(out.issues.is_empty(), "{:?}", out.issues);
        assert_eq!(out.screenshots.len(), 2);
        for name in ["index.html", "app.js", "counter.js"] {
            assert!(
                out.resources.iter().any(|(path, _)| path == name),
                "必须记录实际加载的入口与模块：{name}"
            );
        }
        for image in out.screenshots {
            assert!(images
                .resolve_data_url(&image.id)
                .unwrap()
                .starts_with("data:image/png;base64,"));
        }
        std::fs::write(ws.join("counter.js"), "export const next=()=>0;").unwrap();
        let failed = tokio::time::timeout(
            Duration::from_secs(45),
            probe_with(
                &browser,
                &ws,
                "index.html",
                "计数页面",
                &actions,
                false,
                &images,
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(failed.issues.iter().any(|s| s.contains("交互/断言失败")));
    }
}
