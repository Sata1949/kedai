// 诊断路由:GET /api/diagnostics/cache(缓存感知管线)+ GET /api/diagnostics/data-dirs(双库分叉,DB-7)
// cache 返回近 N 条请求的缓存命中率(加权 token 口径)、逐条明细、按可配置单价估算的
// 费用与节省,以及输入 tokens 对照 max_context_tokens 的四级水位报告。
use crate::api::app_state::AppState;
use crate::config;
use crate::services::cache_diagnostics::{summarize, watermark, CachePricing, CacheUsageRow};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::{err_with_code, ErrorCode};

/// 全会话统计:窗口参数是 `?1`。
const SQL_ALL_SESSIONS: &str = "SELECT session_id, created_at, prompt_tokens, completion_tokens,
        prompt_cache_hit_tokens, prompt_cache_miss_tokens
 FROM llm_requests ORDER BY id DESC LIMIT ?1";

/// 单会话统计:`?1` = session_id,`?2` = 窗口。
///
/// 两个 SQL 的**参数序号必须与各自绑定处一致**——曾因无会话分支沿用了 `?2` 而只绑定
/// 一个参数,使「不带 session_id」的请求(前端优化面板的默认路径)恒定报
/// `Wrong number of parameters passed to query`。改 SQL 时请同时改绑定处。
const SQL_BY_SESSION: &str = "SELECT session_id, created_at, prompt_tokens, completion_tokens,
        prompt_cache_hit_tokens, prompt_cache_miss_tokens
 FROM llm_requests WHERE session_id = ?1 ORDER BY id DESC LIMIT ?2";

#[derive(Deserialize)]
pub struct CacheQuery {
    /// 会话过滤(缺省 = 全部会话合并统计)
    pub session_id: Option<String>,
    /// 统计窗口条数(默认 20,1..=500)
    pub window: Option<usize>,
}

/// GET /api/diagnostics/cache:缓存命中率、费用节省与上下文水位
pub async fn cache(State(state): State<Arc<AppState>>, Query(q): Query<CacheQuery>) -> Response {
    let window = q.window.unwrap_or(20).clamp(1, 500);
    // 近 N 条请求(时间正序返回,便于前端画趋势)
    let rows: Vec<CacheUsageRow> = {
        let conn = match state.db.read() {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(error = %e, "读取缓存统计失败:获取只读连接");
                return err_with_code(
                    ErrorCode::Db,
                    "读取缓存统计失败",
                    StatusCode::INTERNAL_SERVER_ERROR,
                );
            }
        };
        let map_row = |row: &rusqlite::Row<'_>| -> rusqlite::Result<CacheUsageRow> {
            Ok(CacheUsageRow {
                session_id: row.get(0)?,
                created_at: row.get(1)?,
                prompt_tokens: row.get(2)?,
                completion_tokens: row.get(3)?,
                hit: row.get(4)?,
                miss: row.get(5)?,
            })
        };
        // 绑定参数与所选 SQL 的序号一一对应(见两个 SQL 常量的注释)
        let outcome: rusqlite::Result<Vec<CacheUsageRow>> = match &q.session_id {
            Some(sid) => conn.prepare(SQL_BY_SESSION).and_then(|mut stmt| {
                stmt.query_map(rusqlite::params![sid, window as i64], map_row)
                    .and_then(|rows| rows.collect())
            }),
            None => conn.prepare(SQL_ALL_SESSIONS).and_then(|mut stmt| {
                stmt.query_map(rusqlite::params![window as i64], map_row)
                    .and_then(|rows| rows.collect())
            }),
        };
        match outcome {
            Ok(mut v) => {
                v.reverse(); // DESC 取最近 N 条 → 反转为时间正序
                v
            }
            Err(e) => {
                // 原始错误(含 SQLite 文案)只进日志;响应给通用文案,不把内部细节透传给客户端
                tracing::error!(error = %e, window, "读取缓存统计失败");
                return err_with_code(
                    ErrorCode::Db,
                    "读取缓存统计失败",
                    StatusCode::INTERNAL_SERVER_ERROR,
                );
            }
        }
    };
    // 单价:服务内常量默认(DeepSeek 参考价),后续可挪到设置
    let pricing = CachePricing::default();
    let summary = summarize(&rows, &pricing);
    // 水位:以窗口内最新一条请求的 prompt_tokens 为输入侧 token,
    // 对照设置中的 max_context_tokens(设置快照:不留锁跨 await)
    let max_context = state.settings_snapshot().max_context_tokens;
    let latest_prompt = rows.last().map(|r| r.prompt_tokens).unwrap_or(0);
    let level = watermark(latest_prompt, max_context);
    Json(json!({
        "window": window,
        "session_id": q.session_id,
        "totals": {
            "count": summary.count,
            "total_prompt": summary.total_prompt,
            "total_completion": summary.total_completion,
            "total_hit": summary.total_hit,
            "total_miss": summary.total_miss,
            "hit_rate": summary.hit_rate,
        },
        "entries": rows.iter().map(|r| json!({
            "session_id": r.session_id,
            "created_at": r.created_at,
            "prompt_tokens": r.prompt_tokens,
            "completion_tokens": r.completion_tokens,
            "hit": r.hit,
            "miss": r.miss,
        })).collect::<Vec<_>>(),
        "pricing": {
            "cache_hit_per_m": pricing.cache_hit_per_m,
            "input_per_m": pricing.input_per_m,
            "output_per_m": pricing.output_per_m,
            "currency_hint": "CNY/1M",
        },
        "cost": summary.cost,
        "saved": summary.saved,
        "watermark": {
            "level": level.level,
            "ratio": level.ratio,
            "input_tokens": latest_prompt,
            "max_context_tokens": max_context,
            "next_level": level.next_level,
            "gap_tokens": level.gap_tokens,
        },
    }))
    .into_response()
}

/// GET /api/diagnostics/data-dirs:当前生效数据目录 + 检测到的「另一套有用户数据的库」。
///
/// 存在的理由(DB-7 的 UI 侧缺口):`detect_split_data_dirs` 自 2026-10-29 起只在启动期
/// 打 `warn`——用户界面完全看不到,只能自己发现「聊天记录没了」。本端点把同一份检测
/// 结果搬上 HTTP,供前端横幅提示。**纯只读**:不改目录选择、不写文件、不动任何状态。
///
/// 每次请求都重新探测而不缓存:候选目录的内容可能在进程存活期间变化(用户手动拷库、
/// 删掉另一套库),缓存会让提示停留在过期事实上。代价是几次 `metadata` + 只读 SQLite
/// 打开,只在页面启动时调一次,可接受。
pub async fn data_dirs(State(state): State<Arc<AppState>>) -> Response {
    let selected = state.config.data_dir.clone();
    let candidates = config::split_dir_candidates();
    // 目录探测含文件系统与 SQLite 打开(阻塞调用),放 spawn_blocking 避免占 tokio worker
    let scan_target = selected.clone();
    let detected = tokio::task::spawn_blocking(move || {
        config::detect_split_data_dirs(&scan_target, &candidates)
    })
    .await;
    let others = match detected {
        Ok(v) => v,
        Err(e) => {
            // 任务被取消/panic:不假装「没有分叉」——那会把检测失败伪装成正常态
            tracing::error!(error = %e, "双库检测任务失败");
            return err_with_code(
                ErrorCode::Internal,
                "双库检测失败",
                StatusCode::INTERNAL_SERVER_ERROR,
            );
        }
    };
    Json(split_dirs_payload(&selected, &others)).into_response()
}

// ===== 纯函数(端点与单测共用;`*_tests` 侧不重复实现判据) =====

/// 把检测结果摊平成线格式条目(见 [`data_dirs`])。
///
/// 抽成纯函数以便单测断言「路径与判据都如实透出、顺序保持候选顺序」,
/// 而不必拉起整个 AppState。
pub(crate) fn split_dirs_payload(
    selected: &Path,
    found: &[(PathBuf, &'static str)],
) -> serde_json::Value {
    json!({
        "data_dir": selected.to_string_lossy(),
        "others": found
            .iter()
            .map(|(dir, why)| json!({
                "path": dir.to_string_lossy(),
                "evidence": why,
            }))
            .collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod data_dirs_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    /// 检测结果按候选顺序原样透出:路径与判据都必须出现在线格式里,
    /// 否则前端只能显示「有另一套库」却给不出位置。
    #[test]
    fn payload_keeps_path_and_evidence() {
        let selected = PathBuf::from(r"C:\data-a");
        let found = vec![
            (PathBuf::from(r"C:\data-b"), "settings.json + 数据库"),
            (PathBuf::from(r"D:\data-c"), "数据库（含角色或会话）"),
        ];
        let v = split_dirs_payload(&selected, &found);
        assert_eq!(v["data_dir"], json!(r"C:\data-a"));
        let others = v["others"].as_array().unwrap();
        assert_eq!(others.len(), 2, "两套都要报: {v}");
        assert_eq!(others[0]["path"], json!(r"C:\data-b"));
        assert_eq!(others[0]["evidence"], json!("settings.json + 数据库"));
        assert_eq!(others[1]["path"], json!(r"D:\data-c"));
    }

    /// 无分叉时 `others` 必须是空数组(不是 null、不是缺字段):
    /// 前端按 `others.length > 0` 决定是否提示,缺字段会让判据变成 undefined。
    #[test]
    fn empty_result_is_empty_array() {
        let v = split_dirs_payload(Path::new(r"C:\only"), &[]);
        assert_eq!(v["others"], json!([]));
        assert_eq!(v["data_dir"], json!(r"C:\only"));
    }
}
