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

    /// 按 id 取流程(二维批次 6b:静态子图的引用解析)。
    pub fn flow_by_id(&self, id: &str) -> Option<&AgentFlowConfig> {
        self.library.flows.iter().find(|f| f.id == id)
    }

    pub fn get_library(&self) -> &AgentFlowLibrary {
        &self.library
    }

    /// 使用启动时已注册工具集合校验流程,供执行前复用。
    ///
    /// 校验分两层(二维批次 6b 审查修正):**结构**走 `validate_flow`,**引用链**只从
    /// `config` 自己起走一条。执行期**不再全库扫描**——库里任一**无关**流程的脏引用
    /// (手改 `data/agent_flows.json`、外部导入的流程)都不该拦死本次执行,全库口径是
    /// **保存期**的职责(见 [`validate_sub_flows`] 与 `set`)。环与深度另由运行期调用链
    /// 守卫兜底(见 `task_engine/custom.rs::run_sub_flow`)。
    pub fn validate(&self, config: &AgentFlowConfig) -> Result<(), String> {
        validate_flow(config, &self.registered_tools)?;
        walk_sub_flows_from(&flow_index(&self.library), config, &self.registered_tools)
    }

    /// 校验一份**冻结快照**自身(二维批次 5a:绑定任务的执行前校验)。
    /// 收在服务内,调用方无须知道注册工具集从哪来;规则见 [`validate_snapshot`]。
    pub fn validate_task_snapshot(&self, snapshot: &FlowSnapshot) -> Result<(), String> {
        validate_snapshot(snapshot, &self.registered_tools)
    }

    /// 当前流程 → 任务用快照(二维批次 5a:未绑定任务的执行前解析口径)。
    ///
    /// 与 [`snapshot_for`] 的差别只在「取哪一份流程」:本方法是「跟随当前流程」分支,
    /// 故保留 current_flow 的既有语义(**要求 enabled**),校验与闭包收集同一套。
    pub fn current_snapshot(&self) -> Result<FlowSnapshot, String> {
        let cfg = self.get().ok_or("请先在设置中启用一个 Agent 流程")?;
        if !cfg.enabled {
            return Err("当前 Agent 流程未启用,请在设置中开启后再运行 custom 模式".into());
        }
        self.build_snapshot(cfg)
    }

    /// 绑定流程 id → 任务用快照(二维批次 5a:任务创建期与绑定任务的执行前解析)。
    ///
    /// 与「当前流程」同一取用纪律:要求该流程存在且**已启用**(未启用即拒绝绑定,
    /// 避免任务建好一跑就 error),校验口径与 [`validate`] 一致(结构 + 从入口起可达的
    /// 引用链,不看库里无关流程的脏数据)。
    pub fn snapshot_for(&self, flow_id: &str) -> Result<FlowSnapshot, String> {
        let cfg = self
            .flow_by_id(flow_id)
            .ok_or_else(|| format!("所选流程不存在:{flow_id}"))?;
        if !cfg.enabled {
            return Err(format!(
                "所选流程「{}」未启用,请先在设置中开启后再绑定",
                flow_label(cfg)
            ));
        }
        self.build_snapshot(cfg)
    }

    /// 校验一份流程 + 收集它的子流程闭包(两条入口共用的实现)。
    fn build_snapshot(&self, root: &AgentFlowConfig) -> Result<FlowSnapshot, String> {
        self.validate(root)?;
        Ok(self.sub_flow_closure(root))
    }

    /// 入口流程 + **可达子流程闭包**(入口恒为首个;其余按「发现顺序」= 逐层按引用声明顺序)。
    ///
    /// 复用 `sub_flow_edges` 这一条「什么算被引用」的判定(与保存期校验、删除保护同源),
    /// 因此闭包与校验对「可达」的理解不可能分叉;环/深度由调用方的 [`validate`] 挡住,
    /// 这里只做收集(`seen` 去重,不会死循环;悬空引用也在 validate 处拦下)。
    fn sub_flow_closure(&self, root: &AgentFlowConfig) -> FlowSnapshot {
        let by_id = flow_index(&self.library);
        let mut flows: Vec<AgentFlowConfig> = vec![root.clone()];
        let mut seen: BTreeSet<String> = BTreeSet::from([root.id.clone()]);
        let mut cursor = 0usize;
        while cursor < flows.len() {
            let sub_ids = sub_flow_ids(&flows[cursor]);
            cursor += 1;
            for sub_id in sub_ids {
                if !seen.insert(sub_id.clone()) {
                    continue;
                }
                if let Some(cfg) = by_id.get(sub_id.as_str()) {
                    flows.push((*cfg).clone());
                }
            }
        }
        FlowSnapshot {
            root_id: root.id.clone(),
            flows,
        }
    }

    /// 聊天侧执行快照(二维批次 6b):当前流程的启用步骤 + 子流程展开。
    ///
    /// 单一出口的意义:`/api/chat/send` 与 `/api/agent/plan` 两个入口必须给出**同一份**
    /// 步骤序列,否则预览看到的 plan 与实际执行会不一致。子流程展开只能在这里做——
    /// 引擎拿到的是步骤快照(`AgentRunRequest.flow`),它没有流程库。
    pub fn chat_flow_steps(&self, cfg: &AgentFlowConfig) -> Result<Vec<PlanStep>, String> {
        let active: Vec<PlanStep> = cfg.steps.iter().filter(|s| s.enabled).cloned().collect();
        expand_sub_flows(&self.library, &cfg.id, &active)
    }

    /// 保存(创建或更新)流程并设为当前选中;id 为空时自动生成新 id(新建)。
    /// 校验失败返回 Err(不落盘)。子流程引用链校验在**候选库**上做:
    /// 本次编辑可能让引用它的其它流程变深/成环(见 `validate_sub_flows`)。
    ///
    /// **已知代价(全库口径)**:`validate_sub_flows` 遍历候选库里的每个流程,故库内任一
    /// 流程存在非法引用(悬空/自引用/环/深度/被引用流程结构不合法)都会拒绝**本次保存**
    /// ——哪怕本次编辑的流程与它无关。这是有意的:保存是唯一的写入点,只有在这里全量
    /// 把关,执行期才能只查入口可达链(见 `validate`)。
    pub fn set(&mut self, mut config: AgentFlowConfig) -> Result<(), String> {
        validate_flow(&config, &self.registered_tools)?;
        if config.id.trim().is_empty() {
            config.id = Uuid::new_v4().to_string();
        }
        if config.name.trim().is_empty() {
            config.name = "未命名流程".into();
        }
        let mut candidate = self.library.clone();
        match candidate.flows.iter_mut().find(|f| f.id == config.id) {
            Some(existing) => *existing = config.clone(),
            None => candidate.flows.push(config.clone()),
        }
        validate_sub_flows(&candidate, &self.registered_tools)?;
        self.library = candidate;
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
    ///
    /// 被其它流程当作子流程引用时**拒绝删除**(二维批次 6b):否则会留下悬空引用,
    /// 让引用方在执行期才失败。报错点名引用方,用户先解除引用再删。
    ///
    /// 引用口径与校验口径**一致:只看启用步骤**(`sub_flow_edges` 同款)——停用步骤里的
    /// 残留引用不参与执行链、也不参与保存期校验,拿它卡住删除只会让用户被一条
    /// 「不存在的约束」拦住。
    pub fn remove(&mut self, id: &str) -> Result<(), String> {
        let label = match self.flow_by_id(id) {
            Some(f) => flow_label(f),
            None => return Err(format!("流程不存在:{}", id)),
        };
        let referrers: Vec<String> = self
            .library
            .flows
            .iter()
            .filter(|f| {
                f.id != id
                    && f.steps
                        .iter()
                        .any(|s| s.enabled && s.sub_flow_ref() == Some(id))
            })
            .map(flow_label)
            .collect();
        if !referrers.is_empty() {
            return Err(format!(
                "流程「{}」正被子流程引用,无法删除:{}。请先在这些流程里解除引用",
                label,
                referrers.join("、")
            ));
        }
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

/// **任务用的流程快照**(二维批次 5a):入口流程 + 其**可达子流程闭包**,一次性冻结。
///
/// 为什么冻结**闭包**而不只记一个 id:运行期的子图解析原本按 id 回读流程库,于是
/// 「改一个被挂载的子流程」会悄悄改变已建任务的行为——跨流程生效、最难排查。
/// 冻结闭包后,任务跑过一次(或创建时绑定)就与流程库解耦:改库、换当前流程、
/// 甚至删掉那份流程,都不再影响它。
///
/// 落库形态:整段 JSON 存 `tasks.flow_snapshot` 列(**不进**任务列表查询的列清单
/// ——快照是 O(流程库) 体积);下发形态:任务详情顶层 `flow_snapshot` 字段。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowSnapshot {
    /// 入口流程 id(运行期解析的根;`flows` 中必有其一与之相等)
    pub root_id: String,
    /// 入口流程 + 可达子流程闭包(入口恒为首个;其余按发现顺序)
    pub flows: Vec<AgentFlowConfig>,
}

impl FlowSnapshot {
    /// 入口流程(快照自洽时必然存在;缺失即数据损坏,调用方按错误处理)
    pub fn root(&self) -> Option<&AgentFlowConfig> {
        self.flow_by_id(&self.root_id)
    }

    /// 在**闭包内**按 id 取流程(运行期子图解析用:快照外的一律取不到)
    pub fn flow_by_id(&self, id: &str) -> Option<&AgentFlowConfig> {
        self.flows.iter().find(|f| f.id == id)
    }

    /// 把快照当成一份「独立流程库」:校验复用它,从而不与流程库的实时状态耦合
    pub fn as_library(&self) -> AgentFlowLibrary {
        AgentFlowLibrary {
            current_flow_id: Some(self.root_id.clone()),
            flows: self.flows.clone(),
        }
    }
}

/// 校验**冻结快照自身**(绑定任务的执行前校验):结构 + 闭包内的引用链。
///
/// 与 [`AgentFlowService::validate`] 的关键差别是**只看快照、不看流程库**——被冻结的
/// 任务不该因为库里子流程被改/被删而失效(那正是二维批次 5a 要收掉的行为漂移)。
/// 校验规则本身仍是同一套(`validate_flow` + `validate_sub_flows`),不复制判定。
pub fn validate_snapshot(
    snapshot: &FlowSnapshot,
    registered_tools: &BTreeSet<String>,
) -> Result<(), String> {
    let root = snapshot
        .root()
        .ok_or_else(|| format!("流程快照缺少入口流程:{}", snapshot.root_id))?;
    validate_flow(root, registered_tools)?;
    validate_sub_flows(&snapshot.as_library(), registered_tools)
}

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
///  - 反思步骤不得挂载静态子流程(反思不产出正文,挂子图无意义且会绕过反思语义)
///
/// **只管单个流程自身**:跨流程的子流程引用链由 `validate_sub_flows` 负责(它需要库)。
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
        // 反思步骤不产出正文,挂子图会绕过「反思判定 + 失败回退」语义 → 保存期直接拒绝
        if s.action == "reflect" && s.is_sub_flow() {
            return Err(format!("步骤「{}」为反思步骤,不支持挂载子流程", s.name));
        }
        // 节点档位(二维批次 6a):缺省/loose = 工具自循环;strict = 单次调用、不下发工具。
        // 只拒绝未知取值——允许的取值都有已实现的语义,不存在「静默无效」(裁定 15 差异 2 口径)
        if let Some(kind) = s.kind.as_deref() {
            if kind != "loose" && kind != "strict" {
                return Err(format!(
                    "步骤「{}」的节点档位「{}」非法(仅支持 loose / strict)",
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

// ==================== 静态子图原语(二维批次 6b) ====================
//
// **单一出处**:保存期校验(`validate_sub_flows`)、任务侧执行器
// (`task_engine/custom.rs::run_sub_flow`)、聊天侧线性化(`expand_sub_flows`)三处共用
// 下面这些原语。与二维批次 1 的图原语同一纪律:不得在任一侧复制「什么算合法子流程引用」
// 的判断(否则编辑器能保存的引用与能执行的引用会漂移)。
//
// 形态决策(见 `docs/计划.md` 裁定 18):子图取**引用流程 id**形态,不取内联嵌套。
// 理由:内联嵌套需要一整套嵌套编辑 UI(列表 + 画布双层),成本远超本批规模;而引用形态
// 与批次 7 的「子执行 + 深度守卫 + 记账」原语同构,且静态引用的环/深度都能在保存期完全
// 判定(内联嵌套没有这个问题,但也没有跨流程复用的价值)。

/// 子流程嵌套深度上限:入口流程算第 0 层,最多再嵌 3 层(整条链最多 4 个流程)。
///
/// 深度是**成本闸门**:每层子流程把该节点的调用次数乘上子图节点数(与 WF-11 的并行
/// 加倍叠加)。静态子图本身无环(保存期已拒),深度仍必须封顶——子图节点多、层数深时
/// 单节点的 token 消耗会以乘积增长。
pub const MAX_SUB_FLOW_DEPTH: usize = 3;

/// 流程展示名(命名为空时回退 id;错误文案与 plan 前缀都用它)
pub fn flow_label(cfg: &AgentFlowConfig) -> String {
    if cfg.name.trim().is_empty() {
        cfg.id.clone()
    } else {
        cfg.name.trim().to_string()
    }
}

/// 流程中**启用步骤**挂载的子流程引用:(步骤展示名, 子流程 id)。
/// 停用步骤不参与执行,故不参与引用链校验(与 `validate_flow` 只看启用步骤同口径)。
fn sub_flow_edges(cfg: &AgentFlowConfig) -> Vec<(String, String)> {
    cfg.steps
        .iter()
        .filter(|s| s.enabled)
        .filter_map(|s| s.sub_flow_ref().map(|id| (s.name.clone(), id.to_string())))
        .collect()
}

/// 同上,只要 id(快照闭包收集用;与 `sub_flow_edges` 同一条「什么算被引用」的判定)
fn sub_flow_ids(cfg: &AgentFlowConfig) -> Vec<String> {
    sub_flow_edges(cfg).into_iter().map(|(_, id)| id).collect()
}

/// 子流程引用链校验(二维批次 6b,保存期 400):
///  - 被引用的流程必须在本库中存在(悬空引用);
///  - 不得引用流程自身;
///  - 调用链不得成环(A → B → A;**图内**环检测挡不住跨流程环);
///  - 嵌套深度 ≤ [`MAX_SUB_FLOW_DEPTH`];
///  - **被引用的**流程必须结构合法(启用步骤非空 / 有生成步 / 图无环 / 工具已注册),
///    即把它当作「启用态」复用 [`validate_flow`] 的同一套规则——被引用流程自身的
///    `enabled` 开关只决定它能否作为当前流程直接执行,不影响它能否被引用。
///
/// 遍历从**库内每个流程**出发,而不是只从本次编辑的流程:B 被编辑成引用 A 时,
/// 引用 B 的 C 也会跟着变深/成环,只查 B 会漏。库规模是「用户手写的几十个流程」,
/// 全量遍历的成本可忽略(保存是低频操作;执行期另有只查入口链的 [`walk_sub_flows_from`])。
pub fn validate_sub_flows(
    library: &AgentFlowLibrary,
    registered_tools: &BTreeSet<String>,
) -> Result<(), String> {
    let by_id = flow_index(library);
    for flow in &library.flows {
        walk_sub_flows_from(&by_id, flow, registered_tools)?;
    }
    Ok(())
}

/// 流程库的 id → 流程索引(引用解析共用;库内 id 唯一由 `set` 的写入路径保证)。
fn flow_index(library: &AgentFlowLibrary) -> BTreeMap<&str, &AgentFlowConfig> {
    library.flows.iter().map(|f| (f.id.as_str(), f)).collect()
}

/// 从**单个流程**起走一遍引用链(规则见 [`validate_sub_flows`] 的清单)。
/// `by_id` 只用于解析被引用方,链首流程自身无须在库内(set 的候选库场景反之亦然)。
fn walk_sub_flows_from(
    by_id: &BTreeMap<&str, &AgentFlowConfig>,
    flow: &AgentFlowConfig,
    registered_tools: &BTreeSet<String>,
) -> Result<(), String> {
    // 链首是该流程自身;确实没挂子流程的流程无活可干
    if sub_flow_edges(flow).is_empty() {
        return Ok(());
    }
    let mut chain: Vec<String> = vec![flow.id.clone()];
    walk_sub_flows(by_id, flow, &mut chain, registered_tools)
}

/// 引用链深度优先遍历(见 [`validate_sub_flows`] 的规则清单)。
/// `chain` 是自链首起的**流程 id 链(含当前流程)**,用于环检测与深度判定。
fn walk_sub_flows(
    by_id: &BTreeMap<&str, &AgentFlowConfig>,
    flow: &AgentFlowConfig,
    chain: &mut Vec<String>,
    registered_tools: &BTreeSet<String>,
) -> Result<(), String> {
    let label_of = |id: &str| -> String {
        by_id
            .get(id)
            .map(|f| flow_label(f))
            .unwrap_or_else(|| id.to_string())
    };
    for (step_label, sub_id) in sub_flow_edges(flow) {
        if sub_id == flow.id {
            return Err(format!(
                "流程「{}」的步骤「{}」不能引用流程自身",
                flow_label(flow),
                step_label
            ));
        }
        if let Some(pos) = chain.iter().position(|id| *id == sub_id) {
            let mut cycle: Vec<String> = chain[pos..].iter().map(|id| label_of(id)).collect();
            cycle.push(label_of(&sub_id));
            return Err(format!("子流程调用链存在环:{}", cycle.join(" → ")));
        }
        let Some(target) = by_id.get(sub_id.as_str()) else {
            return Err(format!(
                "流程「{}」的步骤「{}」引用的子流程不存在:{}",
                flow_label(flow),
                step_label,
                sub_id
            ));
        };
        if chain.len() > MAX_SUB_FLOW_DEPTH {
            let mut path: Vec<String> = chain.iter().map(|id| label_of(id)).collect();
            path.push(flow_label(target));
            return Err(format!(
                "子流程嵌套超过 {} 层:{}",
                MAX_SUB_FLOW_DEPTH,
                path.join(" → ")
            ));
        }
        // 被引用流程按「启用态」复用同一套结构规则(见函数文档)
        let mut as_enabled = (*target).clone();
        as_enabled.enabled = true;
        validate_flow(&as_enabled, registered_tools).map_err(|e| {
            format!(
                "流程「{}」引用的子流程「{}」无效:{}",
                flow_label(flow),
                flow_label(target),
                e
            )
        })?;
        chain.push(sub_id.clone());
        walk_sub_flows(by_id, target, chain, registered_tools)?;
        chain.pop();
    }
    Ok(())
}

/// 聊天侧线性化:把挂载了子流程的节点**就地展开**成被引用流程的启用步骤(递归),
/// 按各级各自的拓扑序拼成一张扁平列表。
///
/// 为什么是「展开」而不是「让聊天引擎跑嵌套」:聊天侧本来就**按拓扑序串行、步骤之间
/// 不传递产出**(见 `agents/planner.rs::make_custom_plan` 注释),在该位置依次跑子图节点
/// 与「先跑子图、产出交给下游」在聊天里等价;展开还能让 plan 面板逐节点显示进度,
/// 且无须给引擎引入流程库依赖。
///
/// 展开口径:
///  - 产出步骤的 `inputs` **一律清空**——扁平列表已按拓扑序排好,再保留 id 依赖会与
///    拼接后的图不一致(外层节点的 id 在展开后已不存在,留着就是悬空上游);
///  - 展开步骤的**名字**加 `【子流程「X」】` 前缀,让 plan 里能看出边界;
///  - 展开步骤的 **id 按挂载位置重写**(`前缀 + 原 id`,见 `expand_ordered`):同一子流程
///    被两处挂载、或父子流程撞 id(「复制流程」原样保留 step id)时,扁平列表会出现重复 id,
///    而聊天侧随后还要再跑一遍 `make_custom_plan` → `resolve_graph`,那里对重复 id 直接拒绝
///    (`effective_inputs`,文案「步骤 id 重复」)——于是**同一份流程任务侧能跑、聊天侧 400**。
///    顶层前缀为空串,故顶层步骤 id 逐字节不变;
///  - `entry_flow_id` 是本次展开的入口流程 id,用于自引用/环守卫(与保存期同一套判定,
///    此处是防御外部手改配置的兜底)。
pub fn expand_sub_flows(
    library: &AgentFlowLibrary,
    entry_flow_id: &str,
    steps: &[PlanStep],
) -> Result<Vec<PlanStep>, String> {
    let by_id = flow_index(library);
    let mut chain: Vec<String> = vec![entry_flow_id.to_string()];
    let out = expand_ordered(&by_id, steps, &mut chain, "")?;
    // 兜底自检:展平后 `inputs` 已清空(线性兼容),该调用只剩「id 非空且唯一」的判定意义。
    // 前缀派生已保证各挂载点不撞 id,这里防的是「用户把步骤 id 写成与派生前缀同形」这类
    // 极端配置:给出点名 id 的中文错误,而不是让下游报「步骤 id 重复」。
    if let Err(e) = resolve_graph(&out) {
        let dups = duplicated_step_ids(&out);
        if !dups.is_empty() {
            return Err(format!(
                "子流程展开后步骤 id 冲突:重复的步骤 id「{}」——请检查子流程的步骤 id 是否与其它流程重复(原始错误:{e})",
                dups.join("、")
            ));
        }
        return Err(e);
    }
    Ok(out)
}

/// 扁平列表里重复出现的步骤 id(升序;无重复返回空)
fn duplicated_step_ids(steps: &[PlanStep]) -> Vec<String> {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut dups: BTreeSet<String> = BTreeSet::new();
    for s in steps {
        if !seen.insert(s.id.as_str()) {
            dups.insert(s.id.clone());
        }
    }
    dups.into_iter().collect()
}

/// `expand_sub_flows` 的递归体:解析本层拓扑序 → 逐节点产出(子流程节点就地展开)。
///
/// `prefix` 是本层的 id 前缀:入口流程为空串;每进入一层子图,按**挂载位置**追加
/// `s{下标}/`(下标 = 挂载节点在本层数组中的下标)。「同一子流程挂两处」因此得到两个不同
/// 前缀,id 天然唯一。重写是安全的:`PlanStep.id` 在聊天引擎侧不被消费——它只服务于图原语
/// 自身的连接与校验(见 `effective_inputs`),展开后 `inputs` 已清空,id 不再参与执行。
fn expand_ordered(
    by_id: &BTreeMap<&str, &AgentFlowConfig>,
    steps: &[PlanStep],
    chain: &mut Vec<String>,
    prefix: &str,
) -> Result<Vec<PlanStep>, String> {
    let graph = resolve_graph(steps)?;
    let mut out: Vec<PlanStep> = Vec::with_capacity(steps.len());
    for &i in &graph.order {
        let step = &steps[i];
        let Some(sub_id) = step.sub_flow_ref() else {
            out.push(flattened_step(step, prefix));
            continue;
        };
        let label_of = |id: &str| -> String {
            by_id
                .get(id)
                .map(|f| flow_label(f))
                .unwrap_or_else(|| id.to_string())
        };
        let Some(target) = by_id.get(sub_id) else {
            return Err(format!(
                "步骤「{}」引用的子流程不存在:{}",
                step.name, sub_id
            ));
        };
        if let Some(pos) = chain.iter().position(|id| id == sub_id) {
            let mut cycle: Vec<String> = chain[pos..].iter().map(|id| label_of(id)).collect();
            cycle.push(label_of(sub_id));
            return Err(format!("子流程调用链存在环:{}", cycle.join(" → ")));
        }
        if chain.len() > MAX_SUB_FLOW_DEPTH {
            let mut path: Vec<String> = chain.iter().map(|id| label_of(id)).collect();
            path.push(flow_label(target));
            return Err(format!(
                "子流程嵌套超过 {} 层:{}",
                MAX_SUB_FLOW_DEPTH,
                path.join(" → ")
            ));
        }
        let active: Vec<PlanStep> = target.steps.iter().filter(|s| s.enabled).cloned().collect();
        if active.is_empty() {
            return Err(format!(
                "步骤「{}」引用的子流程「{}」没有启用的步骤",
                step.name,
                flow_label(target)
            ));
        }
        chain.push(sub_id.to_string());
        // 子层前缀按挂载位置派生:`s{本层下标}/`(下标取自 graph.order,即数组下标)
        let child = format!("{}s{}/", prefix, i);
        let nested = expand_ordered(by_id, &active, chain, &child)?;
        chain.pop();
        let label = flow_label(target);
        for mut s in nested {
            s.name = format!("【子流程「{}」】{}", label, s.name);
            out.push(s);
        }
    }
    Ok(out)
}

/// 展开产物的扁平化:清空 `inputs`(见 `expand_sub_flows` 的展开口径),并把 id 打上本层前缀。
/// 顶层前缀为空串 → 顶层步骤 id 与配置里的原值逐字节相同。
fn flattened_step(step: &PlanStep, prefix: &str) -> PlanStep {
    PlanStep {
        id: format!("{}{}", prefix, step.id),
        inputs: Vec::new(),
        ..step.clone()
    }
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
    fn validate_flow_rejects_broken_graph_and_unknown_kind() {
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

        // 节点档位(二维批次 6a):loose / strict 都有已实现的语义 → 放行
        for kind in ["loose", "strict"] {
            let ok = flow(
                "kind",
                vec![PlanStep {
                    kind: Some(kind.into()),
                    ..graph_step("a", &[])
                }],
            );
            assert!(validate_flow(&ok, &tools()).is_ok(), "kind={kind}");
        }

        // 未知档位 → 明确报错而非静默无效
        let unknown = flow(
            "unknown-kind",
            vec![PlanStep {
                kind: Some("medium".into()),
                ..graph_step("a", &[])
            }],
        );
        let err = validate_flow(&unknown, &tools()).unwrap_err();
        assert!(err.contains("档位"), "实际错误:{err}");
    }

    #[test]
    fn strict_step_wins_over_tools_config() {
        // 档位优先于 tools(二维批次 6a):严格节点配了工具也仍判为严格;
        // 「严格 + 工具」允许保存(工具配置保留以便切回宽松档,执行期不下发)
        let strict_with_tools = flow(
            "strict-tools",
            vec![PlanStep {
                kind: Some("strict".into()),
                tools: Some(vec!["read".into()]),
                ..graph_step("a", &[])
            }],
        );
        assert!(validate_flow(&strict_with_tools, &tools()).is_ok());
        assert!(strict_with_tools.steps[0].is_strict());

        // 缺省档位 = 宽松(存量流程语义不变)
        let mut plain = graph_step("a", &[]);
        assert!(!plain.is_strict());
        plain.kind = Some("loose".into());
        assert!(!plain.is_strict());
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

    // ===== 静态子图(二维批次 6b) =====

    /// 挂载子流程的节点:图结构同 `graph_step`,只多一个 sub_flow_id
    fn sub_step(id: &str, sub_id: &str) -> PlanStep {
        PlanStep {
            sub_flow_id: Some(sub_id.into()),
            ..graph_step(id, &[])
        }
    }

    fn library(flows: Vec<AgentFlowConfig>) -> AgentFlowLibrary {
        AgentFlowLibrary {
            current_flow_id: flows.first().map(|f| f.id.clone()),
            flows,
        }
    }

    /// 保存流程并返回分配到的 id(库内按名字回查)
    fn save_flow(svc: &mut AgentFlowService, name: &str, steps: Vec<PlanStep>) -> String {
        let mut cfg = flow(name, steps);
        cfg.id = String::new();
        svc.set(cfg).unwrap();
        svc.get_library()
            .flows
            .iter()
            .find(|f| f.name == name)
            .expect("刚保存的流程应在库内")
            .id
            .clone()
    }

    fn service(dir: &TempDataDir) -> AgentFlowService {
        AgentFlowService::new(dir.path().to_path_buf(), tools().into_iter().collect())
    }

    #[test]
    fn sub_flow_refs_reject_dangling_self_and_cycle() {
        // 悬空引用:本库没有该 id
        let lib = library(vec![flow("主", vec![sub_step("n1", "missing")])]);
        let err = validate_sub_flows(&lib, &tools()).unwrap_err();
        assert!(err.contains("不存在"), "实际错误:{err}");

        // 自引用:flow() 的 id 形如 flow-<名字>
        let lib = library(vec![flow("自", vec![sub_step("n1", "flow-自")])]);
        let err = validate_sub_flows(&lib, &tools()).unwrap_err();
        assert!(err.contains("自身"), "实际错误:{err}");

        // 跨流程环 A → B → A(图内环检测挡不住,必须按调用链拦)
        let lib = library(vec![
            flow("A", vec![sub_step("n1", "flow-B")]),
            flow("B", vec![sub_step("n1", "flow-A")]),
        ]);
        let err = validate_sub_flows(&lib, &tools()).unwrap_err();
        assert!(err.contains("环"), "实际错误:{err}");
        assert!(
            err.contains("A") && err.contains("B"),
            "应点名环上流程:{err}"
        );
    }

    #[test]
    fn sub_flow_depth_limit_counts_nesting_levels() {
        // 入口 0 层 + 3 层子流程(A→B→C→D)= 上限内
        let ok = library(vec![
            flow("A", vec![sub_step("n", "flow-B")]),
            flow("B", vec![sub_step("n", "flow-C")]),
            flow("C", vec![sub_step("n", "flow-D")]),
            flow("D", vec![graph_step("n", &[])]),
        ]);
        assert!(
            validate_sub_flows(&ok, &tools()).is_ok(),
            "3 层嵌套应放行(上限 {MAX_SUB_FLOW_DEPTH})"
        );

        // 再加一层 E → 第 4 层,越界
        let too_deep = library(vec![
            flow("A", vec![sub_step("n", "flow-B")]),
            flow("B", vec![sub_step("n", "flow-C")]),
            flow("C", vec![sub_step("n", "flow-D")]),
            flow("D", vec![sub_step("n", "flow-E")]),
            flow("E", vec![graph_step("n", &[])]),
        ]);
        let err = validate_sub_flows(&too_deep, &tools()).unwrap_err();
        assert!(err.contains("嵌套超过"), "实际错误:{err}");
        assert!(
            err.contains(&MAX_SUB_FLOW_DEPTH.to_string()),
            "应点名上限:{err}"
        );
    }

    #[test]
    fn referenced_flow_must_be_structurally_valid_regardless_of_enabled() {
        // 被引用流程没有启用的步骤 → 拒绝(结构规则复用 validate_flow)
        let mut empty = flow("空子", vec![graph_step("n", &[])]);
        empty.steps[0].enabled = false;
        let lib = library(vec![flow("主", vec![sub_step("n", "flow-空子")]), empty]);
        let err = validate_sub_flows(&lib, &tools()).unwrap_err();
        assert!(err.contains("引用的子流程"), "实际错误:{err}");

        // 被引用流程缺少生成步 → 同样拒绝
        let lib = library(vec![
            flow("主", vec![sub_step("n", "flow-反思子")]),
            flow("反思子", vec![step("反思", "reflect", None)]),
        ]);
        let err = validate_sub_flows(&lib, &tools()).unwrap_err();
        assert!(err.contains("缺少生成步骤"), "实际错误:{err}");

        // 被引用流程**未启用**:仍可作为子流程(
        // 「已启用」只决定它能否作为当前流程直接执行)
        let mut helper = flow("辅助", vec![graph_step("n", &[])]);
        helper.enabled = false;
        let lib = library(vec![flow("主", vec![sub_step("n", "flow-辅助")]), helper]);
        assert!(
            validate_sub_flows(&lib, &tools()).is_ok(),
            "未启用的流程仍可作为子流程被引用"
        );
    }

    #[test]
    fn reflect_step_cannot_mount_sub_flow() {
        let cfg = flow(
            "x",
            vec![
                graph_step("gen", &[]),
                PlanStep {
                    sub_flow_id: Some("flow-子".into()),
                    ..step("反思", "reflect", None)
                },
            ],
        );
        let err = validate_flow(&cfg, &tools()).unwrap_err();
        assert!(err.contains("反思步骤,不支持挂载子流程"), "实际错误:{err}");
    }

    #[test]
    fn blank_sub_flow_id_means_not_mounted() {
        // 空串按未挂载处理(与 inputs 同一「空值即缺省」口径):编辑器清空选择时不写脏引用
        let mut s = graph_step("n", &[]);
        s.sub_flow_id = Some("   ".into());
        assert!(!s.is_sub_flow());
        assert_eq!(s.sub_flow_ref(), None);
        s.sub_flow_id = Some(" flow-x ".into());
        assert!(s.is_sub_flow());
        assert_eq!(s.sub_flow_ref(), Some("flow-x"), "引用应 trim 后使用");
    }

    #[test]
    fn set_rejects_edit_that_deepens_another_flows_chain() {
        // 「后门」用例:编辑 B 可能让引用 B 的 A 变深,只查被编辑流程自身会漏。
        let dir = TempDataDir::new("flow-subflow-deepen");
        let mut svc = service(&dir);
        let d = save_flow(&mut svc, "D", vec![graph_step("n", &[])]);
        let c = save_flow(&mut svc, "C", vec![sub_step("n", &d)]);
        let b = save_flow(&mut svc, "B", vec![sub_step("n", &c)]);
        save_flow(&mut svc, "A", vec![sub_step("n", &b)]);
        // 此刻 A→B→C→D 正好 3 层,合法
        // 把 D 改成再嵌一层 E:C 自身只深 2 层,但 A 的链变成 4 层 → 必须被拒
        let e = save_flow(&mut svc, "E", vec![graph_step("n", &[])]);
        let mut d_cfg = svc.flow_by_id(&d).unwrap().clone();
        d_cfg.steps = vec![sub_step("n", &e)];
        let err = svc.set(d_cfg).unwrap_err();
        assert!(err.contains("嵌套超过"), "实际错误:{err}");
        // 拒绝不落盘:库里 D 仍是原来的单节点
        assert_eq!(svc.flow_by_id(&d).unwrap().steps.len(), 1);
    }

    #[test]
    fn remove_rejects_flow_referenced_as_sub_flow() {
        let dir = TempDataDir::new("flow-subflow-del");
        let mut svc = service(&dir);
        let referrer = save_flow(&mut svc, "主", vec![sub_step("n", "builtin-coordination")]);
        let err = svc.remove("builtin-coordination").unwrap_err();
        assert!(err.contains("子流程引用"), "实际错误:{err}");
        assert!(err.contains("主"), "应点名引用方:{err}");
        // 解除引用后即可删除
        let mut fixed = svc.flow_by_id(&referrer).unwrap().clone();
        fixed.steps[0].sub_flow_id = None;
        svc.set(fixed).unwrap();
        assert!(svc.remove("builtin-coordination").is_ok());
    }

    /// 把流程库写进临时数据目录(模拟「手改 data/agent_flows.json」这类脏数据来源)
    fn write_library(dir: &TempDataDir, lib: &AgentFlowLibrary) {
        std::fs::write(
            dir.join("agent_flows.json"),
            serde_json::to_string_pretty(lib).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn validate_checks_only_entry_chain_while_set_scans_whole_library() {
        let dir = TempDataDir::new("flow-validate-scope");
        // 库内有一个**无关**的非法流程(悬空引用)——只能来自手改文件/外部导入,
        // 因为 `set` 的全库校验不允许它经正常写入路径进库
        write_library(
            &dir,
            &library(vec![
                flow("入口", vec![graph_step("n", &[])]),
                flow("脏流程", vec![sub_step("bad", "missing-flow")]),
            ]),
        );
        let mut svc = service(&dir);
        let entry = svc.get().cloned().expect("当前流程应存在");
        assert_eq!(entry.name, "入口");

        // 执行期:只从入口起链 → 无关脏流程不拦(否则所有 custom 任务与聊天预览都起不来)
        assert!(
            svc.validate(&entry).is_ok(),
            "库内无关流程的脏引用不应拦死本次执行"
        );

        // 保存期:仍是全库口径 → 拒绝,且拒绝不落盘
        let err = svc
            .set(flow("新流程", vec![graph_step("n", &[])]))
            .unwrap_err();
        assert!(err.contains("不存在"), "实际错误:{err}");
        assert!(
            !svc.get_library().flows.iter().any(|f| f.name == "新流程"),
            "被拒的保存不应进库"
        );
    }

    #[test]
    fn validate_rejects_broken_entry_chain() {
        let dir = TempDataDir::new("flow-validate-entry");
        // 入口流程**自己**引用了不存在的流程 → 可达链非法,执行期必须拒
        write_library(
            &dir,
            &library(vec![
                flow("入口", vec![sub_step("n", "missing")]),
                flow("旁支", vec![graph_step("n", &[])]),
            ]),
        );
        let svc = service(&dir);
        let entry = svc.get().cloned().unwrap();
        let err = svc.validate(&entry).unwrap_err();
        assert!(err.contains("不存在"), "实际错误:{err}");
    }

    #[test]
    fn remove_ignores_references_from_disabled_steps() {
        // 引用口径与校验口径一致:只看**启用步骤**(停用步骤的残留引用不参与执行,
        // 也不参与保存期校验,拿它卡住删除只会让用户被一条不存在的约束拦住)
        let dir = TempDataDir::new("flow-subflow-del-disabled");
        let mut svc = service(&dir);
        let mut stale = sub_step("n", "builtin-coordination");
        stale.enabled = false;
        save_flow(&mut svc, "主", vec![stale, graph_step("g", &[])]);
        assert!(
            svc.remove("builtin-coordination").is_ok(),
            "停用步骤里的残留引用不应阻止删除"
        );

        // 启用步骤里的引用仍然阻止(既有保护不变)
        let dir2 = TempDataDir::new("flow-subflow-del-enabled");
        let mut svc2 = service(&dir2);
        save_flow(
            &mut svc2,
            "主",
            vec![sub_step("n", "builtin-coordination"), graph_step("g", &[])],
        );
        let err = svc2.remove("builtin-coordination").unwrap_err();
        assert!(err.contains("子流程引用"), "实际错误:{err}");
    }

    #[test]
    fn expand_sub_flows_splices_nested_steps_with_name_prefix() {
        // 子流程两步线性;主流程三步线性(起草 → 挂子流程 → 收尾)
        let sub = flow("子", vec![graph_step("s1", &[]), graph_step("s2", &["s1"])]);
        let main = flow(
            "主",
            vec![
                graph_step("a", &[]),
                sub_step("b", "flow-子"),
                graph_step("c", &["b"]),
            ],
        );
        let lib = library(vec![main.clone(), sub]);
        let active: Vec<PlanStep> = main.steps.iter().filter(|s| s.enabled).cloned().collect();
        let out = expand_sub_flows(&lib, &main.id, &active).unwrap();
        let names: Vec<&str> = out.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "节点a",
                "【子流程「子」】节点s1",
                "【子流程「子」】节点s2",
                "节点c"
            ],
            "子流程节点就地展开,位置即外层拓扑序中的位置"
        );
        assert!(
            out.iter().all(|s| s.inputs.is_empty()),
            "展开产物一律清空 id 依赖(扁平列表已按拓扑序排好)"
        );
        assert!(
            out[1].sub_flow_id.is_none() && out[2].sub_flow_id.is_none(),
            "子图内部节点不应残留 sub_flow_id"
        );
    }

    #[test]
    fn expand_sub_flows_rewrites_ids_per_mount_point() {
        // 同一子流程被两个节点挂载:展开后 id 必须全局唯一,否则聊天侧
        // make_custom_plan → resolve_graph 会以「步骤 id 重复」直接 400,
        // 而同一份流程在任务侧(custom)能正常跑完(任务侧不展开,走 run_sub_flow)。
        let sub = flow("子", vec![graph_step("s1", &[]), graph_step("s2", &["s1"])]);
        let main = flow(
            "主",
            vec![
                graph_step("a", &[]),
                sub_step("b", "flow-子"),
                graph_step("c", &["b"]),
                sub_step("d", "flow-子"),
                graph_step("e", &["d"]),
            ],
        );
        let lib = library(vec![main.clone(), sub]);
        let active: Vec<PlanStep> = main.steps.iter().filter(|s| s.enabled).cloned().collect();
        let out = expand_sub_flows(&lib, &main.id, &active).unwrap();
        let ids: Vec<&str> = out.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["a", "s1/s1", "s1/s2", "c", "s3/s1", "s3/s2", "e"],
            "子图步骤 id 按挂载位置加前缀(下标取挂载节点在本层数组中的位置),顶层 id 不变"
        );
        assert!(resolve_graph(&out).is_ok(), "展开产物 id 必须唯一且非空");

        // 父子撞 id(「复制流程」只换流程 id、steps 深拷贝 → 两边步骤 id 相同)
        let main2 = flow(
            "主2",
            vec![
                graph_step("a", &[]),
                sub_step("b", "flow-撞"),
                graph_step("c", &["b"]),
            ],
        );
        let clash = library(vec![main2.clone(), flow("撞", vec![graph_step("b", &[])])]);
        let out2 = expand_sub_flows(&clash, &main2.id, &main2.steps).unwrap();
        let ids2: Vec<&str> = out2.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids2, vec!["a", "s1/b", "c"], "父子同 id 也被前缀隔开");
    }

    #[test]
    fn expand_sub_flows_self_check_reports_duplicate_ids() {
        // 极端配置:顶层步骤 id 与「子图步骤 + 派生前缀」同形。
        // 自检兜底给出点名 id 的中文错误,而不是让下游报「步骤 id 重复」。
        let sub = flow("子", vec![graph_step("leaf", &[])]);
        let main = flow(
            "主",
            vec![
                graph_step("a", &[]),
                sub_step("b", "flow-子"),
                graph_step("s1/leaf", &["b"]),
            ],
        );
        let lib = library(vec![main.clone(), sub]);
        let active: Vec<PlanStep> = main.steps.iter().filter(|s| s.enabled).cloned().collect();
        let err = expand_sub_flows(&lib, &main.id, &active).unwrap_err();
        assert!(err.contains("子流程展开后步骤 id 冲突"), "实际错误:{err}");
        assert!(err.contains("s1/leaf"), "应点名重复的 id:{err}");
    }

    #[test]
    fn sub_flow_depth_limit_counts_ancestor_chain() {
        // 祖先链:保存期从**库里每个流程**起链,故 H 在最上层就让整条链超限。
        // 前端 flowAncestorDepth + stepSubFlowWarnings 的深度判定与这条等价。
        let too_deep = library(vec![
            flow("H", vec![sub_step("n", "flow-G")]),
            flow("G", vec![sub_step("n", "flow-F")]),
            flow("F", vec![sub_step("n", "flow-X")]),
            flow("X", vec![sub_step("n", "flow-W")]),
            flow("W", vec![graph_step("n", &[])]),
        ]);
        let err = validate_sub_flows(&too_deep, &tools()).unwrap_err();
        assert!(err.contains("嵌套超过"), "实际错误:{err}");
        assert!(
            err.contains("H") && err.contains("W"),
            "应点名整条链的两端:{err}"
        );

        // 去掉最上层的 H → 4 个流程 = 3 层,上限内
        let ok = library(vec![
            flow("G", vec![sub_step("n", "flow-F")]),
            flow("F", vec![sub_step("n", "flow-X")]),
            flow("X", vec![sub_step("n", "flow-W")]),
            flow("W", vec![graph_step("n", &[])]),
        ]);
        assert!(
            validate_sub_flows(&ok, &tools()).is_ok(),
            "3 层嵌套应放行(上限 {MAX_SUB_FLOW_DEPTH})"
        );
    }

    #[test]
    fn expand_sub_flows_rejects_runtime_cycle_and_depth() {
        // 环(保存期已拒,此处是外部手改配置的运行期兜底)
        let a = flow("A", vec![sub_step("n", "flow-B")]);
        let lib = library(vec![a.clone(), flow("B", vec![sub_step("n", "flow-A")])]);
        let err = expand_sub_flows(&lib, &a.id, &a.steps).unwrap_err();
        assert!(err.contains("环"), "实际错误:{err}");

        // 深度:A→B→C→D→E(4 层嵌套)越界
        let deep = library(vec![
            flow("A", vec![sub_step("n", "flow-B")]),
            flow("B", vec![sub_step("n", "flow-C")]),
            flow("C", vec![sub_step("n", "flow-D")]),
            flow("D", vec![sub_step("n", "flow-E")]),
            flow("E", vec![graph_step("n", &[])]),
        ]);
        let entry = deep.flows[0].clone();
        let err = expand_sub_flows(&deep, &entry.id, &entry.steps).unwrap_err();
        assert!(err.contains("嵌套超过"), "实际错误:{err}");

        // 悬空引用
        let dangling = library(vec![flow("A", vec![sub_step("n", "missing")])]);
        let entry = dangling.flows[0].clone();
        let err = expand_sub_flows(&dangling, &entry.id, &entry.steps).unwrap_err();
        assert!(err.contains("不存在"), "实际错误:{err}");
    }

    #[test]
    fn sub_flow_field_is_omitted_when_unset() {
        // 兼容承诺(二维批次 1):存量流程 JSON 读写逐字节不变——
        // 未挂子流程的步骤不得序列化出 sub_flow_id 键
        let s = graph_step("n", &[]);
        let text = serde_json::to_string(&s).unwrap();
        assert!(
            !text.contains("sub_flow_id"),
            "未挂子流程时不应出现该键:{text}"
        );
        let mounted = sub_step("n", "flow-x");
        let text = serde_json::to_string(&mounted).unwrap();
        assert!(text.contains("sub_flow_id"), "挂载时应落盘:{text}");
    }

    // ===== 任务用流程快照(二维批次 5a) =====

    #[test]
    fn snapshot_for_collects_transitive_sub_flow_closure() {
        let dir = TempDataDir::new("flow-snapshot-closure");
        let mut svc = service(&dir);
        // C(叶子)← B(挂 C)← A(挂 B):入口 A 的闭包应含 A + B + C,入口恒首个
        let c = save_flow(&mut svc, "C", vec![graph_step("n", &[])]);
        let b = save_flow(&mut svc, "B", vec![sub_step("n", &c)]);
        let a = save_flow(&mut svc, "A", vec![sub_step("n", &b)]);

        let snap = svc.snapshot_for(&a).unwrap();
        assert_eq!(snap.root_id, a, "根应为入口流程");
        let ids: Vec<&str> = snap.flows.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![a.as_str(), b.as_str(), c.as_str()],
            "闭包应逐层可达"
        );
        assert_eq!(snap.root().map(|f| f.id.as_str()), Some(a.as_str()));
        assert!(snap.flow_by_id(&c).is_some(), "闭包内可取到深层子流程");
        assert!(
            snap.flow_by_id("flow-不存在").is_none(),
            "闭包外的一律取不到(运行期子图解析只认快照)"
        );
    }

    #[test]
    fn snapshot_for_rejects_unknown_and_disabled_flow() {
        let dir = TempDataDir::new("flow-snapshot-reject");
        write_library(
            &dir,
            &library(vec![flow("启用流程", vec![graph_step("n", &[])]), {
                let mut off = flow("停用流程", vec![graph_step("n", &[])]);
                off.enabled = false;
                off
            }]),
        );
        let svc = service(&dir);

        let err = svc.snapshot_for("flow-不存在").unwrap_err();
        assert!(err.contains("不存在"), "实际错误:{err}");
        // 未启用流程不可绑定:与执行期 current_flow 的 enabled 口径一致,提前拒绝
        let err = svc.snapshot_for("flow-停用流程").unwrap_err();
        assert!(err.contains("未启用"), "实际错误:{err}");
    }

    #[test]
    fn frozen_snapshot_stays_valid_after_library_changes() {
        let dir = TempDataDir::new("flow-snapshot-frozen");
        let mut svc = service(&dir);
        let sub = save_flow(&mut svc, "子", vec![graph_step("n", &[])]);
        let main = save_flow(&mut svc, "主", vec![sub_step("n", &sub)]);
        let snap = svc.snapshot_for(&main).unwrap();

        // 库里把子流程改成悬空引用(只有手改文件能做到)→ 快照自身仍合法:
        // 被冻结的任务不该因为库里被改而失效
        let mut broken = flow("子", vec![sub_step("n", "missing")]);
        broken.id = sub.clone();
        write_library(
            &dir,
            &library(vec![
                {
                    let mut m = flow("主", vec![sub_step("n", &sub)]);
                    m.id = main.clone();
                    m
                },
                broken,
            ]),
        );
        let svc2 = service(&dir);
        assert!(
            svc2.validate(svc2.get().unwrap()).is_err(),
            "实时库口径下入口链已非法(对照:证明上面确实把库改坏了)"
        );
        assert!(
            validate_snapshot(&snap, &tools()).is_ok(),
            "冻结快照的校验只看快照自身"
        );
    }
}
