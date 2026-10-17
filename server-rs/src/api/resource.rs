// 角色卡远程资源界面代理:服务端 fetch 作者提供的 HTML 页面(下载资源界面),
// 供前端 srcdoc 沙箱 iframe 渲染。
//
// 背景:原版酒馆助手用 `$('body').load('https://…')` 把作者页面内容注入 body 显示;
// kedai 不直接注入远程脚本,改为「服务端 fetch → srcdoc 沙箱」:
//   - 绕开浏览器跨域限制(CORS)与 X-Frame-Options(作者服务器禁止被 iframe 嵌入时,
//     iframe src 直嵌会显示「已阻止此内容」,服务端 fetch 不受此限制)
//   - 安全:仅 https、SSRF 防护(复用 resolve_public_http_url:拒绝 localhost/私网/
//     元数据地址)、绑定已解析 IP 防 DNS rebinding、禁用重定向、响应大小上限
//     (KEDAI-06 起为分块流式限额:读取中累计,超限即中止,见 read_body_bounded)
use crate::api::{err_with_code, ErrorCode};
use crate::tools::agent_tools::resolve_public_http_url;
use axum::extract::Query;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

/// 资源页大小上限(HTML 文本;超过拒绝,防内存压力)
const MAX_RESOURCE_BYTES: usize = 5 * 1024 * 1024;

#[derive(Deserialize)]
pub struct ProxyQuery {
    pub url: String,
}

/// 按声明编码解码资源页 HTML。
/// from_utf8_lossy 对 GBK/GB2312 等中文编码的作者页面会整页乱码(替换字符铺满),
/// 这里按 content-type charset → <meta charset> 嗅探 → UTF-8 兜底的顺序选择编码。
fn decode_resource_html(bytes: &[u8], content_type: Option<&str>) -> String {
    let label = content_type
        .and_then(charset_from_content_type)
        .or_else(|| sniff_meta_charset(bytes));
    let encoding = label
        .as_deref()
        .and_then(|l| encoding_rs::Encoding::for_label(l.as_bytes()))
        .unwrap_or(encoding_rs::UTF_8);
    encoding.decode(bytes).0.into_owned()
}

/// 解析 content-type 中的 charset 参数(text/html; charset=gbk)
fn charset_from_content_type(content_type: &str) -> Option<String> {
    for part in content_type.split(';').skip(1) {
        let (k, v) = part.split_once('=')?;
        if k.trim().eq_ignore_ascii_case("charset") {
            let label = v.trim().trim_matches('"').trim_matches('\'');
            if !label.is_empty() {
                return Some(label.to_string());
            }
        }
    }
    None
}

/// 嗅探前 4KB 里的 <meta charset="..."> / <meta http-equiv content="...charset=...">
fn sniff_meta_charset(bytes: &[u8]) -> Option<String> {
    let head_len = bytes.len().min(4096);
    let head = String::from_utf8_lossy(&bytes[..head_len]).to_lowercase();
    let idx = head.find("charset")?;
    let after = &head[idx + "charset".len()..];
    let after = after.trim_start();
    let after = after.strip_prefix('=')?;
    let after = after.trim_start();
    let after = after.trim_start_matches(['"', '\'']);
    let end = after
        .find(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_' && c != ':')
        .unwrap_or(after.len());
    let label = after[..end].trim();
    if label.is_empty() {
        None
    } else {
        Some(label.to_string())
    }
}
/// 有界读取响应体(KEDAI-06):按分块累计**实际接收字节**,超限立即停止读取并丢弃响应;
/// 超限分块不再复制进缓冲(此前 `.bytes()` 全量缓冲后才比 `MAX_RESOURCE_BYTES`,
/// 5MB 上限约束不了下载过程的峰值内存)。
///
/// - `Content-Length` 已声明超限(> limit):提前拒绝,不启动读取;
///   缺失/失真/分块传输一律按实际字节计数——声明值不能替代限额。
/// - 边界:恰好 `limit` 字节通过,`limit + 1` 拒绝(与旧 `> MAX` 判定同口径)。
/// - 读取错误(连接中断/超时)原样上抛,由调用方按原路径记日志 + 502。
async fn read_body_bounded(
    mut resp: reqwest::Response,
    limit: usize,
) -> Result<Option<Vec<u8>>, reqwest::Error> {
    let declared = resp.content_length();
    if let Some(len) = declared {
        if len > limit as u64 {
            return Ok(None);
        }
    }
    // 预分配按声明长度(此刻已保证 ≤ limit);缺失时从 0 起
    let mut buf: Vec<u8> = Vec::with_capacity(declared.unwrap_or(0).min(limit as u64) as usize);
    while let Some(chunk) = resp.chunk().await? {
        let needed = match buf.len().checked_add(chunk.len()) {
            Some(n) if n <= limit => n,
            _ => return Ok(None),
        };
        if buf.capacity() < needed {
            // 扩容以上限封顶:几何增长不得越出约定上限
            let target = needed.saturating_mul(2).min(limit).max(needed);
            buf.reserve_exact(target - buf.len());
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(Some(buf))
}

/// GET /api/resource/proxy?url=<https URL>
/// 成功 `{ ok: true, html, base_url }`;失败 `{ ok: false, error, code }` + 真实状态码
/// (批次 1 起统一带 code,不再出现「无 code 的裸 error」)。
pub async fn proxy(Query(q): Query<ProxyQuery>) -> Response {
    let raw = q.url.trim();
    // 仅 https(作者资源页强制加密传输;http/javascript:/data: 一律拒绝)
    let parsed = match reqwest::Url::parse(raw) {
        Ok(u) if u.scheme() == "https" && u.host_str().is_some() => u,
        _ => {
            return err_with_code(
                ErrorCode::Validation,
                "仅支持 https 资源地址",
                StatusCode::BAD_REQUEST,
            );
        }
    };
    // SSRF 防护:拒绝 localhost / 私网 / 元数据地址
    let target = match resolve_public_http_url(raw).await {
        Ok(t) => t,
        Err(e) => {
            // 泄露封堵(批次 1):该解析器为**搜索工具**编写,错误串里既有「搜索端点」
            // 这类错域措辞,DNS 失败分支还会拼 OS 原文(os error 11001),两者都不该
            // 出现在资源卡片的用户提示里。原文只进日志,回给用户稳定文案。
            tracing::warn!(error = %e, url = raw, "资源地址解析被拒");
            return err_with_code(
                ErrorCode::Validation,
                "资源地址不可用,请确认其为公开可访问的 https 地址",
                StatusCode::BAD_REQUEST,
            );
        }
    };
    // 绑定已解析 IP(防 DNS rebinding)+ 禁用重定向(防跳到本地)+ 短超时
    let mut builder = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) Kedai/0.2");
    for address in &target.addresses {
        builder = builder.resolve(&target.host, *address);
    }
    let client = match builder.build() {
        Ok(c) => c,
        Err(e) => {
            // 泄露封堵:reqwest 构建错误原文只进日志
            tracing::error!(error = %e, "构建资源代理请求客户端失败");
            return err_with_code(
                ErrorCode::Internal,
                "构建请求客户端失败,详情见服务端日志",
                StatusCode::INTERNAL_SERVER_ERROR,
            );
        }
    };
    let resp = match client.get(raw).send().await {
        Ok(r) => r,
        Err(e) => {
            // 泄露封堵:上游连接错误原文(含 IP/端口/证书细节)只进日志
            tracing::error!(error = %e, url = raw, "请求资源页失败");
            return err_with_code(
                ErrorCode::Upstream,
                "请求资源页失败,请稍后重试",
                StatusCode::BAD_GATEWAY,
            );
        }
    };
    if resp.status().is_redirection() {
        return err_with_code(
            ErrorCode::Upstream,
            "资源页重定向已禁用",
            StatusCode::BAD_GATEWAY,
        );
    }
    if !resp.status().is_success() {
        // 上游状态码属用户可理解信息(404/403 等),保留但不带响应正文
        return err_with_code(
            ErrorCode::Upstream,
            format!("资源页返回 {}", resp.status()),
            StatusCode::BAD_GATEWAY,
        );
    }
    // content-type 在读取消费 resp 前取出(charset 解码用)
    let resp_content_type = resp
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    // KEDAI-06:有界读取(分块累计,超限立即中止)——不再全量缓冲后才校验
    let bytes = match read_body_bounded(resp, MAX_RESOURCE_BYTES).await {
        Ok(Some(b)) => b,
        Ok(None) => {
            return err_with_code(
                ErrorCode::Upstream,
                "资源页过大(超过 5MB)",
                StatusCode::BAD_GATEWAY,
            );
        }
        Err(e) => {
            // 泄露封堵:读取中断原文只进日志
            tracing::error!(error = %e, url = raw, "读取资源页失败");
            return err_with_code(
                ErrorCode::Upstream,
                "读取资源页失败,请稍后重试",
                StatusCode::BAD_GATEWAY,
            );
        }
    };
    let html = decode_resource_html(&bytes, resp_content_type.as_deref());
    // 作者域名作为 base(srcdoc 内相对 CSS/JS/图片按作者域名解析)
    let base_url = format!(
        "{}://{}",
        parsed.scheme(),
        parsed.host_str().unwrap_or_default()
    );
    Json(json!({ "ok": true, "html": html, "base_url": base_url })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_charset_from_content_type() {
        assert_eq!(
            charset_from_content_type("text/html; charset=gbk").as_deref(),
            Some("gbk")
        );
        assert_eq!(
            charset_from_content_type("text/html; charset=\"GB2312\"").as_deref(),
            Some("GB2312")
        );
        assert_eq!(charset_from_content_type("text/html"), None);
    }

    #[test]
    fn sniffs_meta_charset() {
        let head = br#"<html><head><meta charset="gbk"></head>"#;
        assert_eq!(sniff_meta_charset(head).as_deref(), Some("gbk"));
        let equiv = br#"<meta http-equiv="Content-Type" content="text/html; charset=GB18030">"#;
        assert_eq!(sniff_meta_charset(equiv).as_deref(), Some("gb18030"));
        assert_eq!(sniff_meta_charset(b"<html><head></head>"), None);
    }

    #[test]
    fn decodes_gbk_instead_of_lossy_mojibake() {
        // GBK 编码的「中文」二字;from_utf8_lossy 会产生替换字符乱码
        let (gbk, _, _) = encoding_rs::GBK.encode("中文页面");
        assert_eq!(
            decode_resource_html(&gbk, Some("text/html; charset=gbk")),
            "中文页面"
        );
        // 无 content-type 时走 meta 嗅探
        let mut html = b"<meta charset=gbk>".to_vec();
        html.extend_from_slice(&gbk);
        assert_eq!(
            decode_resource_html(&html, None),
            "<meta charset=gbk>中文页面"
        );
        // UTF-8 默认兜底
        assert_eq!(
            decode_resource_html("中文页面".as_bytes(), None),
            "中文页面"
        );
    }

    // ---- KEDAI-06 有界读取(评审报告批 E;先红后绿) ----
    //
    // 测试经**本机原始 TCP 上游**注入传输层(报告 §八要求:不为测试放开生产 SSRF 校验,
    // 直接把响应喂给 read_body_bounded),覆盖:低于/恰好/超过上限、缺 Content-Length、
    // 声明超限预拒、chunked、截断报错、慢响应超时。

    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// 极简一次性 HTTP 上游:应答给定原始字节;`hold_open` 为真时写完后挂住连接
    /// (用于断言「超限立即停止」不依赖等 EOF / 等终端块)。
    async fn spawn_upstream(response: Vec<u8>, hold_open: bool) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            // 读掉请求头即止,不解析
            let mut seen = Vec::new();
            let mut buf = [0u8; 1024];
            while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
                match sock.read(&mut buf).await {
                    Ok(0) | Err(_) => return,
                    Ok(n) => seen.extend_from_slice(&buf[..n]),
                }
            }
            if sock.write_all(&response).await.is_err() {
                return;
            }
            let _ = sock.flush().await;
            if hold_open {
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
        });
        format!("http://127.0.0.1:{}/", addr.port())
    }

    fn raw_response(head: &str, body: &[u8]) -> Vec<u8> {
        let mut out = head.as_bytes().to_vec();
        out.extend_from_slice(body);
        out
    }

    async fn fetch(url: &str) -> reqwest::Response {
        reqwest::Client::new()
            .get(url)
            .send()
            .await
            .expect("本地测试上游应可连通")
    }

    #[tokio::test]
    async fn bounded_read_ok_below_limit() {
        let body = vec![b'a'; 1024];
        let url = spawn_upstream(
            raw_response(
                "HTTP/1.1 200 OK\r\nContent-Length: 1024\r\nConnection: close\r\n\r\n",
                &body,
            ),
            false,
        )
        .await;
        let got = read_body_bounded(fetch(&url).await, 4096).await.unwrap();
        assert_eq!(got.map(|b| b.len()), Some(1024), "低于上限应完整读取");
    }

    #[tokio::test]
    async fn bounded_read_accepts_exactly_limit() {
        let body = vec![b'a'; 4096];
        let url = spawn_upstream(
            raw_response(
                "HTTP/1.1 200 OK\r\nContent-Length: 4096\r\nConnection: close\r\n\r\n",
                &body,
            ),
            false,
        )
        .await;
        let got = read_body_bounded(fetch(&url).await, 4096).await.unwrap();
        assert_eq!(
            got.map(|b| b.len()),
            Some(4096),
            "恰好等于上限应通过(边界含等号,与旧 `> MAX` 判定同口径)"
        );
    }

    #[tokio::test]
    async fn bounded_read_rejects_over_limit_without_content_length() {
        // 无 Content-Length(close 定界),体为上限+1 且**不关连接**:
        // 若实现等 EOF 才判定,本测试会超时失败
        let body = vec![b'a'; 4097];
        let url = spawn_upstream(
            raw_response("HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n", &body),
            true,
        )
        .await;
        let got = tokio::time::timeout(
            Duration::from_secs(5),
            read_body_bounded(fetch(&url).await, 4096),
        )
        .await
        .expect("超限应立即返回,不应等待连接关闭");
        assert_eq!(got.unwrap(), None, "无声明长度时仍按实际字节计数拒绝");
    }

    #[tokio::test]
    async fn bounded_read_rejects_declared_oversize_early() {
        // 声明超限:只发响应头,体一个字节都不发且连接挂住——提前拒绝不得启动读取
        let url = spawn_upstream(
            raw_response(
                "HTTP/1.1 200 OK\r\nContent-Length: 9999999\r\nConnection: close\r\n\r\n",
                b"",
            ),
            true,
        )
        .await;
        let got = tokio::time::timeout(
            Duration::from_secs(5),
            read_body_bounded(fetch(&url).await, 4096),
        )
        .await
        .expect("声明超限应立即拒绝,不应等待响应体");
        assert_eq!(got.unwrap(), None);
    }

    #[tokio::test]
    async fn bounded_read_chunked_within_limit_ok() {
        // Transfer-Encoding: chunked(无 Content-Length),恰好上限 → 通过
        let mut resp =
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n".to_vec();
        resp.extend_from_slice(b"1000\r\n");
        resp.extend_from_slice(&[b'a'; 0x1000]);
        resp.extend_from_slice(b"\r\n0\r\n\r\n");
        let url = spawn_upstream(resp, false).await;
        let got = read_body_bounded(fetch(&url).await, 4096).await.unwrap();
        assert_eq!(got.map(|b| b.len()), Some(4096), "分块传输恰好上限应通过");
    }

    #[tokio::test]
    async fn bounded_read_chunked_over_limit_rejected() {
        // 分块累计超限(4000+97):不写 0 终止块、挂住连接——
        // 若实现等完整终止才判,本测试会超时
        let mut resp =
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n".to_vec();
        resp.extend_from_slice(b"fa0\r\n"); // 4000
        resp.extend_from_slice(&[b'a'; 4000]);
        resp.extend_from_slice(b"\r\n61\r\n"); // 97
        resp.extend_from_slice(&[b'a'; 97]);
        resp.extend_from_slice(b"\r\n");
        let url = spawn_upstream(resp, true).await;
        let got = tokio::time::timeout(
            Duration::from_secs(5),
            read_body_bounded(fetch(&url).await, 4096),
        )
        .await
        .expect("分块累计超限应立即拒绝");
        assert_eq!(got.unwrap(), None);
    }

    #[tokio::test]
    async fn bounded_read_surfaces_stream_error_on_truncated_body() {
        // 声明 100 字节只发 10 字节即断开:错误上抛(调用方走 502 路径),不静默成短读
        let url = spawn_upstream(
            raw_response(
                "HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n",
                &[b'a'; 10],
            ),
            false,
        )
        .await;
        let got = read_body_bounded(fetch(&url).await, 4096).await;
        assert!(got.is_err(), "截断响应必须报错而非静默返回短内容");
    }

    #[tokio::test]
    async fn bounded_read_surfaces_timeout_error() {
        // 慢响应:头发完只滴 8 字节后挂住;客户端总超时 300ms——读取必须以错误收场(超时仍生效)
        let url = spawn_upstream(
            raw_response("HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n", &[b'a'; 8]),
            true,
        )
        .await;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(300))
            .build()
            .unwrap();
        let resp = client.get(&url).send().await.unwrap();
        let got = tokio::time::timeout(Duration::from_secs(5), read_body_bounded(resp, 4096))
            .await
            .expect("超时应及时生效而不是挂死");
        assert!(got.is_err(), "慢响应超时应上抛错误");
    }
}
