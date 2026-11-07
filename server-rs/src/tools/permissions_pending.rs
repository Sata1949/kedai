// 待授权调用登记（2026-11-06 三档授权；从 `permissions.rs` 拆出）。
//
// 职责：内存 pending 表 + oneshot 通道 + 断线快照。与聊天/任务的授权等待解耦——
// `round.rs` 只调 `begin_wait` / `cancel_wait`，resolve 端点只调 `resolve_wait`，
// 任务详情/快照端点只调 `pending_for_task`。
use crate::models::types::TaskToolAuthorization;
use crate::tools::permissions::{PendingAuthorizationDecision, ToolPermissionManager, ToolRisk};
use tokio::sync::oneshot;

pub(crate) struct PendingAuthorization {
    /// 待授权调用的完整描述（确认卡/快照/任务事件共用同一形状）。
    pub(crate) auth: TaskToolAuthorization,
    pub(crate) sender: oneshot::Sender<PendingAuthorizationDecision>,
}

/// 从任务会话 id 里取出任务 id。
///
/// 任务引擎给 LLM 的会话 id 有四种形态：`task:{id}`、`task:{id}:main:{n}`（团队并行）、
/// `task:{id}:{phase}`（审计）、`task:{id}:{phase}:{step}`（自定义步骤）。
/// 统一取 `task:` 之后的第一段——任务 id 是 UUID v4，不含冒号，故该切分无损。
pub(crate) fn task_id_of_session(session_id: &str) -> Option<&str> {
    let rest = session_id.strip_prefix("task:")?;
    match rest.split(':').next() {
        Some(id) if !id.is_empty() => Some(id),
        _ => None,
    }
}

impl ToolPermissionManager {
    /// 登记一次等待（键 = `(run_id, call_id)`，可并发多条）。
    #[allow(clippy::too_many_arguments)] // 待授权描述的各字段本身就是确认卡的列
    pub fn begin_wait(
        &self,
        run_id: &str,
        call_id: &str,
        session_id: &str,
        tool: &str,
        risk: ToolRisk,
        reason: &str,
        code: Option<&str>,
        input_preview: Option<&str>,
    ) -> oneshot::Receiver<PendingAuthorizationDecision> {
        let (sender, receiver) = oneshot::channel();
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                (run_id.into(), call_id.into()),
                PendingAuthorization {
                    auth: TaskToolAuthorization {
                        name: tool.into(),
                        risk,
                        reason: reason.into(),
                        run_id: run_id.into(),
                        call_id: call_id.into(),
                        session_id: session_id.into(),
                        code: code.map(str::to_string),
                        input_preview: input_preview.map(str::to_string),
                    },
                    sender,
                },
            );
        receiver
    }

    /// 某个任务当前所有待授权调用（断线/刷新恢复的权威快照）。
    ///
    /// 按 `call_id` 排序，保证同一批并发请求在 UI 上的顺序稳定；只返回 `task:` 前缀
    /// 会话的条目，聊天会话的待授权不会混进任务详情。
    pub fn pending_for_task(&self, task_id: &str) -> Vec<TaskToolAuthorization> {
        let pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        let mut items: Vec<_> = pending
            .values()
            .filter(|p| task_id_of_session(&p.auth.session_id) == Some(task_id))
            .map(|p| p.auth.clone())
            .collect();
        items.sort_by(|a, b| a.call_id.cmp(&b.call_id));
        items
    }

    /// 回填一次等待。会话/工具必须与登记时**精确**一致（防止跨任务/跨工具误放行）。
    pub fn resolve_wait(
        &self,
        run_id: &str,
        call_id: &str,
        session_id: &str,
        tool: &str,
        decision: PendingAuthorizationDecision,
    ) -> Result<(), String> {
        let key = (run_id.to_string(), call_id.to_string());
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        let Some(item) = pending.get(&key) else {
            return Err("待授权调用不存在或已结束".into());
        };
        if item.auth.session_id != session_id || item.auth.name != tool {
            return Err("待授权调用与会话或工具不匹配".into());
        }
        // 上方 get 已确认键存在且持锁期间无并发移除,remove 必然为 Some
        let item = pending.remove(&key).expect("键已确认存在,移除必然成功");
        // 有意丢弃 SendError:接收端已 drop 即原因本身(用户离开/会话结束),
        // 文案已等价表达,且此处决策值无需回传给调用方
        item.sender
            .send(decision)
            .map_err(|_| "待授权调用已断开".to_string())
    }

    /// 取消一次等待（超时/断线/任务 stop 后的清理；幂等）。
    pub fn cancel_wait(&self, run_id: &str, call_id: &str) {
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&(run_id.to_string(), call_id.to_string()));
    }
}
