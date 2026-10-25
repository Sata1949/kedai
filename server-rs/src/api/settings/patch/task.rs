//! 任务侧默认连接与墙钟/空闲预算(预算为扁平字段,仅任务侧消费)。
use crate::services::settings_service::{connection_label, task_idle_floor_secs, RuntimeSettings};

use super::super::UpdateSettingsBody;
use super::apply_mode;
use super::PatchError;

pub(super) fn apply(
    s: &mut RuntimeSettings,
    body: &UpdateSettingsBody,
    is_task: bool,
) -> Result<(), PatchError> {
    // 任务模式默认连接(TM-SET-1):空串 = 清除(回到跟随默认连接);非空必须在
    // **本请求应用后的**连接列表里存在且启用,否则 400(本域在 connection 域之后,
    // 故同请求里先改连接再设默认也能正确校验)。
    // 校验只拦「此刻无效」;用户之后删/停用该连接时,运行期 `task_mode_default_connection`
    // 软回退默认连接(创建期的硬校验 + 运行期的软回退,两处口径并存不矛盾)。
    if let Some(v) = &body.task_default_connection_id {
        let t = v.trim().to_string();
        if t.is_empty() {
            apply_mode!(s, is_task, task_default_connection_id, String::new());
        } else {
            match s.connections.iter().find(|p| p.id == t) {
                Some(p) if p.enabled => {
                    apply_mode!(s, is_task, task_default_connection_id, t);
                }
                Some(p) => {
                    return Err(PatchError::new(
                        "task_default_connection_id",
                        format!(
                            "选择的连接「{}」已停用;请启用它,或选其它连接",
                            connection_label(p)
                        ),
                    ));
                }
                None => {
                    return Err(PatchError::new(
                        "task_default_connection_id",
                        "选择的连接不存在(可能已被删除);请在设置里恢复该连接,或选其它连接",
                    ));
                }
            }
        }
    }
    // 任务侧两道闸(提交 3 · D3/D7):扁平全局字段(任务侧消费,不进模式覆盖层);
    // 越界拒绝,与 load 钳制区间一致。step_budget 刻意不设下限(1 秒合法:
    // 低于单次调用看门狗的值 = 「第一轮结束就收尾」的合法语义)。
    if let Some(v) = body.task_step_budget_secs {
        if v != 0 && !(1..=86_400).contains(&v) {
            return Err(PatchError::new(
                "task_step_budget_secs",
                "task_step_budget_secs 须为 0(关闭)或 1..=86400",
            ));
        }
        s.task_step_budget_secs = v;
    }
    // 任务级总预算(PRODCAP-2):同款区间(0 = 关,否则 1..=86400)
    if let Some(v) = body.task_total_budget_secs {
        if v != 0 && !(1..=86_400).contains(&v) {
            return Err(PatchError::new(
                "task_total_budget_secs",
                "task_total_budget_secs 须为 0(关闭)或 1..=86400",
            ));
        }
        s.task_total_budget_secs = v;
    }
    // 团队任务总预算(TMR3-4):同款区间(0 = 关,否则 1..=86400)
    if let Some(v) = body.task_team_budget_secs {
        if v != 0 && !(1..=86_400).contains(&v) {
            return Err(PatchError::new(
                "task_team_budget_secs",
                "task_team_budget_secs 须为 0(关闭)或 1..=86400",
            ));
        }
        s.task_team_budget_secs = v;
    }
    if let Some(v) = body.task_idle_timeout_secs {
        // 下限走 `task_idle_floor_secs()` 单一出处(= 单条命令上限 + 单次模型调用上限 + 1);
        // 文案里也插值同一个数,**不再手写「601」与它的推导式**——批次 2 抬 bash 上限时,
        // 手写文案会与真实区间不一致,用户照文案调参会被莫名拒绝。
        let floor = task_idle_floor_secs();
        if v != 0 && !(floor..=86_400).contains(&v) {
            return Err(PatchError::new(
                "task_idle_timeout_secs",
                format!(
                    "task_idle_timeout_secs 须为 0(关闭)或 {floor}..=86400(下限 = 单条命令上限 + 单次模型调用上限 + 1)"
                ),
            ));
        }
        s.task_idle_timeout_secs = v;
    }
    Ok(())
}
