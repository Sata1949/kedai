// 自定义 Agent 执行流程服务(custom 模式):用户可编辑步骤序列(名称/目标/生成开关/
// 反思/系统提示词/温度/输出上限/工具白名单),持久化到 data/agent_flows.json。
// 支持多流程库:可新建/复制/选择/删除/导入/导出流程,当前选中流程决定 custom 模式行为。
// 与提示词注入服务同构:全局作用域、内存缓存配置、全量读写。
use crate::models::types::PlanStep;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// 自定义执行流程服务:持有 data_dir,内存缓存流程库,读写 data/agent_flows.json
pub struct AgentFlowService {
    data_dir: PathBuf,
    library: AgentFlowLibrary,
    registered_tools: BTreeSet<String>,
}

impl AgentFlowService {
    pub fn new(data_dir: PathBuf, registered_tools: Vec<String>) -> Self {
        let library = AgentFlowLibrary::load(&data_dir);
        AgentFlowService {
            data_dir,
            library,
            registered_tools: registered_tools.into_iter().collect(),
        }
    }

    /// 当前选中的流程(未选择时 None → custom 模式不生效)
    pub fn get(&self) -> Option<&AgentFlowConfig> {
        let id = self.library.current_flow_id.as_ref()?;
        self.library.flows.iter().find(|f| &f.id == id)
    }

    pub fn get_library(&self) -> &AgentFlowLibrary {
        &self.library
    }

    /// 使用启动时已注册工具集合校验流程,供保存与执行前复用。
    pub fn validate(&self, config: &AgentFlowConfig) -> Result<(), String> {
        validate_flow(config, &self.registered_tools)
    }

    /// 保存(创建或更新)流程并设为当前选中;id 为空时自动生成新 id(新建)。
    /// 校验失败返回 Err(不落盘)。
    pub fn set(&mut self, mut config: AgentFlowConfig) -> Result<(), String> {
        validate_flow(&config, &self.registered_tools)?;
        if config.id.trim().is_empty() {
            config.id = Uuid::new_v4().to_string();
        }
        if config.name.trim().is_empty() {
            config.name = "未命名流程".into();
        }
        match self.library.flows.iter_mut().find(|f| f.id == config.id) {
            Some(existing) => *existing = config.clone(),
            None => self.library.flows.push(config.clone()),
        }
        self.library.current_flow_id = Some(config.id);
        self.save_library()
    }

    /// 选择某个流程为当前流程(id 不存在返回 Err)
    pub fn select(&mut self, id: &str) -> Result<(), String> {
        if !self.library.flows.iter().any(|f| f.id == id) {
            return Err(format!("流程不存在:{}", id));
        }
        self.library.current_flow_id = Some(id.to_string());
        self.save_library()
    }

    /// 删除流程;删除当前选中时回退到第一个流程(库空则 None)。至少保留一个流程时不允许删空。
    pub fn remove(&mut self, id: &str) -> Result<(), String> {
        let before = self.library.flows.len();
        self.library.flows.retain(|f| f.id != id);
        if self.library.flows.len() == before {
            return Err(format!("流程不存在:{}", id));
        }
        if self.library.current_flow_id.as_deref() == Some(id) {
            self.library.current_flow_id = self.library.flows.first().map(|f| f.id.clone());
        }
        self.save_library()
    }

    /// 持久化流程库到 data/agent_flows.json(原子写:崩溃不留半截 JSON)
    fn save_library(&self) -> Result<(), String> {
        let text = serde_json::to_string_pretty(&self.library).map_err(|e| e.to_string())?;
        crate::utils::fs_atomic::write_atomic(
            &self.data_dir.join("agent_flows.json"),
            text.as_bytes(),
        )
        .map_err(|e| e.to_string())
    }
}

/// 流程库(全局):当前选中的流程 + 全部流程
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentFlowLibrary {
    /// 当前选中的流程 id(None = 未选择,custom 模式不生效)
    #[serde(default)]
    pub current_flow_id: Option<String>,
    /// 全部流程(按数组顺序展示)
    #[serde(default)]
    pub flows: Vec<AgentFlowConfig>,
}

/// 自定义执行流程配置(单个流程)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentFlowConfig {
    /// 流程 id(库内唯一;旧格式迁移时自动生成)
    #[serde(default)]
    pub id: String,
    /// 流程名称(选择器展示)
    #[serde(default)]
    pub name: String,
    /// 流程说明(可选)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 是否启用自定义流程(chat/send 的 agent_mode=custom 需要 enabled=true 才生效)
    #[serde(default)]
    pub enabled: bool,
    /// 步骤序列(按数组顺序展示;执行顺序见 `resolve_graph` 的拓扑序)
    #[serde(default)]
    pub steps: Vec<PlanStep>,
    /// 并行节点数上限(二维批次 2):缺省 = `DEFAULT_MAX_PARALLEL_NODES`;范围 1-8
    /// (1 = 完全串行)。落在**流程级**而非全局设置:并发度是流程自身的成本画像,
    /// 且不触碰 settings.json 的双模式继承/隔离契约。并行会成倍消耗 token(WF-11)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_parallel_nodes: Option<u32>,
}

/// 并行节点数上限的默认值与上限(二维批次 2;产品风险 WF-11:
/// 并行 = 成本倍增,故默认保守取 2,并在前端给出提示)
pub const DEFAULT_MAX_PARALLEL_NODES: u32 = 2;
pub const MAX_PARALLEL_NODES_LIMIT: u32 = 8;

/// 生效的并行上限:缺省取默认值,越界取边界(1 = 完全串行,行为与二维批次 1 一致)
pub fn effective_max_parallel(cfg: &AgentFlowConfig) -> usize {
    cfg.max_parallel_nodes
        .unwrap_or(DEFAULT_MAX_PARALLEL_NODES)
        .clamp(1, MAX_PARALLEL_NODES_LIMIT) as usize
}

impl AgentFlowLibrary {
    /// 从 data/agent_flows.json 加载;缺失/损坏记录日志后回退默认(不静默)。
    /// 兼容旧单流程格式 `{enabled, steps}`:自动迁移为库(旧步骤保留)。
    pub fn load(data_dir: &Path) -> Self {
        let path = data_dir.join("agent_flows.json");
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                // 按顶层字段预判格式:含 flows/current_flow_id 为库格式,否则为旧单流程格式
                // (库结构字段全部带 default,直接按库解析会误吞旧格式 → 必须先预判)
                let value: Value = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::error!(
                            error = e.to_string(),
                            "自定义流程配置解析失败,已回退默认配置"
                        );
                        return AgentFlowLibrary::default();
                    }
                };
                if value.get("flows").is_some() || value.get("current_flow_id").is_some() {
                    match serde_json::from_value::<AgentFlowLibrary>(value) {
                        Ok(lib) => finalize_library(lib),
                        Err(e) => {
                            tracing::error!(
                                error = e.to_string(),
                                "自定义流程配置解析失败,已回退默认配置"
                            );
                            AgentFlowLibrary::default()
                        }
                    }
                } else {
                    // 旧版单流程格式 → 迁移为流程库
                    match serde_json::from_value::<AgentFlowConfig>(value) {
                        Ok(mut old) => {
                            tracing::info!("检测到旧版单流程配置,已迁移为流程库");
                            if old.id.trim().is_empty() {
                                old.id = Uuid::new_v4().to_string();
                            }
                            if old.name.trim().is_empty() {
                                old.name = "默认流程".into();
                            }
                            // 旧版空配置(从未编辑过)→ 替换为内置协调流程,开箱即用
                            if old.steps.is_empty() {
                                old = builtin_flow();
                            }
                            let lib = AgentFlowLibrary {
                                current_flow_id: Some(old.id.clone()),
                                flows: vec![old],
                            };
                            if let Err(e) = lib.save(data_dir) {
                                tracing::error!(error = e, "旧配置迁移写入失败");
                            }
                            lib
                        }
                        Err(e) => {
                            tracing::error!(
                                error = e.to_string(),
                                "自定义流程配置解析失败,已回退默认配置"
                            );
                            AgentFlowLibrary::default()
                        }
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // 首次运行:注入内置协调流程,开箱即用
                let lib = AgentFlowLibrary {
                    current_flow_id: Some(builtin_flow().id.clone()),
                    flows: vec![builtin_flow()],
                };
                let _ = lib.save(data_dir);
                lib
            }
            Err(e) => {
                tracing::error!(
                    error = e.to_string(),
                    "自定义流程配置读取失败,已回退默认配置"
                );
                AgentFlowLibrary::default()
            }
        }
    }

    /// 持久化流程库到 data/agent_flows.json(原子写:崩溃不留半截 JSON)
    pub fn save(&self, data_dir: &Path) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        crate::utils::fs_atomic::write_atomic(&data_dir.join("agent_flows.json"), text.as_bytes())
            .map_err(|e| e.to_string())
    }
}

/// 库加载收尾:未选择流程时默认指向第一个(升级/迁移兜底)
fn finalize_library(mut lib: AgentFlowLibrary) -> AgentFlowLibrary {
    if lib.current_flow_id.is_none() && !lib.flows.is_empty() {
        lib.current_flow_id = lib.flows.first().map(|f| f.id.clone());
    }
    lib
}

/// 内置默认流程:与 Kedai harness 协调的文学创作/角色扮演流程。
/// 三步:起草正文(先写 ≤200 字计划再输出正文)→ 反思(判定 PASS/FAIL)→ 修订(生成)。
/// 步骤级提示词使用 Kedai 酒馆宏与 mvu 变量协议,与 custom 模式引擎语义对齐。
pub fn builtin_flow() -> AgentFlowConfig {
    AgentFlowConfig {
        id: "builtin-coordination".into(),
        name: "文学创作协调流程".into(),
        description: Some(
            "Kedai 内置协调流程:起草正文(先计划后正文)→ 反思质量 → 修订输出;与酒馆宏、世界书、mvu 变量协议(function calling 优先)对齐。"
                .into(),
        ),
        enabled: true,
        // 并行上限缺省 = DEFAULT_MAX_PARALLEL_NODES(2):内置流程是线性三步,
        // 并发度无实际影响,留 None 以便跟随默认值演进
        max_parallel_nodes: None,
        steps: vec![
            PlanStep {
                id: "draft".into(),
                name: "起草正文".into(),
                enabled: true,
                goal: "先撰写 ≤200 字写作计划,再按计划生成正文,服从用户字数/视角要求".into(),
                action: "direct".into(),
                generates: Some(true),
                system_prompt: Some(
                    "【本步指令·计划并起草】开始输出正文前,先在草稿区撰写一份 200 字以内的写作计划\
                     (段落结构、核心要点、情节走向、节奏安排),随后严格按该计划输出正文;\
                     正文中不得包含计划本身,不得出现「计划」「草稿」等字样。\
                     以 {{char}} 的视角与口吻输出:视角遵循系统提示词与用户要求\
                     (用户未指定时以角色自身视角叙述);不要出现旁白标题、「以上是回复」等元文本。\
                     服从用户的字数与风格要求;用户提问必须正面回答。\
                     变量更新优先通过 function calling 工具(update_variables)完成,模型未走工具时\
                     才回退 <UpdateVariable> 文本协议(JSONPatch 数组,支持 replace / delta / insert / remove / move,\
                     路径对应当前状态的键);无变化则不输出;纯状态更新时正文可以为空。除该块外不得使用任何自定义标签。"
                        .into(),
                ),
                tools: Some(vec!["update_variables".into()]),
                tool_choice: Some("auto".into()),
                ..Default::default()
            },
            PlanStep {
                id: "reflect".into(),
                name: "反思质量".into(),
                enabled: true,
                goal: "检查草稿:字数是否达标、疑问是否回答、视角是否一致、是否被截断;判定输出 PASS 或 FAIL".into(),
                action: "reflect".into(),
                generates: None,
                ..Default::default()
            },
            PlanStep {
                id: "revise".into(),
                name: "修订输出".into(),
                enabled: true,
                goal: "按反思结论修订并输出最终正文;未发现问题时直接输出原草稿".into(),
                action: "direct".into(),
                generates: Some(true),
                system_prompt: Some(
                    "【本步指令·修订输出】基于反思结论修订上一版草稿:修复缺陷后再次以 {{char}} 视角完整输出最终正文;\
                     若反思结论为通过,原样输出草稿。任何情况下都不得输出反思过程本身。\
                     变量更新同样优先经 function calling 工具(update_variables),未走工具时回退 <UpdateVariable> 文本协议,\
                     格式与起草步骤相同。"
                        .into(),
                ),
                tools: Some(vec!["update_variables".into()]),
                tool_choice: Some("auto".into()),
                ..Default::default()
            },
        ],
    }
}

// ==================== 二维流程原语(二维批次 1) ====================
//
// **单一出处**:保存期校验(`validate_flow`)、任务侧执行器(`task_engine/custom.rs`)、
// 聊天侧线性化(`agents/planner.rs::make_custom_plan`)三处共用下面这些纯函数。
// 与 `prompt_kit` 的共享原语同一纪律:不得在任一侧复制实现(否则「什么算合法图」
// 会出现多个版本,前端能保存的图与能执行的图就会漂移)。

/// 流程图解析结果:所有下标一律对齐传入的 `steps` **数组顺序**。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowGraph {
    /// 每个步骤的有效上游下标(已去重、按下标升序)
    pub inputs: Vec<Vec<usize>>,
    /// 执行拓扑序(Kahn;就绪节点按下标升序出队)
    pub order: Vec<usize>,
}

/// 线性兼容模式判定:全部步骤的 `inputs` 均为空 → 语义回退为「按数组顺序串联」。
///
/// 该模式是**存量一维流程零迁移的保证**:线性模式下第 i 步的输入恒为第 i-1 步,
/// 拓扑序 == 数组顺序,生成消息格式与旧版逐字节一致(见 custom.rs 的单父模板)。
pub fn is_linear_compat(steps: &[PlanStep]) -> bool {
    steps.iter().all(|s| s.inputs.is_empty())
}

/// 解析每个步骤的有效输入集(逐个对应 `steps`)。
///
/// - 线性兼容模式:第 i 步输入 = 第 i-1 步(首步无输入);
/// - 二维模式:显式 `inputs` 按 id 解析;`inputs` 为空的步骤即**源节点**(只用任务目标)。
///
/// 校验(失败返回中文错误,供 400 展示):步骤 id 非空且唯一、上游必须存在且已启用
/// (调用方传入的已是启用步骤集)、不得自环。父节点下标按**升序**排列
/// (D3:多父合并顺序确定、可复现),且已去重(同一父写两次不产生重复段落)。
pub fn effective_inputs(steps: &[PlanStep]) -> Result<Vec<Vec<usize>>, String> {
    let mut index_of: BTreeMap<&str, usize> = BTreeMap::new();
    for (i, s) in steps.iter().enumerate() {
        if s.id.trim().is_empty() {
            return Err(format!("步骤「{}」缺少 id(二维流程按 id 连接)", s.name));
        }
        if index_of.insert(s.id.as_str(), i).is_some() {
            return Err(format!("步骤 id 重复:{}", s.id));
        }
    }
    let linear = is_linear_compat(steps);
    let mut out: Vec<Vec<usize>> = Vec::with_capacity(steps.len());
    for (i, s) in steps.iter().enumerate() {
        if linear {
            out.push(if i == 0 { Vec::new() } else { vec![i - 1] });
            continue;
        }
        let mut parents: Vec<usize> = Vec::with_capacity(s.inputs.len());
        for parent_id in &s.inputs {
            if parent_id.trim().is_empty() {
                return Err(format!("步骤「{}」的上游列表含空 id", s.name));
            }
            let Some(&pi) = index_of.get(parent_id.trim()) else {
                return Err(format!(
                    "步骤「{}」引用了不存在或未启用的上游:{}",
                    s.name, parent_id
                ));
            };
            if pi == i {
                return Err(format!("步骤「{}」不能把自身作为上游", s.name));
            }
            if !parents.contains(&pi) {
                parents.push(pi);
            }
        }
        parents.sort_unstable();
        out.push(parents);
    }
    Ok(out)
}

/// 成果节点选拔(D4):显式 `is_output=true` 优先;未标注时取**无后继汇点**。
/// 候选集内取下标最大的「direct + generates=true」步骤;候选内无生成步时回退
/// 全流程最后一个生成步(存量线性流程:汇点即末步,语义与旧版一致);都没有 → None。
///
/// 注意:本函数只回答「谁是成果节点」;该节点产出为空时的兜底见 custom.rs
/// (按数组下标降序取上一个非空生成产出,保证结果与执行顺序无关)。
pub fn output_index(steps: &[PlanStep], inputs: &[Vec<usize>]) -> Option<usize> {
    let is_generating = |i: usize| steps[i].action == "direct" && steps[i].generates == Some(true);
    let explicit: Vec<usize> = (0..steps.len())
        .filter(|&i| steps[i].is_output == Some(true))
        .collect();
    let candidates: Vec<usize> = if explicit.is_empty() {
        let mut has_successor = vec![false; steps.len()];
        for parents in inputs {
            for &p in parents {
                if p < has_successor.len() {
                    has_successor[p] = true;
                }
            }
        }
        (0..steps.len()).filter(|&i| !has_successor[i]).collect()
    } else {
        explicit
    };
    candidates
        .iter()
        .rev()
        .copied()
        .find(|&i| is_generating(i))
        .or_else(|| (0..steps.len()).rev().find(|&i| is_generating(i)))
}

/// 流程图解析:有效输入集 + 执行拓扑序(校验与执行共用的一步到位入口)。
pub fn resolve_graph(steps: &[PlanStep]) -> Result<FlowGraph, String> {
    let inputs = effective_inputs(steps)?;
    let order = kahn_order(&inputs, steps)?;
    Ok(FlowGraph { inputs, order })
}

/// Kahn 拓扑排序:就绪队列用 BTreeSet 维护,恒按下标升序出队——线性兼容流程的
/// 拓扑序因此等于数组顺序(执行顺序与旧版完全一致),同一流程两次运行结果可复现。
fn kahn_order(inputs: &[Vec<usize>], steps: &[PlanStep]) -> Result<Vec<usize>, String> {
    let n = inputs.len();
    let mut indegree = vec![0usize; n];
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, parents) in inputs.iter().enumerate() {
        for &p in parents {
            if p >= n || p == i {
                continue; // 越界/自环已由 effective_inputs 拦截,此处仅防御
            }
            children[p].push(i);
            indegree[i] += 1;
        }
    }
    let mut ready: BTreeSet<usize> = (0..n).filter(|&i| indegree[i] == 0).collect();
    let mut order = Vec::with_capacity(n);
    while let Some(&i) = ready.iter().next() {
        ready.remove(&i);
        order.push(i);
        for &child in &children[i] {
            indegree[child] -= 1;
            if indegree[child] == 0 {
                ready.insert(child);
            }
        }
    }
    if order.len() != n {
        let stuck: Vec<String> = (0..n)
            .filter(|&i| indegree[i] > 0)
            .map(|i| format!("「{}」", steps[i].name))
            .collect();
        return Err(format!(
            "自定义流程存在环,无法确定执行顺序:{}",
            stuck.join("、")
        ));
    }
    Ok(order)
}

/// 校验自定义流程(失败返回中文错误,PUT 时 400 给用户):
///  - 启用的步骤非空
///  - 至少一个「生成正文」的 direct 步骤(反思回退/输出依赖它)
///  - action 仅 direct / reflect;reflect 步骤不得携带 generates=Some(true)
///  - 温度 0.0-2.0、输出上限 1-32768(仅校验显式提供的值)
///  - 二维依赖边:步骤 id 唯一、上游必须存在且已启用、无自环、无环(见 `resolve_graph`)
pub fn validate_flow(
    cfg: &AgentFlowConfig,
    registered_tools: &BTreeSet<String>,
) -> Result<(), String> {
    // 未启用(开关关闭):允许保存任意状态(用户可先编辑、后启用);启用时才校验
    if !cfg.enabled {
        return Ok(());
    }
    let active: Vec<PlanStep> = cfg.steps.iter().filter(|s| s.enabled).cloned().collect();
    if active.is_empty() {
        return Err("自定义流程为空:请启用至少一个步骤".into());
    }
    if !active
        .iter()
        .any(|s| s.action == "direct" && s.generates == Some(true))
    {
        return Err("自定义流程缺少生成步骤:至少需要一个「生成正文」的 direct 步骤".into());
    }
    // 二维依赖边校验(线性兼容流程恒通过:隐式串联无悬空/无环风险)
    resolve_graph(&active)?;
    // 并行上限(二维批次 2):越界直接拒绝,避免执行期静默夹取
    if let Some(n) = cfg.max_parallel_nodes {
        if !(1..=MAX_PARALLEL_NODES_LIMIT).contains(&n) {
            return Err(format!(
                "并行节点上限需在 1-{} 之间(当前 {})",
                MAX_PARALLEL_NODES_LIMIT, n
            ));
        }
    }
    for s in &active {
        if s.action != "direct" && s.action != "reflect" {
            return Err(format!(
                "步骤「{}」动作非法(仅支持 direct / reflect)",
                s.name
            ));
        }
        if s.action == "reflect" && s.generates == Some(true) {
            return Err(format!("步骤「{}」为反思步骤,不应开启生成正文", s.name));
        }
        if s.action == "reflect" && s.system_prompt.is_some() {
            return Err(format!("步骤「{}」为反思步骤,不支持系统提示词", s.name));
        }
        // 节点档位:strict(单次调用)属二维批次 6,当前直接拒绝——不允许静默无效
        if let Some(kind) = s.kind.as_deref() {
            if kind != "loose" {
                return Err(format!(
                    "步骤「{}」的节点档位「{}」尚未实现(严格节点将在二维批次 6 落地)",
                    s.name, kind
                ));
            }
        }
        if let Some(t) = s.temperature {
            if !(0.0..=2.0).contains(&t) {
                return Err(format!("步骤「{}」温度需在 0.0-2.0 之间", s.name));
            }
        }
        if let Some(m) = s.max_tokens {
            if !(1..=32768).contains(&m) {
                return Err(format!("步骤「{}」输出上限需在 1-32768 之间", s.name));
            }
        }
        if s.goal.trim().is_empty() {
            return Err(format!("步骤「{}」缺少目标说明", s.name));
        }
        if let Some(names) = &s.tools {
            for name in names {
                if name.trim().is_empty() || !registered_tools.contains(name) {
                    return Err(format!("步骤「{}」引用了未注册工具: {}", s.name, name));
                }
            }
        }
        let choice = s.tool_choice.as_deref().unwrap_or("auto");
        if !matches!(choice, "auto" | "none" | "required" | "function") {
            return Err(format!("步骤「{}」tool_choice 非法", s.name));
        }
        if choice == "required" && s.tools.is_none() {
            return Err(format!(
                "步骤「{}」tool_choice=required 时必须配置有效工具",
                s.name
            ));
        }
        if choice == "function" {
            let function = s
                .tool_choice_function
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .ok_or_else(|| format!("步骤「{}」tool_choice=function 时必须指定工具", s.name))?;
            let effective = match &s.tools {
                None => false,
                Some(names) if names.is_empty() => registered_tools.contains(function),
                Some(names) => names.iter().any(|name| name == function),
            };
            if !effective {
                return Err(format!(
                    "步骤「{}」指定的 function 工具不在有效工具内: {}",
                    s.name, function
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::test_support::TempDataDir;

    fn tools() -> BTreeSet<String> {
        ["read", "search", "update_variables", "calculator"]
            .into_iter()
            .map(str::to_string)
            .collect()
    }

    fn step(name: &str, action: &str, generates: Option<bool>) -> PlanStep {
        PlanStep {
            id: format!("s-{name}"),
            name: name.into(),
            enabled: true,
            goal: format!("{name} 目标"),
            action: action.into(),
            generates,
            ..Default::default()
        }
    }

    fn flow(name: &str, steps: Vec<PlanStep>) -> AgentFlowConfig {
        AgentFlowConfig {
            id: format!("flow-{name}"),
            name: name.into(),
            description: None,
            enabled: true,
            steps,
            max_parallel_nodes: None,
        }
    }

    #[test]
    fn builtin_flow_step_tools_and_mvu_wording() {
        let f = builtin_flow();
        // 内置流程:起草(先计划后正文)→ 反思 → 修订(理解意图步已废弃)
        assert_eq!(f.steps.len(), 3);
        // 起草步:配 update_variables,提示词含「先写计划再输出正文」且点名工具
        let draft = &f.steps[0];
        assert_eq!(
            draft.tools.as_deref(),
            Some(&["update_variables".to_string()][..]),
            "起草步应实际下发 update_variables"
        );
        let dsp = draft.system_prompt.as_deref().unwrap_or("");
        assert!(
            dsp.contains("update_variables"),
            "起草步应点名 update_variables 工具: {dsp}"
        );
        assert!(
            dsp.contains("回退") && dsp.contains("<UpdateVariable>"),
            "起草步应说明文本协议是回退: {dsp}"
        );
        assert!(
            dsp.contains("200 字") && dsp.contains("计划") && dsp.contains("正文"),
            "起草步应先写 ≤200 字计划再输出正文: {dsp}"
        );
        // 反思步:无工具、无 system_prompt
        assert!(f.steps[1].tools.is_none());
        assert!(f.steps[1].system_prompt.is_none());
        // 修订步:update_variables + 回退协议
        let revise = &f.steps[2];
        assert_eq!(
            revise.tools.as_deref(),
            Some(&["update_variables".to_string()][..])
        );
        let rsp = revise.system_prompt.as_deref().unwrap_or("");
        assert!(
            rsp.contains("update_variables") && rsp.contains("回退"),
            "修订步应点名 update_variables 与回退协议: {rsp}"
        );
    }

    #[test]
    fn valid_flow_passes() {
        let cfg = flow(
            "ok",
            vec![
                step("理解", "direct", Some(false)),
                step("生成", "direct", Some(true)),
                step("反思", "reflect", None),
            ],
        );
        assert!(validate_flow(&cfg, &tools()).is_ok());
    }

    #[test]
    fn empty_flow_rejected() {
        // 未启用(开关关闭):允许保存空流程(用户可先编辑后启用)
        assert!(validate_flow(&AgentFlowConfig::default(), &tools()).is_ok());
        // 启用但无步骤 → 拒绝
        let mut cfg = flow("x", Vec::new());
        cfg.enabled = true;
        assert!(validate_flow(&cfg, &tools()).is_err());
        // 启用但全部步骤 disabled → 拒绝
        let mut cfg = flow("x", vec![step("禁用", "direct", Some(true))]);
        cfg.steps[0].enabled = false;
        assert!(validate_flow(&cfg, &tools()).is_err());
    }

    #[test]
    fn no_generating_step_rejected() {
        let cfg = flow(
            "x",
            vec![
                step("理解", "direct", Some(false)),
                step("反思", "reflect", None),
            ],
        );
        assert!(validate_flow(&cfg, &tools()).is_err());
    }

    #[test]
    fn unregistered_whitelist_tool_is_rejected() {
        let mut generation = step("生成", "direct", Some(true));
        generation.tools = Some(vec!["missing_tool".into()]);
        let error = validate_flow(&flow("x", vec![generation]), &tools()).unwrap_err();
        assert!(error.contains("未注册工具"), "实际错误: {error}");
    }

    #[test]
    fn function_choice_must_reference_effective_tool() {
        let mut generation = step("生成", "direct", Some(true));
        generation.tools = Some(vec!["read".into()]);
        generation.tool_choice = Some("function".into());
        generation.tool_choice_function = Some("calculator".into());
        let error = validate_flow(&flow("x", vec![generation]), &tools()).unwrap_err();
        assert!(error.contains("不在有效工具内"), "实际错误: {error}");
    }

    #[test]
    fn bad_action_rejected() {
        let cfg = flow("x", vec![step("怪步", "teleport", Some(true))]);
        assert!(validate_flow(&cfg, &tools()).is_err());
    }

    #[test]
    fn reflect_with_prompt_rejected() {
        let cfg = flow(
            "x",
            vec![
                step("生成", "direct", Some(true)),
                PlanStep {
                    system_prompt: Some("不应支持".into()),
                    ..step("反思", "reflect", None)
                },
            ],
        );
        assert!(validate_flow(&cfg, &tools()).is_err());
    }

    #[test]
    fn load_corrupted_file_falls_back_to_default() {
        let dir = TempDataDir::new("flow-test");
        std::fs::write(dir.join("agent_flows.json"), "{not json").unwrap();
        let lib = AgentFlowLibrary::load(&dir);
        assert!(lib.flows.is_empty());
    }

    #[test]
    fn load_missing_file_creates_builtin() {
        let dir = TempDataDir::new("flow-missing");
        let lib = AgentFlowLibrary::load(&dir);
        assert_eq!(lib.flows.len(), 1);
        assert_eq!(lib.flows[0].id, "builtin-coordination");
        assert_eq!(lib.current_flow_id.as_deref(), Some("builtin-coordination"));
        assert!(validate_flow(&lib.flows[0], &tools()).is_ok());
    }

    #[test]
    fn load_legacy_single_flow_migrates() {
        let dir = TempDataDir::new("flow-legacy");
        std::fs::write(
            dir.join("agent_flows.json"),
            r#"{"enabled":true,"steps":[{"id":"s1","name":"生成","enabled":true,"goal":"生成正文","action":"direct","generates":true}]}"#,
        )
        .unwrap();
        let lib = AgentFlowLibrary::load(&dir);
        assert_eq!(lib.flows.len(), 1);
        assert_eq!(lib.flows[0].steps.len(), 1);
        assert_eq!(lib.flows[0].name, "默认流程");
        assert_eq!(
            lib.current_flow_id.as_deref(),
            Some(lib.flows[0].id.as_str())
        );
    }

    #[test]
    fn service_set_select_remove_roundtrip() {
        let dir = TempDataDir::new("flow-svc");
        let mut svc =
            AgentFlowService::new(dir.path().to_path_buf(), tools().into_iter().collect());
        // 内置默认流程已在首次加载时注入
        assert_eq!(svc.get_library().flows.len(), 1);

        // 新建流程(set 空 id → 自动分配)
        let cfg = flow("my", vec![step("生成", "direct", Some(true))]);
        let mut new_cfg = cfg.clone();
        new_cfg.id = String::new();
        svc.set(new_cfg).unwrap();
        let lib = svc.get_library();
        assert_eq!(lib.flows.len(), 2);
        let mine = lib.flows.iter().find(|f| f.name == "my").unwrap();
        assert!(!mine.id.is_empty());
        assert_eq!(lib.current_flow_id.as_deref(), Some(mine.id.as_str()));

        // 选择内置流程
        svc.select("builtin-coordination").unwrap();
        assert_eq!(svc.get().map(|f| f.name.as_str()), Some("文学创作协调流程"));

        // 删除当前流程 → 回退到第一个
        svc.remove("builtin-coordination").unwrap();
        assert_eq!(svc.get_library().flows.len(), 1);
        assert_eq!(
            svc.get_library().current_flow_id.as_deref(),
            Some(svc.get_library().flows[0].id.as_str())
        );

        // 重新加载(持久化验证)
        let svc2 = AgentFlowService::new(dir.path().to_path_buf(), tools().into_iter().collect());
        assert_eq!(svc2.get_library().flows.len(), 1);
    }

    // ===== 二维流程原语(二维批次 1) =====

    /// 生成步骤(可带上游 id):二维测试的图节点
    fn graph_step(id: &str, inputs: &[&str]) -> PlanStep {
        PlanStep {
            id: id.into(),
            name: format!("节点{id}"),
            enabled: true,
            goal: format!("{id} 目标"),
            action: "direct".into(),
            generates: Some(true),
            inputs: inputs.iter().map(|s| (*s).to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn linear_flow_chains_previous_step_as_input() {
        // 存量一维流程:inputs 全空 → 线性兼容,输入即前一步(首步无输入)
        let steps = vec![
            step("起草", "direct", Some(true)),
            step("修订", "direct", Some(true)),
        ];
        assert!(is_linear_compat(&steps));
        let graph = resolve_graph(&steps).unwrap();
        assert_eq!(graph.inputs, vec![Vec::<usize>::new(), vec![0]]);
        // 拓扑序 == 数组顺序(就绪队列按下标升序),执行顺序与旧版一致
        assert_eq!(graph.order, vec![0, 1]);
    }

    #[test]
    fn two_dimensional_flow_resolves_inputs_and_topo_order() {
        // 菱形:a、b 为源节点,c 合并两路,d 收口
        let steps = vec![
            graph_step("a", &[]),
            graph_step("b", &[]),
            graph_step("c", &["a", "b"]),
            graph_step("d", &["c"]),
        ];
        assert!(!is_linear_compat(&steps));
        let graph = resolve_graph(&steps).unwrap();
        assert_eq!(graph.inputs[2], vec![0, 1], "多父按数组下标升序");
        assert_eq!(graph.inputs[3], vec![2]);
        assert_eq!(graph.order, vec![0, 1, 2, 3]);
    }

    #[test]
    fn multi_parent_order_is_by_index_and_deduped() {
        // 声明顺序 b→a 也不影响合并顺序(D3:按下标升序);同一父写两次只算一次
        let steps = vec![
            graph_step("a", &[]),
            graph_step("b", &[]),
            graph_step("c", &["b", "a", "a"]),
        ];
        let graph = resolve_graph(&steps).unwrap();
        assert_eq!(graph.inputs[2], vec![0, 1]);
    }

    #[test]
    fn graph_edges_rejected_on_dangling_duplicate_and_self_loop() {
        let dangling = vec![graph_step("a", &[]), graph_step("b", &["zzz"])];
        let err = resolve_graph(&dangling).unwrap_err();
        assert!(err.contains("不存在或未启用"), "实际错误:{err}");

        let duplicate = vec![graph_step("a", &[]), graph_step("a", &[])];
        let err = resolve_graph(&duplicate).unwrap_err();
        assert!(err.contains("重复"), "实际错误:{err}");

        let self_loop = vec![graph_step("a", &["a"])];
        let err = resolve_graph(&self_loop).unwrap_err();
        assert!(err.contains("自身"), "实际错误:{err}");

        // 缺 id:二维模式按 id 连接,空 id 无法解析
        let mut no_id = graph_step("a", &[]);
        no_id.id = String::new();
        let err = resolve_graph(&[no_id, graph_step("b", &["a"])]).unwrap_err();
        assert!(err.contains("缺少 id"), "实际错误:{err}");
    }

    #[test]
    fn cycle_rejected_and_nodes_named() {
        let steps = vec![
            graph_step("a", &["c"]),
            graph_step("b", &["a"]),
            graph_step("c", &["b"]),
        ];
        let err = resolve_graph(&steps).unwrap_err();
        assert!(err.contains("环"), "实际错误:{err}");
        assert!(
            err.contains("节点a") && err.contains("节点c"),
            "应点名环上节点:{err}"
        );
    }

    #[test]
    fn output_node_prefers_explicit_then_sink() {
        // 显式标注优先于汇点:c 是唯一汇点,但 b 被标注为成果节点
        let steps = vec![
            graph_step("a", &[]),
            PlanStep {
                is_output: Some(true),
                ..graph_step("b", &[])
            },
            graph_step("c", &["b"]),
        ];
        let graph = resolve_graph(&steps).unwrap();
        assert_eq!(output_index(&steps, &graph.inputs), Some(1));

        // 未标注:取无后继汇点
        let sink = vec![graph_step("a", &[]), graph_step("b", &["a"])];
        let graph = resolve_graph(&sink).unwrap();
        assert_eq!(output_index(&sink, &graph.inputs), Some(1));
    }

    #[test]
    fn output_node_skips_non_generating_sink_then_falls_back() {
        // 汇点是反思步(不生成)→ 回退最后一个生成步(存量线性流程 [生成, 反思] 语义)
        let reflect_tail = vec![
            step("生成", "direct", Some(true)),
            step("反思", "reflect", None),
        ];
        let graph = resolve_graph(&reflect_tail).unwrap();
        assert_eq!(output_index(&reflect_tail, &graph.inputs), Some(0));

        // 全流程无生成步 → None(调用方按「未产出任何成果」报错)
        let no_gen = vec![
            step("反思一", "reflect", None),
            step("反思二", "reflect", None),
        ];
        let graph = resolve_graph(&no_gen).unwrap();
        assert_eq!(output_index(&no_gen, &graph.inputs), None);
    }

    #[test]
    fn validate_flow_rejects_broken_graph_and_strict_kind() {
        // 环 → 400 文案点名节点
        let cyclic = flow(
            "cyclic",
            vec![graph_step("a", &["b"]), graph_step("b", &["a"])],
        );
        let err = validate_flow(&cyclic, &tools()).unwrap_err();
        assert!(err.contains("环"), "实际错误:{err}");

        // 悬空上游 → 拒绝
        let dangling = flow("dangling", vec![graph_step("a", &["missing"])]);
        assert!(validate_flow(&dangling, &tools()).is_err());

        // 节点档位:strict 尚未实现 → 明确报错而非静默无效;loose 放行
        let strict = flow(
            "strict",
            vec![PlanStep {
                kind: Some("strict".into()),
                ..graph_step("a", &[])
            }],
        );
        let err = validate_flow(&strict, &tools()).unwrap_err();
        assert!(err.contains("档位"), "实际错误:{err}");
        let loose = flow(
            "loose",
            vec![PlanStep {
                kind: Some("loose".into()),
                ..graph_step("a", &[])
            }],
        );
        assert!(validate_flow(&loose, &tools()).is_ok());
    }

    #[test]
    fn parallel_limit_defaults_clamped_and_validated() {
        let mut cfg = flow("parallel", vec![graph_step("a", &[])]);
        // 缺省 2(保守:并行会成倍消耗 token,WF-11)
        assert_eq!(effective_max_parallel(&cfg), 2);
        // 显式值生效
        cfg.max_parallel_nodes = Some(4);
        assert_eq!(effective_max_parallel(&cfg), 4);
        // 执行期防护:越界取边界(1 = 完全串行)
        cfg.max_parallel_nodes = Some(0);
        assert_eq!(effective_max_parallel(&cfg), 1);
        cfg.max_parallel_nodes = Some(99);
        assert_eq!(
            effective_max_parallel(&cfg),
            MAX_PARALLEL_NODES_LIMIT as usize
        );
        // 保存期校验:越界直接 400,不静默夹取
        assert!(validate_flow(&cfg, &tools()).is_err());
        cfg.max_parallel_nodes = Some(0);
        assert!(validate_flow(&cfg, &tools()).is_err());
        cfg.max_parallel_nodes = Some(MAX_PARALLEL_NODES_LIMIT);
        assert!(validate_flow(&cfg, &tools()).is_ok());
    }

    #[test]
    fn topo_order_is_min_index_ready_expansion() {
        // 拓扑序 = 反复取「当前就绪节点中下标最小者」的展开
        //(并行调度器的就绪队列按下标升序出队,用的正是同一条规则)
        let steps = vec![
            graph_step("丙", &["乙"]), // 下标 0:依赖下标 1
            graph_step("乙", &["甲"]), // 下标 1:依赖下标 2
            graph_step("甲", &[]),     // 下标 2:源节点
        ];
        let graph = resolve_graph(&steps).unwrap();
        assert_eq!(
            graph.order,
            vec![2, 1, 0],
            "应从下标最小的就绪节点开始,而不是数组声明顺序"
        );
        assert_eq!(graph.inputs[0], vec![1]);
        assert!(graph.inputs[2].is_empty());
    }
}
