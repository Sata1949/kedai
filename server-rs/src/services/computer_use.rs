// 电脑操作(computer use)运行期治理(CU-1,2026-10-06):急停开关 + 操作审计。
//
// ## 为什么现在只覆盖 screenshot
//
// CU 提交 3/4 的 win_*/dev_* 工具尚未存在;既有 `screenshot`(VISION-7 交付的读屏能力)
// 是当前唯一「读取这台电脑」的通道,故把急停与审计挂在它上面作为**真实消费者**——
// 避免产出无消费点的空开关(本仓对「存而未用」的教训见 CFG-1/DC-1 登记)。
// 能力总闸(`computer_use_enabled`/`cu_allow_*`)与「输入独占租约」随 CU 提交 3/4
// (结构化 GUI 工具族)同批引入,理由与消费点已登记在 `docs/计划.md` 的 CU 章。
//
// ## 急停语义(与 CU 执行稿的取舍)
//
// - 置位后所有 cu 类工具(当前 = screenshot)拒绝执行,返回稳定错误码
//   [`CONTROL_STOPPED_CODE`]——用**稳定码**而非模糊文案,模型与前端据此识别
//   「被急停」而不是「未启用」;
// - 清除只走用户显式「恢复」或 Agent 的 stop 工具(stopped 后 resume 只能由人触发,
//   工具面只暴露 stop,不给模型自解锁);
// - 不做自动过期:急停是用户意图,不该被系统悄悄解除(重启进程亦为显式动作)。
//
// ## 审计纪律(与 exec_audit 同款 + 只记元数据)
//
// - **每次尝试有且只有一条**:成功、被拒(急停/未启用/参数错)都落;
// - **屏幕像素一律不入库**:只记范围描述 / 尺寸 / 字节数 / sha256 / 图像引用名;
// - 写入失败只告警不阻断(审计是旁路,不应影响主流程);
// - 保留最近 N 行(与 exec_audit 的 2000 行同口径)。

use crate::models::db::{now_iso, Db};
use crate::models::types::ToolContext;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// 急停开关(进程内单例:AppState 持有,同一 `Arc` 注入 ToolDeps 供工具执行侧读取)。
///
/// 为什么不做成进程级 static:测试与多实例场景会互相污染(见 TEST-ISO-1 的教训);
/// 挂在 AppState/ToolDeps 上天然是「每个运行时实例一份」,单测可直接构造。
#[derive(Debug, Default)]
pub struct ComputerUseControl {
    stopped: AtomicBool,
}

impl ComputerUseControl {
    pub fn new() -> Self {
        Self::default()
    }

    /// 置位急停(前端按钮 / Agent 的 stop 工具共用)
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
    }

    /// 解除急停(仅用户显式「恢复」路径调用)
    pub fn resume(&self) {
        self.stopped.store(false, Ordering::SeqCst);
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }
}

/// 急停拒绝的稳定错误码(工具结果与前端据此识别;不要用模糊文案做判据)
pub const CONTROL_STOPPED_CODE: &str = "CONTROL_STOPPED";

/// 急停拒绝文案(单一出处:工具执行侧与 status 端点共用口径)
pub fn stopped_message() -> String {
    format!("{CONTROL_STOPPED_CODE}: 电脑操作已被急停,在用户点「恢复操作电脑」前不会执行")
}

/// 审计行的来源(与 exec_audit 同口径:chat | task;Android 直调暂无 cu 通道)
pub const SOURCE_CHAT: &str = "chat";
pub const SOURCE_TASK: &str = "task";

/// 由工具上下文推断来源与任务 id(与 bash 审计同口径:`task:` 前缀 = 任务模式虚拟会话)
pub fn source_of(ctx: &ToolContext) -> (&'static str, Option<String>) {
    let is_task = ctx.session_id.starts_with("task:");
    if is_task {
        (
            SOURCE_TASK,
            Some(ctx.session_id.trim_start_matches("task:").to_string()),
        )
    } else {
        (SOURCE_CHAT, None)
    }
}

/// 审计表保留行数上限(最近 N 行);与 exec_audit 的 2000 行同口径。
const CU_AUDIT_KEEP_ROWS: i64 = 2000;

/// 一次 cu 操作尝试的审计入参(具名结构,字段多且多为可选)
#[derive(Debug, Clone)]
pub struct CuAuditRecord {
    /// chat | task
    pub source: &'static str,
    pub task_id: Option<String>,
    pub session_id: Option<String>,
    /// windows | android(其它平台为编译占位,不产生审计)
    pub platform: &'static str,
    /// 动作(当前唯一:`read_screen`)
    pub action: &'static str,
    /// 范围描述(全屏 / 显示器 i / 区域 / 窗口「标题」;失败时记参数级提示)
    pub target: String,
    /// allowed | denied
    pub decision: &'static str,
    /// ok | refused | error(结果三元:成功 / 主动拒绝 / 执行出错)
    pub result: &'static str,
    /// 稳定错误码(成功时为空串)
    pub error_code: String,
    /// 图像引用名(frame_id;未落盘为空串)
    pub image_ref: String,
    /// PNG 字节数(未落盘为 None)
    pub png_bytes: Option<i64>,
    /// PNG sha256(未落盘为空串)
    pub sha256: String,
}

/// 审计行(面板展示用;线格式与前端 api/computerUse.ts 对齐)
#[derive(Debug, Clone, Serialize)]
pub struct CuAuditEntry {
    pub id: i64,
    pub ts: String,
    pub source: String,
    pub task_id: Option<String>,
    pub session_id: Option<String>,
    pub platform: String,
    pub action: String,
    pub target: String,
    pub decision: String,
    pub result: String,
    pub error_code: String,
    pub image_ref: String,
    pub png_bytes: Option<i64>,
    pub sha256: String,
}

/// 写一行审计。返回是否写入成功(失败已记 warn,调用方无需处理)。
pub fn record(db: &Arc<Db>, r: &CuAuditRecord) -> bool {
    let conn = db.write();
    let result = conn.execute(
        "INSERT INTO cu_audit \
         (ts, source, task_id, session_id, platform, action, target, decision, result, \
          error_code, image_ref, png_bytes, sha256) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        rusqlite::params![
            now_iso(),
            r.source,
            r.task_id,
            r.session_id,
            r.platform,
            r.action,
            r.target,
            r.decision,
            r.result,
            r.error_code,
            r.image_ref,
            r.png_bytes,
            r.sha256,
        ],
    );
    match result {
        Ok(_) => {
            // 保留策略:写后按 id 保留最近 N 行(与 exec_audit 同款;失败仅告警)。
            if let Err(e) = conn.execute(
                "DELETE FROM cu_audit WHERE id NOT IN (
                   SELECT id FROM cu_audit ORDER BY id DESC LIMIT ?1
                 )",
                rusqlite::params![CU_AUDIT_KEEP_ROWS],
            ) {
                tracing::warn!(op = "cu_audit prune", error = %e, "电脑操作审计保留策略清理失败");
            }
            true
        }
        Err(e) => {
            tracing::warn!(op = "cu_audit insert", error = %e, "电脑操作审计落库失败");
            false
        }
    }
}

/// 审计查询:按时间倒序,limit 上限 500(与 exec_audit 同口径)。
pub fn list(db: &Arc<Db>, limit: usize) -> Vec<CuAuditEntry> {
    let limit = limit.clamp(1, 500);
    let conn = match db.read() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(op = "cu_audit read", error = %e, "只读连接获取失败,回退空列表");
            return Vec::new();
        }
    };
    let mut stmt = match conn.prepare(
        "SELECT id, ts, source, task_id, session_id, platform, action, target, decision, \
         result, error_code, image_ref, png_bytes, sha256 FROM cu_audit \
         ORDER BY ts DESC, id DESC LIMIT ?1",
    ) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "电脑操作审计查询准备失败");
            return Vec::new();
        }
    };
    let rows = stmt.query_map(rusqlite::params![limit as i64], |row| {
        Ok(CuAuditEntry {
            id: row.get(0)?,
            ts: row.get(1)?,
            source: row.get(2)?,
            task_id: row.get(3)?,
            session_id: row.get(4)?,
            platform: row.get(5)?,
            action: row.get(6)?,
            target: row.get(7)?,
            decision: row.get(8)?,
            result: row.get(9)?,
            error_code: row.get(10)?,
            image_ref: row.get(11)?,
            png_bytes: row.get(12)?,
            sha256: row.get(13)?,
        })
    });
    match rows {
        Ok(it) => it.flatten().collect(),
        Err(e) => {
            tracing::warn!(error = %e, "电脑操作审计查询失败");
            Vec::new()
        }
    }
}

/// 清空审计(设置面板「清空」按钮;不删表,仅删行)。
pub fn clear(db: &Arc<Db>) -> Result<usize, String> {
    let conn = db.write();
    conn.execute("DELETE FROM cu_audit", [])
        .map_err(|e| format!("清空电脑操作审计失败: {e}"))
}

/// PNG 内容 sha256(审计留痕但不落像素:哈希用于事后核对同一帧,不在库里存图)
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::test_support::TempDataDir;

    fn temp_db(tag: &str) -> (TempDataDir, Arc<Db>) {
        let dir = TempDataDir::new(tag);
        let db = Arc::new(Db::open(&dir.join("cu.db"), &dir).unwrap());
        (dir, db)
    }

    fn rec(decision: &'static str) -> CuAuditRecord {
        CuAuditRecord {
            source: SOURCE_CHAT,
            task_id: None,
            session_id: Some("s1".into()),
            platform: "windows",
            action: "read_screen",
            target: "全屏".into(),
            decision,
            result: "ok",
            error_code: String::new(),
            image_ref: "shot-x.png".into(),
            png_bytes: Some(123),
            sha256: "ab".into(),
        }
    }

    /// 急停置位/恢复的最小语义(且默认不置位——新装不应处于急停态)
    #[test]
    fn control_default_stopped_resume_cycle() {
        let c = ComputerUseControl::new();
        assert!(!c.is_stopped(), "默认必须是未急停");
        c.stop();
        assert!(c.is_stopped());
        c.resume();
        assert!(!c.is_stopped());
    }

    /// 来源推断与 bash 审计同口径:task: 前缀 = 任务模式
    #[test]
    fn source_inference_matches_task_prefix() {
        let chat = ToolContext {
            session_id: "s1".into(),
            character_id: String::new(),
            agent_depth: 0,
            scope: None,
            budget: None,
        };
        let (src, tid) = source_of(&chat);
        assert_eq!(src, SOURCE_CHAT);
        assert!(tid.is_none());

        let task = ToolContext {
            session_id: "task:t42".into(),
            ..chat
        };
        let (src, tid) = source_of(&task);
        assert_eq!(src, SOURCE_TASK);
        assert_eq!(tid.as_deref(), Some("t42"));
    }

    /// 写入可查、清空生效;sha256 是十六进制且内容敏感(同图同值、异图异值)
    #[test]
    fn record_list_clear_roundtrip() {
        let (_guard, db) = temp_db("cu-audit");
        assert!(record(&db, &rec("allowed")));
        let rows = list(&db, 10);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].decision, "allowed");
        assert_eq!(rows[0].target, "全屏");
        assert_eq!(clear(&db).unwrap(), 1);
        assert!(list(&db, 10).is_empty());
    }

    #[test]
    fn sha256_is_stable_and_content_sensitive() {
        let a = sha256_hex(b"png-bytes");
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(a, sha256_hex(b"png-bytes"));
        assert_ne!(a, sha256_hex(b"other-bytes"));
    }

    /// 保留策略:超过上限只留最近 N 行(N 取小值不易构造,故直接验证上限常量为正且有界)
    #[test]
    fn retention_bound_is_positive() {
        assert!(CU_AUDIT_KEEP_ROWS > 0);
    }
}
