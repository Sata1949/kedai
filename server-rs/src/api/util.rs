// 路由子模块共享的响应工具(自 api/mod.rs 迁入,路径经 mod.rs 再导出保持不变)
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response, Sse};

/// 便捷:带状态码的 JSON 响应(各路由子模块 use super::WithStatus)
pub trait WithStatus {
    fn with_status(self, code: StatusCode) -> Response;
}

impl WithStatus for Response {
    fn with_status(mut self, code: StatusCode) -> Response {
        *self.status_mut() = code;
        self
    }
}

/// DB 阻塞任务失败统一 500 响应(2026-08 DB 并发改造:
/// spawn_blocking JoinError / 连接池错误 / 服务内 String 错误);
/// 带结构化错误码 code: "DB"(见 errors.rs)。
pub(crate) fn db_err(e: &str) -> Response {
    super::err_with_code(super::ErrorCode::Db, e, StatusCode::INTERNAL_SERVER_ERROR)
}

/// SSE 响应统一装配(单点):KeepAlive 30s + `Cache-Control: no-cache, no-transform`。
///
/// **收敛背景(2026-09-14)**:`/api/chat/send`(api/chat.rs)与 `/api/tasks/events`
/// (api/tasks.rs)此前各自逐行复制这段装配。两处必须一致——`no-transform` 防中间层
/// 缓冲/压缩破坏 SSE 分帧,KeepAlive 防长空闲被代理断开;任何一处漏配都会出现
/// 「任务流正常但聊天流卡住」这类难查的不对称故障。故收敛为单点。
///
/// 入参为**已构造好的事件流**,调用方只负责产生事件(两种流的产生方式不同:
/// chat 是 mpsc 接收、tasks 是 broadcast 订阅并处理 Lagged),装配本身不再重复。
pub(crate) fn sse_response<S>(stream: S) -> Response
where
    S: futures::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>
        + Send
        + 'static,
{
    let mut response = Sse::new(stream)
        .keep_alive(
            axum::response::sse::KeepAlive::new().interval(std::time::Duration::from_secs(30)),
        )
        .into_response();
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-cache, no-transform"),
    );
    response
}
