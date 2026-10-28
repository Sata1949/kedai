// 步骤级消息视图与 POV 判定基准（POV-1 文件大小治理：自 run_loop.rs 迁入）。
// 纯代码搬移，逻辑不变；run_loop.rs 只保留主状态机循环。
use super::*;

/// 构造步骤级消息视图(RPFLOW-1:草稿步与既有生成步共用同一装配语义):
/// 先把 scopes 镜像同步到当前变量树(步骤提示词的 getvar 类宏能读到本步骤之前的
/// setvar 副作用),再把步骤 system_prompt 宏展开后以「[本步指令]」追加到 system 末尾。
/// 返回独立视图,不污染共享 llm_messages(反思回退后按步骤重建、无中间消息残留)。
pub(super) fn step_messages(
    rctx: &mut RunContext<'_>,
    ctx: &CollectedCtx,
    user_input: &str,
    step: &crate::models::types::PlanStep,
) -> Vec<LlmMessage> {
    {
        let mut s = rctx.scopes.lock().unwrap_or_else(|e| e.into_inner());
        s.sync_chat_tree(rctx.assistant_vars);
        s.sync_chat_flat(rctx.session_vars);
    }
    let mut step_msgs = rctx.llm_messages.clone();
    if step
        .system_prompt
        .as_deref()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
    {
        let mut scopes_guard = rctx.scopes.lock().unwrap_or_else(|e| e.into_inner());
        let mut mctx = MacroCtx {
            character_name: &ctx.chara_name,
            character_description: &ctx.chara_desc,
            user_name: "用户",
            user_input,
            personality: &ctx.personality,
            scenario: &ctx.scenario,
            mes_example: &ctx.mes_example,
            history: &ctx.history_tuples,
            vars: &mut *rctx.session_vars,
            assistant_vars: Some(&mut *rctx.assistant_vars),
            scopes: Some(&mut *scopes_guard),
        };
        let anchor = current_pov_anchor(ctx);
        step_msgs = with_step_prompt(rctx.llm_messages, step, &mut mctx, Some(&anchor));
    }
    step_msgs
}

/// 本轮人称锚定(单一计算点):步骤指令与反思共用同一判据。
/// 简单/复杂模式同判据——配置了可识别视角发显式版,否则发兜底版。
/// PR3(USERPERSONA-1)起把用户称呼作为第三参数传入;当前为默认「你」。
pub(super) fn current_pov_anchor(ctx: &CollectedCtx) -> String {
    let mode = if ctx.inject_snapshot.simple.perspective_enabled {
        crate::services::pov::parse_pov_mode(&ctx.inject_snapshot.simple.perspective)
    } else {
        None
    };
    crate::services::pov::pov_anchor(mode, &ctx.chara_name, None)
}

/// 反思判定基准:简单模式四项(宏展开,不含禁词);复杂模式返回空串
/// (楼层要求由用户自负,登记 POV-L6)。
pub(super) fn current_inject_requirements(rctx: &mut RunContext<'_>, ctx: &CollectedCtx) -> String {
    if ctx.inject_snapshot.mode != InjectMode::Simple {
        return String::new();
    }
    let text = ctx.inject_snapshot.simple_items_text();
    if text.trim().is_empty() {
        return text;
    }
    let mut scopes_guard = rctx.scopes.lock().unwrap_or_else(|e| e.into_inner());
    let mut mctx = MacroCtx {
        character_name: &ctx.chara_name,
        character_description: &ctx.chara_desc,
        user_name: "用户",
        user_input: "",
        personality: &ctx.personality,
        scenario: &ctx.scenario,
        mes_example: &ctx.mes_example,
        history: &ctx.history_tuples,
        vars: rctx.session_vars,
        assistant_vars: Some(rctx.assistant_vars),
        scopes: Some(&mut *scopes_guard),
    };
    crate::parsing::macros::expand_macros(&text, &mut mctx)
}
