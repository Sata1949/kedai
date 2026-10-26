//! 任务运行期自检问题的持久化(SKY-9-L1,2026-10-25)。
//!
//! 背景:SKY-9 的无头浏览器自检只在任务终态门槛里跑一次,结果此前只进
//! `tasks.error` 与 Status 事件;详情接口的 `delivery.content_issues` 是读时静态
//! 重算,于是错误条说「页面没有 canvas」、成果卡却 `content_issues=[]`。
//!
//! 本模块把运行期问题落到 `tasks.runtime_issues`(JSON 字符串数组),详情接口只
//! **读**这一列(绝不在详情请求里重跑浏览器——每次 SSE 刷新都跑一次 3~6 秒的
//! 浏览器进程不可接受)。写入口只有一个:任务终态门槛在自检完成后调用
//! [`TaskService::set_runtime_issues`]。重跑(`reset_task`)、批准(`approve`)与
//! 追加指令(`followup`)起跑时清空,避免上一轮的问题挂在新一轮成果卡上。
//!
//! 与 `flow_snapshot` 同款:该列**不进** `TASK_COLS`(列表接口用不上),按任务 id
//! 单列读写。列定义与迁移见 `models/db/schema.rs` / `migration/ddl.rs`。
use super::*;
use rusqlite::Connection;

/// 详情展示上限:自检本身最多报 8 条,这里再兜一层,防止手改/脏数据把响应撑大。
const MAX_PERSISTED_ISSUES: usize = 32;

impl TaskService {
    /// 写入本轮运行期自检问题(空切片 = 清空)。DB 写失败只记日志、返回 false,
    /// 不阻断任务收尾——持久化失败最多让成果卡少一行提示,不该把任务判失败。
    pub(crate) fn set_runtime_issues(&self, id: &str, issues: &[String]) -> bool {
        let conn = self.db.write();
        write(&conn, id, issues)
    }

    /// 清空运行期自检问题(重跑/批准/追加指令起跑时调用)。
    pub(crate) fn clear_runtime_issues(&self, id: &str) -> bool {
        self.set_runtime_issues(id, &[])
    }

    /// 读本轮运行期自检问题(详情接口调用;列缺失/脏数据一律按空数组)。
    pub(crate) fn runtime_issues(&self, id: &str) -> Vec<String> {
        let Ok(conn) = self.db.read() else {
            tracing::warn!(task_id = id, "运行期自检问题读取失败:数据库读连接不可用");
            return Vec::new();
        };
        read(&conn, id)
    }
}

/// 单列写入(抽成自由函数便于在临时库上直接测;调用方负责取连接)。
fn write(conn: &Connection, id: &str, issues: &[String]) -> bool {
    let items: Vec<String> = issues
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .take(MAX_PERSISTED_ISSUES)
        .map(str::to_string)
        .collect();
    let json = match serde_json::to_string(&items) {
        Ok(text) => text,
        Err(e) => {
            // Vec<String> 序列化失败只可能是分配/内存级异常;退化为空数组而不是写半截
            tracing::error!(
                task_id = id,
                error = e.to_string(),
                "运行期自检问题序列化失败"
            );
            "[]".to_string()
        }
    };
    match conn.execute(
        "UPDATE tasks SET runtime_issues = ?1 WHERE id = ?2",
        params![json, id],
    ) {
        Ok(0) => {
            tracing::warn!(task_id = id, "运行期自检问题写入未命中任务行");
            false
        }
        Ok(_) => true,
        Err(e) => {
            tracing::warn!(
                task_id = id,
                error = e.to_string(),
                "运行期自检问题写入失败"
            );
            false
        }
    }
}

/// 单列读取:任务不存在、列为空/非法 JSON 都按「无运行期问题」处理。
fn read(conn: &Connection, id: &str) -> Vec<String> {
    let raw: Option<String> = match conn
        .query_row(
            "SELECT runtime_issues FROM tasks WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .optional()
    {
        Ok(raw) => raw,
        Err(e) => {
            tracing::warn!(
                task_id = id,
                error = e.to_string(),
                "运行期自检问题读取失败"
            );
            return Vec::new();
        }
    };
    let Some(raw) = raw else {
        return Vec::new();
    };
    if raw.trim().is_empty() {
        return Vec::new();
    }
    match serde_json::from_str::<Vec<String>>(&raw) {
        Ok(items) => items
            .into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .take(MAX_PERSISTED_ISSUES)
            .collect(),
        Err(e) => {
            tracing::warn!(
                task_id = id,
                error = e.to_string(),
                "运行期自检问题解析失败"
            );
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let conn = Connection::open_in_memory().expect("打开内存库失败");
        conn.execute_batch(
            "CREATE TABLE tasks (
               id             TEXT PRIMARY KEY,
               runtime_issues TEXT NOT NULL DEFAULT '[]'
             );
             INSERT INTO tasks (id) VALUES ('t1');",
        )
        .expect("建测试表失败");
        conn
    }

    #[test]
    fn write_read_clear_round_trip() {
        let conn = conn();
        let issues = vec![
            "index.html:页面没有 canvas 画布(3D/可视化交付未渲染)".to_string(),
            "index.html:GameMain.ready=false(入口未完成初始化)".to_string(),
        ];
        assert!(write(&conn, "t1", &issues));
        assert_eq!(read(&conn, "t1"), issues);

        assert!(write(&conn, "t1", &[]));
        assert!(read(&conn, "t1").is_empty(), "清空后必须读回空数组");
    }

    #[test]
    fn empty_entries_are_dropped_and_list_is_capped() {
        let conn = conn();
        let issues: Vec<String> = (0..40)
            .map(|i| {
                if i == 0 {
                    "   ".to_string()
                } else {
                    format!("issue-{i}")
                }
            })
            .collect();
        assert!(write(&conn, "t1", &issues));
        let read_back = read(&conn, "t1");
        assert_eq!(read_back.len(), MAX_PERSISTED_ISSUES);
        assert_eq!(read_back[0], "issue-1");
        assert!(read_back.iter().all(|s| !s.trim().is_empty()));
    }

    #[test]
    fn missing_task_and_dirty_column_read_empty() {
        let conn = conn();
        assert!(read(&conn, "missing").is_empty());
        assert!(
            !write(&conn, "missing", &["x".to_string()]),
            "未命中行应返回 false"
        );

        conn.execute(
            "UPDATE tasks SET runtime_issues = 'not-json' WHERE id = 't1'",
            [],
        )
        .expect("写脏数据失败");
        assert!(read(&conn, "t1").is_empty(), "脏数据不得让详情接口失败");
    }
}
