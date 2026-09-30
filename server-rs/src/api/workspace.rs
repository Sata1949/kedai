//! 工作区接口:路径校验(创建期与探测端点共用)与工作区画像探测(2026-09-30 CODE-4)。
//!
//! 为什么 `validate_workspace` 在这里而不是 `api/tasks.rs`:创建任务(校验后冻结)与
//! 画像探测(校验后只读)必须用**同一把尺**——此前它是 tasks.rs 的私有函数,搬到这里
//! 后两个调用点共用一份,不存在第二条实现。判据本身仍是**单一出处**
//! (`tools::workspace_guard::data_dir_conflict`),本模块只做「参数 → 400 文案」的翻译。
use crate::api::app_state::AppState;
use crate::api::validation;
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

/// 校验工作区路径并**冻结**为规范化绝对路径(编码通道批次 1)。
///
/// 返回 `Ok(None)` = 未绑定(缺省/空串,旧客户端零变化);`Ok(Some(path))` = 可落库的
/// canonical 绝对路径。判据与工具期二次防线同源(`tools::workspace_guard::data_dir_conflict`),
/// 不在这里另写一份比较逻辑——创建期与运行期的口径必须只有一个出处。
///
/// 抽成自由函数以便单测覆盖三条负路径,不必起整个 AppState。
pub(crate) fn validate_workspace(
    raw: Option<&str>,
    data_dir: &std::path::Path,
) -> Result<Option<String>, String> {
    let raw = match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None => return Ok(None),
        Some(raw) => raw,
    };
    let path = std::path::Path::new(raw);
    if !path.is_dir() {
        return Err(format!("工作区不存在或不是目录:{raw}"));
    }
    let canonical = path
        .canonicalize()
        .map_err(|e| format!("工作区路径无法解析:{raw}({e})"))?;
    if let Some(reason) = crate::tools::workspace_guard::data_dir_conflict(&canonical, data_dir) {
        return Err(reason);
    }
    Ok(Some(canonical.to_string_lossy().into_owned()))
}

#[derive(Deserialize)]
pub struct ProfileQuery {
    /// 待探测的工作区路径(创建表单在用户选定/输入后用)
    pub path: Option<String>,
}

/// GET /api/workspace/profile?path=:探测工作区的项目类型画像(只读)。
///
/// 展示面用,与创建任务**同一把尺**(不存在 / 非目录 / 数据目录冲突 → 400),
/// 于是「创建表单能探测的目录」与「创建任务能绑定的目录」不会分叉。
/// `path` 缺失/空白是本端点**独有**的一条校验:创建期把空串当「不绑定」,
/// 探测没有「空」可探,故文案不宣称与创建期一致。
///
/// 探测本身是浅层目录读取(见 `services::workspace_profile`),挪阻塞线程池执行——
/// 目录很大时不该让 async 运行时等 IO。
pub async fn profile(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ProfileQuery>,
) -> Response {
    let raw = match q.path.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(raw) => raw.to_string(),
        None => return validation("缺少 path 参数:工作区画像需要指定要探测的目录"),
    };
    let data_dir = state.config.data_dir.clone();
    let probed = state
        .db_call(move || {
            let canonical = validate_workspace(Some(&raw), &data_dir)?;
            // 非空输入不会返回 Ok(None);真出现也按「缺少 path」处理,不 panic
            let canonical = canonical.ok_or("缺少 path 参数:工作区画像需要指定要探测的目录")?;
            Ok::<_, String>(crate::services::workspace_profile::probe(
                std::path::Path::new(&canonical),
            ))
        })
        .await;
    match probed {
        Err(e) => crate::api::internal(e),
        Ok(Err(e)) => validation(e),
        Ok(Ok(profile)) => Json(json!({ "ok": true, "profile": profile })).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 工作区校验(编码通道批次 1)三条负路径 + 一条正路径。
    /// 负路径都在**创建期**拦下(不建行):指到不存在的位置、指到数据目录内、
    /// 指到数据目录的上级(后者会让工作区工具顺着相对路径走进真实数据目录)。
    ///
    /// 2026-09-30 CODE-4:随 `validate_workspace` 从 `api/tasks.rs` 原样搬来,
    /// 断言逐字未改(纯移动)。
    #[test]
    fn validate_workspace_rejects_invalid_targets() {
        let tmp = crate::utils::test_support::TempDataDir::new("ws-validate");
        let data_dir = tmp.join("data");
        std::fs::create_dir_all(&data_dir).unwrap();
        let ws = tmp.join("ws");
        std::fs::create_dir_all(&ws).unwrap();

        // 缺省/空串 → 未绑定(旧客户端零变化)
        assert_eq!(validate_workspace(None, &data_dir).unwrap(), None);
        assert_eq!(validate_workspace(Some("   "), &data_dir).unwrap(), None);

        // 不存在的位置 → 拒绝
        let missing = tmp.join("nope");
        let err = validate_workspace(Some(&missing.to_string_lossy()), &data_dir)
            .expect_err("不存在的目录必须被拒");
        assert!(err.contains("不存在"), "文案应指明不存在: {err}");

        // 落在数据目录内 → 拒绝
        let inside_data = data_dir.join("inner");
        std::fs::create_dir_all(&inside_data).unwrap();
        let err = validate_workspace(Some(&inside_data.to_string_lossy()), &data_dir)
            .expect_err("数据目录内的路径必须被拒");
        assert!(err.contains("数据目录"), "文案应指明数据目录: {err}");

        // 数据目录的上级 → 拒绝(覆盖 tmp 本身)
        let err = validate_workspace(Some(&tmp.path().to_string_lossy()), &data_dir)
            .expect_err("包含数据目录的路径必须被拒");
        assert!(err.contains("上级"), "文案应指明上级目录: {err}");

        // 正常目录 → canonical 绝对路径
        let got = validate_workspace(Some(&ws.to_string_lossy()), &data_dir)
            .unwrap()
            .expect("应返回已绑定的工作区");
        assert_eq!(
            std::path::Path::new(&got),
            ws.canonicalize().unwrap(),
            "返回的必须是 canonical 路径"
        );
    }
}
