// LLM 消息构建:6 层位置拼装(build_llm_messages_with_position)与测试兼容入口
// (build_llm_messages,仅 #[cfg(test)]);空楼层/空注入跳过语义见 MAINTENANCE.md,不变。
// (自 messages.rs 拆分迁入,纯代码移动,逻辑不变;本次再按职责细拆:
//  步骤派生 → steps.rs,位置拼装/前缀稳定/槽位布局回归测试 → build_tests.rs)
// 可见性说明:原 messages.rs 中 pub(super)(= 对 engine 可见)的导出条目在此改为
// pub(in crate::agents::engine),供 messages/mod.rs 以相同可见性再导出,范围不变。
use crate::agents::engine::worldbook::WorldInjection;
use crate::models::types::{ImageRef, LlmMessage};
use crate::parsing::assistant::AssistantVars;
use crate::parsing::macros::{expand_macros, MacroCtx};
use crate::services::prompt_inject_service::{FloorRole, InjectMode, PromptInjectConfig};
// 防注入包裹原语已下沉 services::prompt_kit(WP7),与任务模式共用同一实现
use crate::services::prompt_kit::untrusted_boundary;
// 人称锚定(POV-1):与预览/步骤/反思共用同一文案出处
use crate::services::pov::{parse_pov_mode, pov_anchor};
use std::collections::HashMap;

/// 位置1 激发条目的统一说明头(WB-C,2026-10-11):与常驻侧 [`CONSTANT_WORLD_HEADER`] 同口径,
/// 措辞按「本轮命中」区分。说明头落在 `untrusted_boundary` **之外**,条目组整体包成一块。
const TRIGGERED_WORLD_HEADER: &str = "世界书(本轮命中设定,必须严格遵循,冲突时以世界书为准):";

/// 位置3 常驻条目的统一说明头(WB-E 批 2,遗留 L32):文案沿用原**内置默认模板**里那句字面
/// 「世界书(当前已命中的设定,…)」,改由引擎在 `{{world_info}}` 展开处拥有——**说明头随内容走**:
/// 有常驻条目正文才出头,只剩状态块或全空时整段消失(不再句下空转)。与
/// [`TRIGGERED_WORLD_HEADER`] 同构,同样落在 `untrusted_boundary` **之外**。
/// 默认模板(`settings_service::params::default_roleplay_agent_prompt`)因此删掉了那行字面文案,
/// 存量逐字旧默认由 `rehydrate_default_roleplay_prompt` 升级,避免两处说明头。
const CONSTANT_WORLD_HEADER: &str = "世界书(当前已命中的设定,必须严格遵循,冲突时以世界书为准):";

/// 构建发送给 LLM 的消息数组(6 层规范落地的核心拼装点)。
/// 位置5(头部)= 系统提示词/主 agent 提示词;位置4 = 简单注入 + 复杂模式楼层;
/// 位置3 = 角色卡 + 世界书常态(constant);位置2 = 历史上下文;位置1 = 世界书激发(触发);
/// 位置0(尾部)= 预设尾部提示词。物理布局:
///   - system 消息 = 位置5 主提示词({{char}}/{{personality}}/{{scenario}}/{{world_info}} 宏展开)
///     + 位置4 系统角色楼层 + 位置3 角色卡 personality/scenario 与世界书常态(经宏/{{world_info}})
///   - 位置4 非系统角色楼层(user/assistant)紧随 system,按 order 升序
///   - 位置2 历史上下文原样
///   - 位置1 世界书激发 + 位置0 预设尾部追加到最新用户消息尾部(缓存友好:system+早期历史稳定)
///
/// 参数:world_constant = 位置3 世界书常态文本;world_triggered = 位置1 世界书激发文本;
/// preset_tail = 位置0 预设尾部提示词(空 = 禁用)。宏展开共享同一 mctx,
/// 保证 {{setvar}} 在前面的文本写入、{{getvar}} 在后面的文本能读到(时序与 ST prompt_order 一致)。
/// 返回 (messages, protected_tail):protected_tail 为拼入 system 末尾的注入文本/楼层块
/// 字符数(无注入为 0),供 trim_to_context 截断时优先保留(注入不能先于角色设定被切掉)。
#[allow(clippy::too_many_arguments)]
#[cfg(test)]
pub(in crate::agents::engine) fn build_llm_messages(
    character_name: &str,
    character_description: &str,
    personality: &str,
    scenario: &str,
    world_text: Option<&str>,
    history: &[(String, String)],
    custom_prompt: Option<&str>,
    inject: Option<&PromptInjectConfig>,
    vars: &mut HashMap<String, String>,
    assistant_vars: &mut AssistantVars,
) -> (Vec<LlmMessage>, usize) {
    // 兼容入口:旧单一世界书视为常态组(位置3,role=system);无尾部注入(测试与旧调用路径不变)
    // 测试用空作用域上下文(None 语义,行为与旧一致)
    let constant: Vec<WorldInjection> = world_text
        .map(|t| vec![WorldInjection::entry("system", t)])
        .unwrap_or_default();
    build_llm_messages_with_position(
        character_name,
        character_description,
        personality,
        scenario,
        &constant,
        &[],
        history,
        // 测试兼容入口不带图像(视觉能力包 D2 前既有行为)
        &[],
        custom_prompt,
        inject,
        None,
        "user",
        None,
        "user",
        vars,
        assistant_vars,
        None,
        // 文学能力包:测试兼容入口不带三段文本(行为与加参数前一致)
        LiteraryTexts::default(),
    )
}
/// 文学能力包的文本参数(LIT-2 / LIT-3)。**文本单一出处**在设置层
/// (`services::settings_service::params` 的常量)，由装配层(`messages/context.rs`)
/// 按扁平快照取好传入——build 层不读设置，保持纯粹、测试可直接注入字面文本。
///
/// - `default_suffix`：**兜底模板增强段**(LIT-2)——仅「用户清空提示词框」的内置兜底
///   路径生效；自定义/物化缺省提示词路径不由它追加；
/// - `tail`：**位置 4 system 尾增强段**(LIT-3)——任何路径都追加，计入 `protected_tail`，
///   不经 untrusted 包裹(强制语气)；
/// - `note`：**位置 0 Author's Note 段**(LIT-3)——随尾部角色落 user/assistant 尾，
///   经 `untrusted_boundary("literary_note")` 包裹(素材语气)，不计 `protected_tail`；
/// - `style`：**位置 0 文风素材段**(LIT-6)——与 `note` 同处位置 0、同样经 untrusted 包裹、
///   同样不计 `protected_tail`；拼装顺序钉死「激发 → 反思建议 → **style** → note → 预设尾部」
///   (AN 保持最后)。
///
/// 全 `None` 时行为与加本参数前**逐字节一致**(开关关的回归钉子)。
/// 结构体传参(而非逐个位置参数)：各段文本同源同判据，后续加字段不再改动各调用点。
#[derive(Default, Clone, Copy)]
pub(in crate::agents::engine) struct LiteraryTexts<'a> {
    pub(in crate::agents::engine) default_suffix: Option<&'a str>,
    pub(in crate::agents::engine) tail: Option<&'a str>,
    pub(in crate::agents::engine) note: Option<&'a str>,
    pub(in crate::agents::engine) style: Option<&'a str>,
}

/// 角色卡文本(CARD-1):description / personality / scenario / mes_example 同源同判据,
/// 用结构体收口,避免每加一个卡字段就改全部调用点。各字段在 build 层分别经
/// `untrusted_boundary("character_card.*")` 包裹。
#[derive(Default, Clone, Copy)]
pub(in crate::agents::engine) struct CardTexts<'a> {
    pub(in crate::agents::engine) description: &'a str,
    pub(in crate::agents::engine) personality: &'a str,
    pub(in crate::agents::engine) scenario: &'a str,
    pub(in crate::agents::engine) mes_example: &'a str,
}

/// 用户设定文本(USERPERSONA-1):称呼、用户角色原文与已包裹的【用户角色】段。
/// `block` 由装配层用 `user_persona_service::block()` 生成(单一出处),build 层只负责
/// 在模板之后、注入层之前拼入 system。
#[derive(Default, Clone, Copy)]
pub(in crate::agents::engine) struct PersonaTexts<'a> {
    pub(in crate::agents::engine) user_name: &'a str,
    pub(in crate::agents::engine) user_description: &'a str,
    pub(in crate::agents::engine) block: Option<&'a str>,
}

/// build_llm_messages 的 6 层位置版本(位置定义见 build_llm_messages 文档)。
/// 世界书常驻(role=system)并入 system;激发与预设尾部追加到最新用户消息尾部
/// (system + 早期历史前缀保持稳定 → 前缀缓存命中率显著提升,缓存友好)。
/// 各条目角色可自由选择:system 角色在尾部消息位置钳制为 user。
/// reflect_advice = 反思失败建议(位置0,自动生成的改进建议;注入到位置1 激发之后、
/// 预设尾部之前,不再作为最末尾内容);reflect_advice_role = user / assistant(system 钳制为 user)。
#[allow(clippy::too_many_arguments)]
pub(in crate::agents::engine) fn build_llm_messages_with_card(
    character_name: &str,
    card: CardTexts<'_>,
    persona: PersonaTexts<'_>,
    world_constant: &[WorldInjection],
    world_triggered: &[WorldInjection],
    history: &[(String, String)],
    // 与 history 逐条对齐的图像引用(视觉能力包 D2;空 slice = 无图,行为与加字段前一致)
    history_images: &[Vec<ImageRef>],
    custom_prompt: Option<&str>,
    inject: Option<&PromptInjectConfig>,
    preset_tail: Option<&str>,
    preset_tail_role: &str,
    reflect_advice: Option<&str>,
    reflect_advice_role: &str,
    vars: &mut HashMap<String, String>,
    assistant_vars: &mut AssistantVars,
    // 7 作用域变量(计划二);None = 无作用域上下文(宏/EJS 走旧行为)
    scopes: Option<&mut crate::parsing::scopes::ScopeVars>,
    // 文学能力包三段文本(LIT-2/LIT-3;见 LiteraryTexts 文档)。默认全 None 时与
    // 加本参数前逐字节一致。
    literary: LiteraryTexts<'_>,
) -> (Vec<LlmMessage>, usize) {
    let mut messages = Vec::new();
    // 角色卡字段属于不可信素材。只包裹宏展开后的替换值，不改写原始角色扮演文本。
    let bounded_description = untrusted_boundary("character_card.description", card.description);
    let bounded_personality = untrusted_boundary("character_card.personality", card.personality);
    let bounded_scenario = untrusted_boundary("character_card.scenario", card.scenario);
    // CARD-1:对话示例同属角色卡不可信素材;与 description/personality/scenario 同一边界规则
    let bounded_mes_example = untrusted_boundary("character_card.mes_example", card.mes_example);
    // USERPERSONA-1:用户角色设定属不可信素材,只包裹替换值,不改写原始文本。
    let bounded_user_description = untrusted_boundary("user_persona", persona.user_description);
    // 宏上下文:自定义系统提示词 / 简单合成 / 楼层 / 世界书共享同一 vars,按序展开,
    // 保证 {{setvar}} 在前面的文本写入、{{getvar}} 在后面的文本能读到(时序与 ST prompt_order 一致)。
    let mut mctx = MacroCtx {
        character_name,
        character_description: &bounded_description,
        user_name: persona.user_name,
        user_description: &bounded_user_description,
        user_input: "",
        personality: &bounded_personality,
        scenario: &bounded_scenario,
        mes_example: &bounded_mes_example,
        history,
        vars,
        assistant_vars: Some(assistant_vars),
        scopes,
    };
    // 无用户消息时(空历史/仅 assistant 历史):位置1 激发与位置0 预设尾部没有可追加的
    // user 消息,退化为并入 system 兜底(避免凭空新增 user 消息干扰对话流)。
    let has_user = history.iter().any(|(r, _)| r == "user");
    // 文学能力包(LIT-3):位置 0 Author's Note 段(空则 None)。经 untrusted 包裹 = 素材语气
    // (强制语气归位置 4 段);不计 protected_tail —— 诚实边界:极端裁剪下可被切。
    let literary_note = literary
        .note
        .filter(|n| !n.trim().is_empty())
        .map(|n| untrusted_boundary("literary_note", n));
    // 文学能力包(LIT-6):位置 0 文风素材段(空则 None)。与 AN 段同处位置 0、同样经
    // untrusted 包裹;拼装顺序钉死「激发 → 反思建议 → 文风 → AN → 预设尾部」(AN 最后)。
    let literary_style = literary
        .style
        .filter(|n| !n.trim().is_empty())
        .map(|n| untrusted_boundary("literary_style", n));
    // 位置3 常驻 system 角色 → 并入 system 文本;无 user 时激发/预设尾部也并入兜底
    let sys_world: String = if has_user {
        world_constant
            .iter()
            .filter(|i| i.role == "system")
            .map(|i| i.text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n")
    } else {
        let mut parts: Vec<String> = Vec::new();
        for inj in world_constant.iter().chain(world_triggered.iter()) {
            parts.push(inj.text.clone());
        }
        // 反思失败建议(位置0):激发之后、预设尾部之前
        if let Some(a) = reflect_advice {
            if !a.trim().is_empty() {
                parts.push(a.to_string());
            }
        }
        // 文学能力包(LIT-3/LIT-6):位置 0 文风段 → AN 段(退化分支并入 system 兜底;
        // 顺序同 live 分支:激发 → 反思建议 → 文风 → AN → 预设尾部)
        if let Some(s) = &literary_style {
            parts.push(s.clone());
        }
        if let Some(n) = &literary_note {
            parts.push(n.clone());
        }
        if let Some(t) = preset_tail {
            parts.push(t.to_string());
        }
        parts.join("\n\n")
    };
    // 常驻说明头随内容走(遗留 L32,WB-E 批 2):内置默认模板不再字面写那句「世界书(当前已命中的
    // 设定,…)」,该头由引擎拥有——**两条注入路径共用同一段判定与文案**(占位符路径与「留空即用
    // 内置兜底模板」路径在用户视角是同一件事,不该一条有头一条没头)。
    // 有常驻条目正文才出头;只剩变量状态块(全激发卡的常见形态)或全空时整段消失,
    // 不再留「句在、内容空」的悬空强制语气句;状态块是引擎产物,不套「必须严格遵循」的设定头(与 WB-C 同判据)。
    let constant_body_present = world_constant
        .iter()
        .any(|i| i.role == "system" && !i.state_block);
    let bounded_world = untrusted_boundary("world_book", &sys_world);
    let world_block = if constant_body_present && !bounded_world.is_empty() {
        format!("{CONSTANT_WORLD_HEADER}\n{bounded_world}")
    } else {
        bounded_world
    };
    let mut sys = match custom_prompt {
        Some(tpl) if !tpl.trim().is_empty() => {
            // {{world_info}} 宏层不认,先替换为位置3 世界书常驻文本;{{character_name}}/{{character_description}}
            // /{{char}}/{{user}}/{{random}}/{{getvar}} 等由宏系统统一展开
            let has_world_placeholder = tpl.contains("{{world_info}}");
            let pre = tpl.replace("{{world_info}}", &world_block);
            let mut s = expand_macros(&pre, &mut mctx);
            // 变量状态块兜底(2026-10-09 冲突修复):模板漏写 {{world_info}} 时,世界书**常驻
            // 条目**按「Custom Prompt 完整替换、不是追加」契约确实不注入(设置页已明示);
            // 但状态块是引擎状态而非世界书素材——漏掉它,模型看不到当前状态就永远不会输出
            // <UpdateVariable>,变量更新通道整体静默停摆。此处只补回状态块,世界书条目语义不变。
            // 口径与占位符路径一致:同样经宏展开(变量值里的宏两路径同解),同样经
            // world_book 边界包裹(占位符路径的整段 sys_world 也包在其中)。
            if !has_world_placeholder {
                if let Some(block) = world_constant
                    .iter()
                    .find(|i| i.state_block && i.role == "system")
                {
                    if !block.text.trim().is_empty() {
                        let expanded = expand_macros(&block.text, &mut mctx);
                        s.push_str(&format!(
                            "\n\n{}",
                            untrusted_boundary("world_book", &expanded)
                        ));
                    }
                }
            }
            s
        }
        _ => {
            // 内置默认模板(用户清空自定义系统提示词时兜底):文学创作定位,融合「可待」预设精华
            // (创作总纲/不转述/独立角色/基础文风/模块化剧情),并保留 kedai 变量更新协议。
            let mut s = format!(
                "你是角色「{character_name}」的扮演者与文学创作者,与用户进行沉浸式角色扮演 / 文学创作。"
            );
            if !card.description.is_empty() {
                s.push_str(&format!(
                    "\n\n{}",
                    untrusted_boundary(
                        "character_card",
                        &format!("背景设定:\n{}", card.description)
                    )
                ));
            }
            if !bounded_personality.is_empty() {
                s.push_str(&format!("\n\n性格:\n{bounded_personality}"));
            }
            if !bounded_scenario.is_empty() {
                s.push_str(&format!("\n\n场景:\n{bounded_scenario}"));
            }
            if !bounded_mes_example.is_empty() {
                s.push_str(&format!("\n\n【对话示例】\n{bounded_mes_example}"));
            }
            if !world_block.is_empty() {
                s.push_str(&format!("\n\n{world_block}"));
            }
            s.push_str(
                "\n\n【创作总纲】\n\
                 本会话定位为文学创作系统:你的正文是小说文本而非聊天记录,以\"能否被称为一段好小说\"为最低验收标准。描写应当可朗读、可回味、经得起推敲。\n\n\
                 【角色扮演规则】\n\
                 1. 视角与口吻:按系统提示词中的人称锚定输出;未给出锚定时,以第三人称限知、镜头贴住角色叙述,角色台词保持角色第一人称口吻;不要出现旁白标题、「以上是回复」「作为AI」等元文本;角色设定与世界观保持一致。\n\
                 2. 用户指令优先:用户提出的字数、风格、视角、情节走向要求必须服从;用户提问必须在本轮正面回答,不允许回避。\n\
                 3. 不转述:用户输入的动作与对话视为已经发生,不得复述或引用,直接从其后无缝衔接继续创作新的剧情。\n\
                 4. 主角与独立角色:用户所操控角色是剧情主角,但其台词、内心与决定由用户掌握;你不得代写或替其做决定,只写其他角色与环境对它的反应。所有角色都是独立的人,有自己的价值观、思考方式与喜好,不会无条件依附用户,好感度不会因小事凭空上涨或下降。\n\
                 5. 推进节奏:一次输出不得把当前事件直接推进至末尾,应当适当拆分事件、合理安排节奏,在正文末尾为用户留下可互动的窗口。\n\n\
                 【写作要求】\n\
                 1. 句式:长短句合理搭配;段落之间空一行;适当插入短段,不得连续堆叠对话;各段长度合理交错。\n\
                 2. 描写:区分周围描写(环境)与聚焦描写(重点物),合理穿插;相同环境描写不得重复提及;拒绝堆砌\"自然\"\"仿佛\"\"似乎\"等空泛修饰与过度精确化的机械描写;比喻须贴切,不把物比作与其无关的事物。\n\
                 3. 对话:口语化,话题连贯不重复;所有说出口的对话必须用中文双引号\"...\"包裹。\n\
                 4. 规避:禁止先否后肯句式(不是……而是……)、动物比喻、元评论(用括号解释正文)、夸张化描写;非必要不描写回忆,尤其不得复述与上文类似的回忆。\n\
                 5. 开头结尾:每次输出的开头与结尾都应当新颖,不得与上一次输出类似或重复,不得强行升华。\n\
                 6. 不得以任何方式描述任何角色的具体年龄。\n\
                 7. 角色对话必须符合其性格与设定;有对话示例时,口吻与句式贴近示例,但不得复述示例。\n\
                 8. 逻辑一致:严格遵守时间/对话/行为/变量/剧情的逻辑;角色不得知晓不该知道的设定信息,不得出现\"根据XX的设定\"这类作者视角发言。\n\n\
                 【剧情结构(模块化)】\n\
                 每次输出将剧情分为三个剧情模块和一个结尾模块,模块之间以过渡段承转;模块类型在【环境/推进/插入】中选择,同类型不得连续出现三次;模块与结尾的格式、内容不得与上一次输出相似或雷同;禁止在正文中对模块或过渡部分做任何标注。用户明确要求不分模块、或本轮为对话/极短篇幅时,本条不适用。\n\n\
                 【输出纪律】\n\
                 一次只输出角色回应本身;若需要分段,使用空行,不使用 Markdown 标题。",
            );
            // 文学能力包(LIT-2):兜底模板追加文学增强段(开关开时由装配层传入;
            // 文本不在此复制,单一出处见 settings_service::params)
            if let Some(extra) = literary.default_suffix {
                if !extra.trim().is_empty() {
                    s.push_str(&format!("\n\n{extra}"));
                }
            }
            s
        }
    };
    // USERPERSONA-1:用户显式启用的全局层——与「Custom Prompt 完整替换」并列,
    // 模板之后、simple/complex 注入之前。任务模式不经本函数,天然隔离。
    if let Some(block) = persona.block {
        if !block.trim().is_empty() {
            sys.push_str(&format!("\n\n{block}"));
        }
    }
    // 拼入 system 尾部的注入文本总长度(截断保护用)
    let mut protected_tail: usize = 0;

    // 位置4 简单模式:合成四项注入文本,宏展开后拼入 system 末尾
    if let Some(cfg) = inject {
        if cfg.mode == InjectMode::Simple {
            let text = cfg.simple_inject_text();
            if !text.is_empty() {
                let expanded = expand_macros(&text, &mut mctx);
                protected_tail += expanded.chars().count();
                sys.push_str(&format!("\n\n{expanded}"));
            }
        }
    }

    // 位置4 复杂模式:楼层按 order 顺序展开(共享 mctx),统一归位位置4——
    // role=system 的内容拼入系统提示词;user/assistant 角色紧随 system 按 order 排
    // (废弃旧 before/after/depth 历史内散插;旧配置 position 字段仅兼容解析,不再参与注入)。
    let mut floors_in_chat: Vec<(String, String)> = Vec::new();
    if let Some(cfg) = inject {
        if cfg.mode == InjectMode::Complex {
            for floor in cfg.enabled_floors_sorted() {
                let content = expand_macros(&floor.content, &mut mctx);
                if matches!(floor.role, FloorRole::System) {
                    // system 角色始终进系统提示词(避免对话中间夹 system 消息);
                    // 空内容跳过(与下方非 system 分支同口径):导入 ST 预设的空锚点/
                    // 组头块展开后为空,继续拼 "\n\n" 会在系统提示词尾部留下空白。
                    if !content.trim().is_empty() {
                        protected_tail += content.chars().count();
                        sys.push_str(&format!("\n\n{content}"));
                    }
                } else {
                    floors_in_chat.push((floor.role.as_str().to_string(), content));
                }
            }
            // 禁词库自省提示:所有模式生效(简单模式由 simple_inject_text 追加,复杂模式在此追加)。
            // 机械空替换删除已退役(RPFLOW-2,观感不自然):deep/agent 由反思纪律段 +
            // 反思模型的工具同义改写承担,custom 走 reflect_with_tools(同段纪律);
            // 其余模式仅注入本自省提示,不在引擎侧做任何机械替换。
            let banned_hint = cfg.simple.banned_words_hint();
            if !banned_hint.is_empty() {
                let expanded = expand_macros(&banned_hint, &mut mctx);
                protected_tail += expanded.chars().count();
                sys.push_str(&format!("\n\n{expanded}"));
            }
        }
    }

    // 文学能力包(LIT-3):位置 4 增强段 —— 插在 system 压入消息**之前**(此后 sys 不再使用),
    // 计入 protected_tail(极端裁剪下不可被切);不经 untrusted 包裹(可用强制语气)。
    // 段文本同轮恒定(无时间戳/随机序),不破坏前缀缓存假设。
    if let Some(t) = literary.tail {
        if !t.trim().is_empty() {
            protected_tail += t.chars().count();
            sys.push_str(&format!("\n\n{t}"));
        }
    }

    // 人称锚定(POV-1):位置 4 最后一条静态指令。简单/复杂模式均发——配置了可识别视角
    // 发显式版(强制覆盖提示词/预设/上文),未配置或未知值发兜底版(让位给会话提示词/
    // 预设/用户本轮输入)。计入 protected_tail:极端裁剪下不得先于注入层被切掉。
    // 注意:锚定必须排在文学尾段之后——它是本轮最后一条静态人称指令,recency 最强。
    if let Some(cfg) = inject {
        let mode = if cfg.simple.perspective_enabled {
            parse_pov_mode(&cfg.simple.perspective)
        } else {
            None
        };
        let user_name = if persona.user_name.trim().is_empty() || persona.user_name == "用户" {
            None
        } else {
            Some(persona.user_name)
        };
        let anchor = pov_anchor(mode, character_name, user_name);
        protected_tail += anchor.chars().count();
        sys.push_str(&format!("\n\n{anchor}"));
    }

    messages.push(LlmMessage::plain("system", &sys));
    // 位置4 非系统角色楼层:紧随 system,保持楼层 order 顺序
    // (空内容跳过:导入 ST 预设时纯 {{addvar}} 累积宏楼层展开为空,避免产生空 user/assistant 消息)
    for (role, content) in &floors_in_chat {
        if !content.trim().is_empty() {
            messages.push(LlmMessage::plain(role, content));
        }
    }
    // 位置3 常驻非系统角色(role=user/assistant):作独立消息紧跟位置4(按注入顺序)
    if has_user {
        for inj in world_constant.iter().filter(|i| i.role != "system") {
            let expanded = expand_macros(&inj.text, &mut mctx);
            if !expanded.trim().is_empty() {
                messages.push(LlmMessage::plain(
                    &inj.role,
                    &untrusted_boundary("world_book", &expanded),
                ));
            }
        }
    }

    // 位置2 历史上下文 + 位置1 世界书激发 + 位置0 预设尾部
    if let Some(idx) = history.iter().rposition(|(r, _)| r == "user") {
        // 尾部角色:system 在尾部消息位置钳制为 user(尾部无 system 消息概念)
        let tail_role = if preset_tail_role == "assistant" {
            "assistant"
        } else {
            "user"
        };
        // 位置1 user 激发(含 system 钳制为 user)+ 位置0 反思建议 + 位置0 预设尾部(user)→ 拼最新 user 消息尾部
        // WB-C(2026-10-11 决策 D4):普通条目与常驻同口径——先落统一说明头,整组经
        // untrusted_boundary("world_book") 包成单块(旧实现为裸 `[comment]\ncontent` 拼接,
        // 无头无包裹,注入内容不可归因;常驻侧的「必须严格遵循」句在全激发卡上还会悬空)。
        // **引擎变量状态块不参与包裹**:它是引擎产物、承载 <UpdateVariable> 更新协议,
        // 包进「不可信素材」边界等于自我削弱。尾部顺序固定:
        // 头 → 条目组 → 状态块 → 反思建议 → 文风段 → AN 段 → 预设尾部。
        let mut triggered_entries: Vec<String> = Vec::new();
        let mut state_blocks: Vec<String> = Vec::new();
        for inj in world_triggered.iter().filter(|i| i.role != "assistant") {
            let expanded = expand_macros(&inj.text, &mut mctx);
            if expanded.trim().is_empty() {
                continue;
            }
            if inj.state_block {
                state_blocks.push(expanded);
            } else {
                triggered_entries.push(expanded);
            }
        }
        let mut user_tail: Vec<String> = Vec::new();
        if !triggered_entries.is_empty() {
            user_tail.push(TRIGGERED_WORLD_HEADER.to_string());
            user_tail.push(untrusted_boundary(
                "world_book",
                &triggered_entries.join("\n\n"),
            ));
        }
        user_tail.extend(state_blocks);
        // 反思失败建议(位置0):自动生成的改进建议,注入在激发之后、预设尾部之前。
        // 按 reflect_advice_role 选边:user 并入最新 user 消息尾部;assistant 走下方 assistant_tail。
        if reflect_advice_role != "assistant" {
            if let Some(a) = reflect_advice {
                if !a.trim().is_empty() {
                    user_tail.push(a.to_string());
                }
            }
        }
        // 文学能力包(LIT-3/LIT-6):位置 0 文风段 → AN 段(反思建议之后、预设尾部之前;
        // 随尾部角色,与该侧预设尾部同命 —— 都在位置 0、都可被极端裁剪切掉)
        if tail_role == "user" {
            if let Some(s) = &literary_style {
                user_tail.push(s.clone());
            }
            if let Some(n) = &literary_note {
                user_tail.push(n.clone());
            }
        }
        if let Some(t) = preset_tail {
            if tail_role == "user" {
                let expanded = expand_macros(t, &mut mctx);
                if !expanded.trim().is_empty() {
                    user_tail.push(untrusted_boundary("world_book", &expanded));
                }
            }
        }
        let user_tail_text = if user_tail.is_empty() {
            None
        } else {
            Some(user_tail.join("\n\n"))
        };
        // 位置1 assistant 激发 + 位置0 反思建议 + 位置0 预设尾部(assistant)→ 独立 assistant 消息(最新 user 之后)
        let mut assistant_tail: Vec<String> = Vec::new();
        for inj in world_triggered.iter().filter(|i| i.role == "assistant") {
            let expanded = expand_macros(&inj.text, &mut mctx);
            if !expanded.trim().is_empty() {
                assistant_tail.push(expanded);
            }
        }
        // 反思失败建议(位置0,assistant 角色):激发之后、预设尾部之前
        if reflect_advice_role == "assistant" {
            if let Some(a) = reflect_advice {
                if !a.trim().is_empty() {
                    assistant_tail.push(a.to_string());
                }
            }
        }
        // 文学能力包(LIT-3/LIT-6):位置 0 文风段 → AN 段(assistant 侧对称:反思建议之后、
        // 预设尾部之前)
        if tail_role == "assistant" {
            if let Some(s) = &literary_style {
                assistant_tail.push(s.clone());
            }
            if let Some(n) = &literary_note {
                assistant_tail.push(n.clone());
            }
        }
        if let Some(t) = preset_tail {
            if tail_role == "assistant" {
                let expanded = expand_macros(t, &mut mctx);
                if !expanded.trim().is_empty() {
                    assistant_tail.push(untrusted_boundary("world_book", &expanded));
                }
            }
        }
        for (i, (role, content)) in history.iter().enumerate() {
            // history 中的 system 消息与既有行为一致跳过
            if role == "system" {
                continue;
            }
            let mut final_content = content.clone();
            if i == idx {
                if let Some(t) = &user_tail_text {
                    final_content.push_str(&format!("\n\n{t}"));
                }
            }
            let mut msg = LlmMessage::plain(role, &final_content);
            // 视觉能力包 D2:图像引用按 index 挂到对应历史消息上(无图为空 vec,序列化省略)
            if let Some(imgs) = history_images.get(i) {
                msg.images = imgs.clone();
            }
            messages.push(msg);
            // assistant 尾部注入紧跟最新 user 消息之后(位置1 → 位置0 顺序)
            if i == idx {
                for at in &assistant_tail {
                    messages.push(LlmMessage::plain("assistant", at));
                }
            }
        }
    } else {
        // 无 user 消息:激发与预设尾部已并入 system,历史原样输出
        for (i, (role, content)) in history.iter().enumerate() {
            if role == "system" {
                continue;
            }
            let mut msg = LlmMessage::plain(role, content);
            if let Some(imgs) = history_images.get(i) {
                msg.images = imgs.clone();
            }
            messages.push(msg);
        }
    }
    (messages, protected_tail)
}

/// 测试兼容入口:旧签名(无 mes_example)。生产走 `build_llm_messages_with_card`;
/// 保留本函数供既有 build 测试的 23 处调用,避免为「空示例」机械改全部调用点。
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(in crate::agents::engine) fn build_llm_messages_with_position(
    character_name: &str,
    character_description: &str,
    personality: &str,
    scenario: &str,
    world_constant: &[WorldInjection],
    world_triggered: &[WorldInjection],
    history: &[(String, String)],
    history_images: &[Vec<ImageRef>],
    custom_prompt: Option<&str>,
    inject: Option<&PromptInjectConfig>,
    preset_tail: Option<&str>,
    preset_tail_role: &str,
    reflect_advice: Option<&str>,
    reflect_advice_role: &str,
    vars: &mut HashMap<String, String>,
    assistant_vars: &mut AssistantVars,
    scopes: Option<&mut crate::parsing::scopes::ScopeVars>,
    literary: LiteraryTexts<'_>,
) -> (Vec<LlmMessage>, usize) {
    build_llm_messages_with_card(
        character_name,
        CardTexts {
            description: character_description,
            personality,
            scenario,
            mes_example: "",
        },
        PersonaTexts {
            user_name: "用户",
            user_description: "",
            block: None,
        },
        world_constant,
        world_triggered,
        history,
        history_images,
        custom_prompt,
        inject,
        preset_tail,
        preset_tail_role,
        reflect_advice,
        reflect_advice_role,
        vars,
        assistant_vars,
        scopes,
        literary,
    )
}
