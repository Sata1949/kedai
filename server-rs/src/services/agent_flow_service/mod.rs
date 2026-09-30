// 自定义 Agent 执行流程服务(custom 模式):用户可编辑步骤序列(名称/目标/生成开关/
// 反思/系统提示词/温度/输出上限/工具白名单),持久化到 data/agent_flows.json。
// 支持多流程库:可新建/复制/选择/删除/导入/导出流程,当前选中流程决定 custom 模式行为。
// 与提示词注入服务同构:全局作用域、内存缓存配置、全量读写。
//
// QUALITY-FIX Q1-3(2026-09-27):本模块由单文件 3249 行拆成目录模块——`mod.rs` 只留
// **公共词汇**(全部 pub 类型与常量)与 re-export,实现按流程阶段分为 service / library /
// graph / subflow / bundle 五个子模块,单元测试搬到 `tests.rs`。纯移动、不改行为;
// 外部 `crate::services::agent_flow_service::<名>` 路径因此零改动。
//
// 新增代码放哪个子模块:改流程的读写/持久化 → service.rs;流程库载入保存与内置流程 →
// library.rs;步骤图与校验 → graph.rs;子流程校验与展开 → subflow.rs;搬运包与快照 → bundle.rs;
// 跨子模块复用的新类型/常量 → 本文件(再 `pub use` 需要对外暴露的自由函数)。

use crate::models::types::PlanStep;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::PathBuf;

mod bundle;
mod graph;
mod library;
mod service;
mod subflow;

// 子模块内定义的公共项在此 re-export:外部 `crate::services::agent_flow_service::<名>` 路径零改动
// (`mod.rs` 里定义的类型与常量无需 re-export)。
pub use bundle::validate_snapshot;
pub use graph::{
    effective_inputs, effective_max_parallel, is_linear_compat, output_index, resolve_graph,
    validate_flow,
};
pub use library::builtin_flow;
// 能力包流程并入缝(CODE-5):生产侧由 service.rs 经 super::* 使用;
// `coding_pack_flows` 只有测试直接引用(生产只经 pack_flows 间接消费),故单独标 cfg(test) 免 unused 告警
#[cfg(test)]
pub(crate) use library::coding_pack_flows;
pub(crate) use library::{merge_pack_flows, pack_flows};
pub use subflow::{expand_sub_flows, flow_label, validate_sub_flows};

#[cfg(test)]
mod tests;

/// 自定义执行流程服务:持有 data_dir,内存缓存流程库,读写 data/agent_flows.json
pub struct AgentFlowService {
    data_dir: PathBuf,
    library: AgentFlowLibrary,
    registered_tools: BTreeSet<String>,
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
    /// **已注入过的能力包流程 id**(CODE-5,2026-09-30):防重复注入,兼作「删过/改过不复活」
    /// 的凭据——注入过就不再注入,与之后被删被改无关,故无需单独墓碑。
    ///
    /// 加性字段:`skip_serializing_if` 让空值(从未开过任何包)时的库文件**逐字节不变**;
    /// 旧库文件缺该键时按空表解析(serde default)。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub seeded_pack_ids: Vec<String>,
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

/// **流程搬运包**(二维批次 7a):导出文件的线格式。
///
/// 为什么要有具名结构体而不是 `json!` 手拼:这是**跨进程、跨机器**的对外契约(文件会被用户
/// 带走),字段漂移的代价最高——只有具名结构体才能登记进 `tools/check-contract.mjs`,与前端
/// `AgentFlowBundle` 做机检(`DistillOutcome` 的漏字段先例就是这么漏出去的)。
/// 字段名即线格式键;`kedai_flow_bundle` 与 [`FLOW_BUNDLE_KEY`] 的一致性由单测锁死。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowBundle {
    /// 版本号(键名见 [`FLOW_BUNDLE_KEY`])
    pub kedai_flow_bundle: u32,
    /// 导出时刻(ISO 字符串;仅作溯源展示)
    pub exported_at: String,
    /// 入口流程 id(全库导出时 = 当前流程;可能为 null)
    pub root_id: Option<String>,
    /// 包内流程:入口 + 可达子流程闭包,或全库
    pub flows: Vec<AgentFlowConfig>,
}

/// 一条 id 重映射记录(导入报告用):同 id 内容不同时,导入的那份被分配了新 id。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FlowIdRemap {
    /// 文件里的原 id
    pub old_id: String,
    /// 本库实际分配的新 id
    pub new_id: String,
    /// 流程展示名(报告文案里点名用)
    pub name: String,
}

/// 导入报告:前端据此给出「导入 N 个(跳过 M 个,其中 K 个因 id 冲突分配了新 id)」。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FlowImportReport {
    /// 本次**写入库**的流程数(新增 + 覆盖;B4 之前恒等于新增数)
    pub imported: usize,
    /// 因「本库已有同 id 且内容相同」而跳过的流程数(重复导入同一份文件 → imported=0)
    pub skipped: usize,
    /// 因 id 冲突被分配新 id 的流程清单(仅 `rename` 模式会非空)
    pub renamed: Vec<FlowIdRemap>,
    /// 被**覆盖**的流程清单(A 批 B4;仅 `replace` 模式会非空)。
    ///
    /// 覆盖保留 id(引用不破),故这里只需「覆盖了谁」——与 `renamed` 的「谁变成了谁」
    /// 是两件事,不复用同一个结构以免读者以为有 id 映射。
    #[serde(default)]
    pub replaced: Vec<FlowReplacement>,
}

/// 被覆盖的流程(A 批 B4):覆盖**保留 id**,故只记 id 与展示名(报告文案用)。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowReplacement {
    pub id: String,
    pub name: String,
}

/// 导入时的 id 冲突处理方式(A 批 B4)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImportConflictMode {
    /// 缺省(= 二维批次 7a 行为,零变化):同 id 内容不同 → 新增副本并分配新 id,
    /// 本库那份**原样不动**。
    #[default]
    Rename,
    /// 覆盖:同 id 内容不同 → **覆盖本库那一份**(id 不变 → 引用它的流程不受影响)。
    ///
    /// 三条边界是刻意的:① 只覆盖**文件里出现过的 id**,库中其它流程一律不动
    /// (绝不做「导入即清库」);② 内容相同仍走跳过(指纹幂等不变);③ 保留段 id
    /// (`export`/`import`/`select`)与空 id 仍走重命名——那些情形没有「本库同一份」可言。
    /// 覆盖不可逆,故请求侧要求**显式**传 `on_conflict=replace`(前端另加二次确认)。
    Replace,
}

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

/// 流程图解析结果:所有下标一律对齐传入的 `steps` **数组顺序**。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowGraph {
    /// 每个步骤的有效上游下标(已去重、按下标升序)
    pub inputs: Vec<Vec<usize>>,
    /// 执行拓扑序(Kahn;就绪节点按下标升序出队)
    pub order: Vec<usize>,
}

/// 并行节点数上限的默认值与上限(二维批次 2;产品风险 WF-11:
/// 并行 = 成本倍增,故默认保守取 2,并在前端给出提示)
pub const DEFAULT_MAX_PARALLEL_NODES: u32 = 2;

pub const MAX_PARALLEL_NODES_LIMIT: u32 = 8;

/// 节点级工具轮次上限的取值范围(二维批次 5b):与设置项 `max_tool_rounds` 的 1-200 同口径
pub const MAX_TOOL_ROUNDS_LIMIT: u32 = 200;

/// 节点级上下文上限的取值范围(二维批次 8)。
///
/// 上限与全局 `max_context_tokens` 同口径(`api/settings.rs` 的 65536..=1048576);
/// **下限刻意更低**(256):本字段的用途正是「给单个节点设**更小**的窗口」,沿用全局的低限
/// 会让它失去意义。更小的值也没有实用价值——节点输入至少含任务目标与省略标记。
pub const MIN_STEP_MAX_CONTEXT: u32 = 256;

pub const MAX_STEP_MAX_CONTEXT: u32 = 1_048_576;

/// 节点级单次调用超时的取值范围(A 批 A1;单位秒)。
///
/// 下限 30s:再低会把「上游首字延迟本就数秒」的正常调用误判为超时;
/// 上限 3600s:给慢上游留出比宿主缺省(300s)更宽的一档——若上限只到 300s,
/// 该字段就只剩「收紧」一种用法,「宽/严节点混用时间预算」的诉求落不了地。
pub const MIN_STEP_CALL_TIMEOUT_SECS: u32 = 30;

pub const MAX_STEP_CALL_TIMEOUT_SECS: u32 = 3600;

/// 节点级空产出重试次数的取值范围(A 批 A2;n = **额外**尝试上限,总尝试 = 1 + n)。
///
/// 上限 5 是成本闸:每次重试都是一次全量输入的重发,再高会让「一个坏节点」吃掉整个任务的
/// 预算。下限取 1 而非 0——「不重试」已由「清空该字段」表达,再放一个 0 只会让同一语义
/// 有两条路径(与 6a 裁定 17「不许存在但静默无效」同一纪律)。
pub const MIN_STEP_MAX_RETRIES: u32 = 1;

pub const MAX_STEP_MAX_RETRIES: u32 = 5;

/// 流程搬运包(导出文件)的版本键与版本号(二维批次 7a)。
///
/// 版本键**存在且不等于本值**时导入侧明确拒绝(提示升级),缺失该键则按旧格式容忍
/// (单流程 `{name,steps}` / `{config:{…}}` / 库格式 `{flows,current_flow_id}`)——
/// 旧文件是既有的用户数据,不能因为新增一整套搬运格式就导不进来。
pub const FLOW_BUNDLE_KEY: &str = "kedai_flow_bundle";

pub const FLOW_BUNDLE_VERSION: u32 = 1;

/// 子流程嵌套深度上限:入口流程算第 0 层,最多再嵌 3 层(整条链最多 4 个流程)。
///
/// 深度是**成本闸门**:每层子流程把该节点的调用次数乘上子图节点数(与 WF-11 的并行
/// 加倍叠加)。静态子图本身无环(保存期已拒),深度仍必须封顶——子图节点多、层数深时
/// 单节点的 token 消耗会以乘积增长。
pub const MAX_SUB_FLOW_DEPTH: usize = 3;

/// 动态调用的嵌套深度上限(二维批次 7b 对比模式):入口流程算第 0 层,最多再嵌 2 层。
///
/// 为什么**不复用** `MAX_SUB_FLOW_DEPTH`:两者是成本模型不同的两条轴——静态子图在
/// **保存期**就已定形(用户看得见连线),动态调用由**模型**在运行期决定。复用同一常量
/// 会让「静态子图想放宽一点」顺手放宽模型的可调用深度,也可能让「子智能体深度」这类
/// 无关设置意外改到流程嵌套。各自封顶、各自报错文案,谁也篡改不了谁。
///
/// 与静态子图深度**独立**计数:一次动态调用不额外占用静态子图的层数,反之亦然
/// (总嵌套因此以 3 + 2 为界,再乘每任务调用预算 [MAX_FLOW_CALLS_PER_TASK] 的封顶)。
pub const MAX_FLOW_CALL_DEPTH: usize = 2;

/// 单个任务内动态调用的次数上限(二维批次 7b)。
///
/// 深度与环守卫拦的是「同一条调用链」,拦不住「模型串行地反复调用同一套流程」——
/// 每次调用都是一整套流程的模型调用,只有按**任务**计数才能给总成本封顶。
/// 计数在进程内、按任务、随本轮执行从零开始(重跑重新计数);超限的调用在**任何模型
/// 调用之前**就被拒绝,不烧 token。
pub const MAX_FLOW_CALLS_PER_TASK: usize = 8;

// ==================== 调用闸的可配置区间(A 批 A3) ====================
//
// 上面两个常量从此**退居缺省值与测试基准**:真源是任务侧设置项
// `max_flow_call_depth` / `max_flow_calls_per_task`(见 `settings_service::params`),
// 消费点在 `task_engine/flow_call.rs`(读 `ctx.settings`,不再读常量)。
// 区间上限刻意保守:两条都是**成本**闸,调大等于允许更长的模型自主链。

/// 动态调用深度上限的可配区间(1..=5):上限 5 给「深链编排」留余量,
/// 但不放开到无界——每加一层,单次语义的调用次数按子图规模相乘。
pub const MIN_FLOW_CALL_DEPTH: u32 = 1;

pub const MAX_FLOW_CALL_DEPTH_LIMIT: u32 = 5;

/// 每任务动态调用次数上限的可配区间(1..=64):64 = 缺省值 8 的 8 倍,
/// 够跑「一次任务里逐项处理十来个条目」这类用法,又仍是一个有限上界。
pub const MIN_FLOW_CALLS_PER_TASK: u32 = 1;

pub const MAX_FLOW_CALLS_PER_TASK_LIMIT: u32 = 64;

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
            seeded_pack_ids: Vec::new(),
        }
    }
}
