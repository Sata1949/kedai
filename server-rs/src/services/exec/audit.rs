// 命令执行审计(阶段 B/C 的合规要求):每次**尝试**执行命令落一行,含被拒绝的。
//
// 为什么独立成表而非写日志:root/ADB 级命令不可逆,「谁在何时以什么等级跑了什么、
// 结果如何」需要可在设置面板内查询与事后追溯(日志按天清理,不能作为审计源)。
//
// 纪律:
// - 拒绝也要落(decision=denied),否则「有人试图跑 rm -rf /」不留痕;
// - stdout/stderr 截断存储,避免审计表被一条长输出灌爆;
// - 写入失败只告警不阻断命令执行(审计是旁路,不应影响主流程)。
use crate::models::db::{now_iso, Db};
// 命令风险词汇已下沉 L1(2026-09-14):L2 直连 models,不经 tools 转发(否则仍是跨代边)。
use crate::models::tool_policy::CommandRisk;
use serde::Serialize;
use std::sync::Arc;

/// 审计行的来源(在哪个模式下发起)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditSource {
    /// 角色扮演聊天(agent 工具循环)
    Chat,
    /// 任务模式(task 工作台)
    Task,
    /// Android 执行层直调
    Android,
}

impl AuditSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Task => "task",
            Self::Android => "android",
        }
    }
}

/// 裁决结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditDecision {
    Allowed,
    /// 被拒(授权未通过 / 任务模式无确认通道 / 命令级硬门拦截)
    Denied,
}

impl AuditDecision {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Allowed => "allowed",
            Self::Denied => "denied",
        }
    }
}

/// 审计面板展示用的一行(线格式与前端 exec.ts 对齐)。
#[derive(Debug, Clone, Serialize)]
pub struct ExecAuditEntry {
    pub id: i64,
    pub ts: String,
    pub source: String,
    pub task_id: Option<String>,
    pub session_id: Option<String>,
    pub command: String,
    pub shell: String,
    pub tier: String,
    pub risk: String,
    pub decision: String,
    pub exit_code: Option<i64>,
    pub stdout_summary: String,
    pub stderr_summary: String,
    /// 风险标记(D1 审计增强):'' | data_dir_touch | parent_climb(见 [`classify_risk_flag`])
    pub risk_flag: String,
}

/// 一次审计写入的入参(字段多且多为可选,用具名结构避免位置参数错配)。
#[derive(Debug, Clone)]
pub struct AuditRecord {
    pub source: AuditSource,
    pub task_id: Option<String>,
    pub session_id: Option<String>,
    pub command: String,
    pub shell: String,
    pub tier: String,
    pub risk: CommandRisk,
    pub decision: AuditDecision,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    /// 风险标记(见 [`classify_risk_flag`];无语义时传空串)
    pub risk_flag: String,
}

/// 风险标记:命令文本命中了数据目录的绝对路径。
/// 用途见 [`classify_risk_flag`] —— 这是**审计线索**,不是拦截依据。
pub const FLAG_DATA_DIR_TOUCH: &str = "data_dir_touch";

/// 风险标记:命令文本含 `..` 路径段(相对上溯,可越过当前工作区)。
pub const FLAG_PARENT_CLIMB: &str = "parent_climb";

/// 给命令文本打风险标记(供审计筛选与事后追溯),**不改变命令是否执行**。
///
/// 为什么只标记不拦截:模型一旦拿到 shell,就能读该进程有权限读的**任意**路径;cwd jail
/// 只约束「相对路径与缺省目录」,拦不住命令文本里的绝对路径。假装封堵比不封堵更糟——
/// 这里能做的是把「这轮命令碰了用户真实数据目录 / 试图往上爬」记进审计表,让 D1 那类
/// 「模型把数据目录当草稿纸」的行为事后可查、可量化(2026-09 实测证据见 docs/经验.md)。
///
/// 判定是启发式字符串检查,两个标记取更强的一个:
/// - `data_dir_touch`:命令含数据目录绝对路径(两种分隔符写法都算,大小写不敏感);
/// - `parent_climb`:命令含 `..` 路径段(如 `cd ..`、`..\x`、`../x`)。
pub fn classify_risk_flag(command: &str, data_dir: &std::path::Path) -> String {
    let dir = data_dir.to_string_lossy().to_lowercase();
    if !dir.is_empty() {
        let lower = command.to_lowercase();
        let back = dir.replace('/', "\\");
        let slash = dir.replace('\\', "/");
        if lower.contains(&back) || lower.contains(&slash) {
            return FLAG_DATA_DIR_TOUCH.into();
        }
    }
    if has_parent_climb(command) {
        return FLAG_PARENT_CLIMB.into();
    }
    String::new()
}

/// 命令文本是否含 `..` **路径段**:前一个字符是边界(串首或分隔符/空白/引号等)、
/// 后一个字符是边界或串尾,且不是 `...` 这类省略号。
fn has_parent_climb(command: &str) -> bool {
    fn boundary(c: Option<&char>) -> bool {
        match c {
            None => true,
            Some(c) => matches!(
                c,
                '/' | '\\' | ' ' | '\t' | '"' | '\'' | '=' | ';' | '&' | '|' | '(' | ')'
            ),
        }
    }
    let cs: Vec<char> = command.chars().collect();
    for i in 0..cs.len().saturating_sub(1) {
        if cs[i] != '.' || cs[i + 1] != '.' {
            continue;
        }
        let before = if i == 0 { None } else { cs.get(i - 1) };
        let after = cs.get(i + 2);
        if boundary(before) && boundary(after) && after.map(|c| *c != '.').unwrap_or(true) {
            return true;
        }
    }
    false
}

/// 单条摘要保留长度(字符)。审计用于回溯「跑了什么、成没成」,
/// 不需要完整输出;完整内容在工具结果里已给模型/用户。
const SUMMARY_CHARS: usize = 2000;

fn summarize(s: &str) -> String {
    let count = s.chars().count();
    if count <= SUMMARY_CHARS {
        return s.to_string();
    }
    let head: String = s.chars().take(SUMMARY_CHARS).collect();
    format!("{head}…(共 {count} 字符)")
}

/// 写一行审计。返回是否写入成功(失败已记 warn,调用方无需处理)。
pub fn record(db: &Arc<Db>, r: &AuditRecord) -> bool {
    let conn = db.write();
    let result = conn.execute(
        "INSERT INTO exec_audit \
         (ts, source, task_id, session_id, command, shell, tier, risk, decision, exit_code, stdout_summary, stderr_summary, risk_flag) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        rusqlite::params![
            now_iso(),
            r.source.as_str(),
            r.task_id,
            r.session_id,
            r.command,
            r.shell,
            r.tier,
            r.risk.as_str(),
            r.decision.as_str(),
            r.exit_code.map(|c| c as i64),
            summarize(&r.stdout),
            summarize(&r.stderr),
            r.risk_flag,
        ],
    );
    match result {
        Ok(_) => {
            // 保留策略(2026-09-13 批次 2):审计表此前只增不删(仅手动 DELETE /api/exec/audit
            // 清理),长期使用会无界增长。每次写入后按 id 保留最近 EXEC_AUDIT_KEEP_ROWS 行;
            // 成本为一次带索引的 DELETE,失败仅告警不影响本次审计记录。
            if let Err(e) = conn.execute(
                "DELETE FROM exec_audit WHERE id NOT IN (
                   SELECT id FROM exec_audit ORDER BY id DESC LIMIT ?1
                 )",
                rusqlite::params![EXEC_AUDIT_KEEP_ROWS],
            ) {
                tracing::warn!(
                    op = "exec_audit prune",
                    error = e.to_string(),
                    "审计保留策略清理失败"
                );
            }
            true
        }
        Err(e) => {
            tracing::warn!(
                op = "exec_audit insert",
                error = e.to_string(),
                "命令审计落库失败"
            );
            false
        }
    }
}

/// 审计表保留行数上限(最近 N 行);超出部分在每次写入后清理。
const EXEC_AUDIT_KEEP_ROWS: i64 = 2000;

/// 审计查询:按时间倒序,limit 上限 500。
/// `source` / `risk` 非空时按其过滤(前端审计面板的筛选项)。
pub fn list(
    db: &Arc<Db>,
    limit: usize,
    source: Option<&str>,
    risk: Option<&str>,
) -> Vec<ExecAuditEntry> {
    let limit = limit.clamp(1, 500);
    let conn = match db.read() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(op = "命令审计 read", error = %e, "只读连接获取失败,回退空列表");
            return Vec::new();
        }
    };
    let mut sql = String::from(
        "SELECT id, ts, source, task_id, session_id, command, shell, tier, risk, decision, \
         exit_code, stdout_summary, stderr_summary, risk_flag FROM exec_audit WHERE 1=1",
    );
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    if let Some(s) = source.filter(|s| !s.trim().is_empty()) {
        sql.push_str(" AND source = ?");
        params.push(Box::new(s.to_string()));
    }
    if let Some(r) = risk.filter(|r| !r.trim().is_empty()) {
        sql.push_str(" AND risk = ?");
        params.push(Box::new(r.to_string()));
    }
    sql.push_str(" ORDER BY ts DESC, id DESC LIMIT ?");
    params.push(Box::new(limit as i64));

    let mut stmt = match conn.prepare(&sql) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = e.to_string(), "命令审计查询准备失败");
            return Vec::new();
        }
    };
    let refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|b| b.as_ref()).collect();
    let rows = stmt.query_map(refs.as_slice(), |row| {
        Ok(ExecAuditEntry {
            id: row.get(0)?,
            ts: row.get(1)?,
            source: row.get(2)?,
            task_id: row.get(3)?,
            session_id: row.get(4)?,
            command: row.get(5)?,
            shell: row.get(6)?,
            tier: row.get(7)?,
            risk: row.get(8)?,
            decision: row.get(9)?,
            exit_code: row.get(10)?,
            stdout_summary: row.get(11)?,
            stderr_summary: row.get(12)?,
            risk_flag: row.get(13)?,
        })
    });
    match rows {
        Ok(it) => it.flatten().collect(),
        Err(e) => {
            tracing::warn!(error = e.to_string(), "命令审计查询失败");
            Vec::new()
        }
    }
}

/// 清空审计(设置面板「清空」按钮;不删表,仅删行)。
pub fn clear(db: &Arc<Db>) -> Result<usize, String> {
    let conn = db.write();
    conn.execute("DELETE FROM exec_audit", [])
        .map_err(|e| format!("清空命令审计失败: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::db::Db;
    use crate::utils::test_support::TempDataDir;

    /// 返回 (守卫, 库):解构绑定按**逆序**析构,守卫在前才活到最后(见 test_support 模块头)
    fn temp_db(tag: &str) -> (TempDataDir, Arc<Db>) {
        let dir = TempDataDir::new(&format!("exec-audit-{tag}"));
        let db = Arc::new(Db::open(&dir.join("kedai.db"), &dir).expect("建库"));
        (dir, db)
    }

    fn rec(cmd: &str, decision: AuditDecision) -> AuditRecord {
        AuditRecord {
            source: AuditSource::Chat,
            task_id: None,
            session_id: Some("s1".into()),
            command: cmd.into(),
            shell: "sh".into(),
            tier: "sandbox".into(),
            risk: CommandRisk::Sensitive,
            decision,
            exit_code: Some(0),
            stdout: "ok".into(),
            stderr: String::new(),
            risk_flag: String::new(),
        }
    }

    /// 风险标记分类器(D1 审计增强):命中数据目录绝对路径 / `..` 上溯时打标,
    /// 普通命令与省略号不误报。**只标记不拦截**——本用例只断言分类结果。
    #[test]
    fn risk_flag_marks_data_dir_touch_and_parent_climb() {
        let data_dir = std::path::Path::new(r"C:\Users\u\AppData\Roaming\com.kedai.app\data");
        // 命中数据目录绝对路径:反斜杠 / 正斜杠两种写法 + 大小写不敏感
        assert_eq!(
            classify_risk_flag(
                r"type C:\Users\u\AppData\Roaming\com.kedai.app\data\settings.json",
                data_dir
            ),
            FLAG_DATA_DIR_TOUCH
        );
        assert_eq!(
            classify_risk_flag(
                "cat c:/users/u/appdata/roaming/com.kedai.app/data/kedai.db",
                data_dir
            ),
            FLAG_DATA_DIR_TOUCH
        );
        // 相对上溯:`cd ..`、`..\x`、`../x` 都算
        assert_eq!(
            classify_risk_flag("cd .. && dir", data_dir),
            FLAG_PARENT_CLIMB
        );
        assert_eq!(
            classify_risk_flag(r"type ..\settings.json", data_dir),
            FLAG_PARENT_CLIMB
        );
        assert_eq!(
            classify_risk_flag("cat ../x.txt", data_dir),
            FLAG_PARENT_CLIMB
        );
        // 普通命令(含实测里被反复重试的那类自检命令)不打标
        assert!(classify_risk_flag("node test/vec2.test.mjs && echo done", data_dir).is_empty());
        assert!(classify_risk_flag("git status --porcelain", data_dir).is_empty());
        // 省略号/非路径段的三点不误报
        assert!(classify_risk_flag("echo 完成…", data_dir).is_empty());
        assert!(classify_risk_flag("echo a...b", data_dir).is_empty());
    }

    /// 风险标记随审计行往返落库(列表读出与写入一致)。
    #[test]
    fn records_and_reads_risk_flag() {
        let (_dir, db) = temp_db("flag");
        let mut r = rec("type ..\\settings.json", AuditDecision::Denied);
        r.risk_flag = FLAG_PARENT_CLIMB.into();
        record(&db, &r);
        let rows = list(&db, 10, None, None);
        assert_eq!(rows[0].risk_flag, FLAG_PARENT_CLIMB);
    }

    #[test]
    fn records_and_lists_newest_first() {
        let (_dir, db) = temp_db("list");
        assert!(record(&db, &rec("echo 1", AuditDecision::Allowed)));
        assert!(record(&db, &rec("echo 2", AuditDecision::Allowed)));
        let rows = list(&db, 10, None, None);
        assert_eq!(rows.len(), 2);
        // 倒序:id 大的在前
        assert!(rows[0].id > rows[1].id);
    }

    #[test]
    fn records_denied_attempts() {
        let (_dir, db) = temp_db("denied");
        record(&db, &rec("rm -rf /", AuditDecision::Denied));
        let rows = list(&db, 10, None, None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].decision, "denied");
    }

    #[test]
    fn filters_by_source_and_risk() {
        let (_dir, db) = temp_db("filter");
        let mut r = rec("echo a", AuditDecision::Allowed);
        r.source = AuditSource::Task;
        r.risk = CommandRisk::Destructive;
        record(&db, &r);
        record(&db, &rec("echo b", AuditDecision::Allowed)); // Chat + Sensitive
        assert_eq!(list(&db, 10, Some("task"), None).len(), 1);
        assert_eq!(list(&db, 10, None, Some("destructive")).len(), 1);
        assert_eq!(list(&db, 10, Some("chat"), Some("sensitive")).len(), 1);
    }

    #[test]
    fn long_output_summarized() {
        let (_dir, db) = temp_db("trunc");
        let mut r = rec("cat big", AuditDecision::Allowed);
        r.stdout = "x".repeat(SUMMARY_CHARS + 500);
        record(&db, &r);
        let rows = list(&db, 10, None, None);
        assert!(rows[0].stdout_summary.contains("字符"), "应标注总长");
        assert!(rows[0].stdout_summary.chars().count() < SUMMARY_CHARS + 100);
    }

    #[test]
    fn clear_removes_rows() {
        let (_dir, db) = temp_db("clear");
        record(&db, &rec("echo x", AuditDecision::Allowed));
        assert_eq!(clear(&db).unwrap(), 1);
        assert!(list(&db, 10, None, None).is_empty());
    }

    #[test]
    fn limit_is_clamped() {
        let (_dir, db) = temp_db("clamp");
        for i in 0..5 {
            record(&db, &rec(&format!("echo {i}"), AuditDecision::Allowed));
        }
        // limit=0 被夹到 1;超大值夹到 500(此处只有 5 行)
        assert_eq!(list(&db, 0, None, None).len(), 1);
        assert_eq!(list(&db, 10_000, None, None).len(), 5);
    }
}
