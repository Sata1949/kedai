//! 流程服务的读写面:`AgentFlowService` 的全部方法与私有辅助。

// 子模块内统一用 super::* 拿到 mod.rs 的公共词汇(类型/常量)与 re-export 的自由函数。
use super::*;
use crate::models::types::PlanStep;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use uuid::Uuid;

// 跨子模块的私有辅助(pub(super),只在本目录内可见)。
use super::bundle::{flow_fingerprint, is_reserved_flow_id};
use super::subflow::{flow_index, sub_flow_ids, walk_sub_flows_from};

impl AgentFlowService {
    /// `coding_bundle_enabled`:编码能力包开关(CODE-5)的**有效值**(按 task 覆盖层合并后的,
    /// 见 `api/app_state.rs` 的读取口径)。构造期并入一次,覆盖「库文件已存在但从未注入」的
    /// 升级路径;运行期用户开/关包由 `api/settings.rs` 的写入钩子调 [`Self::sync_pack_flows`]。
    pub fn new(
        data_dir: PathBuf,
        registered_tools: Vec<String>,
        coding_bundle_enabled: bool,
    ) -> Self {
        let mut library = AgentFlowLibrary::load(&data_dir);
        if merge_pack_flows(&mut library, pack_flows(coding_bundle_enabled)) {
            // 落盘失败只记错误:流程库已在内存里可用,下次启动会再试一次(不静默吞掉)
            if let Err(e) = library.save(&data_dir) {
                tracing::error!(error = e, "能力包流程并入后写入失败");
            }
        }
        AgentFlowService {
            data_dir,
            library,
            registered_tools: registered_tools.into_iter().collect(),
        }
    }

    /// 按开关同步能力包流程(CODE-5;设置写入钩子与构造期共用)。
    ///
    /// 语义只有两条:**开包 → 并入缺失的包流程**(幂等,已注入过的跳过);
    /// **关包 → 什么都不做**(已注入的副本留在用户库里,不回收——既定边界,同 LIT-5 Q2)。
    /// 有变更才落盘,故重复开包不会反复写文件。
    pub fn sync_pack_flows(&mut self, coding_bundle_enabled: bool) {
        if !merge_pack_flows(&mut self.library, pack_flows(coding_bundle_enabled)) {
            return;
        }
        if let Err(e) = self.save_library() {
            tracing::error!(error = e, "能力包流程并入后写入失败");
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

    /// 当前流程 → 任务用快照(二维批次 5a:未绑定任务的执行前解析口径;7b 携对比模式名单)。
    ///
    /// 与 [`snapshot_for`] 的差别只在「取哪一份流程」:本方法是「跟随当前流程」分支,
    /// 故保留 current_flow 的既有语义(**要求 enabled**),校验与闭包收集同一套。
    ///
    /// 名单成员按**当时的库**解析且取**宽松**口径(二维批次 7b):未绑定任务的快照本就是
    /// 「执行时才捕获」的语义,成员在创建后被删/停用/改坏时从可调用集里剔除并告警即可
    /// ——「根流程照常执行」是主语义,不该因为一个成员坏了整任务都跑不起来。
    pub fn current_snapshot(&self, extra_ids: &[String]) -> Result<FlowSnapshot, String> {
        let cfg = self.get().ok_or("请先在设置中启用一个 Agent 流程")?;
        if !cfg.enabled {
            return Err("当前 Agent 流程未启用,请在设置中开启后再运行 custom 模式".into());
        }
        let extras = self.resolve_members(extra_ids, false)?;
        self.build_snapshot(cfg, &extras)
    }

    /// 绑定流程 id → 任务用快照(二维批次 5a:任务创建期与绑定任务的执行前解析)。
    ///
    /// 与「当前流程」同一取用纪律:要求该流程存在且**已启用**(未启用即拒绝绑定,
    /// 避免任务建好一跑就 error),校验口径与 [`validate`] 一致(结构 + 从入口起可达的
    /// 引用链,不看库里无关流程的脏数据)。
    ///
    /// 名单成员(`extra_ids`,二维批次 7b)取**严格**口径:创建期逐项点名报错,不接受
    /// 「字段存在但静默无效」;并显式拒绝「根流程出现在名单里」——根流程正在执行,
    /// 被调用必然撞调用链环守卫,放进名单只会让描述文案与现实不一致。
    pub fn snapshot_for(
        &self,
        flow_id: &str,
        extra_ids: &[String],
    ) -> Result<FlowSnapshot, String> {
        let cfg = self
            .flow_by_id(flow_id)
            .ok_or_else(|| format!("所选流程不存在:{flow_id}"))?;
        if !cfg.enabled {
            return Err(format!(
                "所选流程「{}」未启用,请先在设置中开启后再绑定",
                flow_label(cfg)
            ));
        }
        // 只有**绑定根流程**这一分支才判「名单不得含根」:未绑定任务跟随当前流程,
        // 根是哪一份要等执行时才定,那种情况由运行期可调用集扣除(见 task_engine/flow_call.rs)。
        if extra_ids.iter().any(|id| id == flow_id) {
            return Err(format!(
                "对比模式名单不能包含根流程「{}」:该流程正在执行,无法再被调用",
                flow_label(cfg)
            ));
        }
        let extras = self.resolve_members(extra_ids, true)?;
        self.build_snapshot(cfg, &extras)
    }

    /// 对比模式名单成员**严格**校验(二维批次 7b;任务创建期调用)。
    ///
    /// 未绑定根流程的任务也要验:字段不许静默无效——名单里写了不存在的流程,或写了一套
    /// 结构不合法的流程,必须在创建时就报错,而不是等模型调用时才失败。
    pub fn validate_members(&self, ids: &[String]) -> Result<(), String> {
        self.resolve_members(ids, true).map(|_| ())
    }

    /// 对比模式名单成员解析(二维批次 7b):逐项要求存在 → 启用 → 结构合法。
    ///
    /// `strict = true`(创建期):任一不满足即整单拒绝,错误文案点名是哪一份、哪一条;
    /// `strict = false`(运行期,未绑定任务的「当时的库」):不满足者剔除并 `warn`,
    /// 返回仍可用的那些(调用方据此算可调用集)。
    ///
    /// 结构校验必须在这里做:名单成员**未必**被任何流程以 `sub_flow_id` 引用,而
    /// `validate_snapshot` 的 `validate_sub_flows` 只校验「链首 + 被引用方」——不主动
    /// 验一遍,坏流程会以「描述里列着、一调就报错」的形态漏到运行期。
    fn resolve_members(
        &self,
        ids: &[String],
        strict: bool,
    ) -> Result<Vec<AgentFlowConfig>, String> {
        let mut out: Vec<AgentFlowConfig> = Vec::new();
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for id in ids {
            let id = id.trim();
            if id.is_empty() || !seen.insert(id) {
                continue;
            }
            let reason = match self.flow_by_id(id) {
                None => Some(format!("流程不存在:{id}")),
                Some(cfg) if !cfg.enabled => Some(format!("流程「{}」未启用", flow_label(cfg))),
                Some(cfg) => validate_flow(cfg, &self.registered_tools)
                    .err()
                    .map(|e| format!("流程「{}」结构不合法:{e}", flow_label(cfg))),
            };
            match (reason, strict) {
                (None, _) => {
                    // 上面已确认命中;expect 会被规则 E 拦,改用 if let 再取一次
                    if let Some(cfg) = self.flow_by_id(id) {
                        out.push(cfg.clone());
                    }
                }
                (Some(msg), true) => return Err(format!("对比模式名单: {msg}")),
                (Some(msg), false) => {
                    tracing::warn!(flow_id = id, reason = msg, "对比模式名单成员不可用,已剔除");
                }
            }
        }
        Ok(out)
    }

    /// 校验一份流程 + 收集它的子流程闭包(两条入口共用的实现)。
    /// `extras` 为对比模式名单成员(二维批次 7b;根流程之外**必须**在冻结域内的那些流程)。
    fn build_snapshot(
        &self,
        root: &AgentFlowConfig,
        extras: &[AgentFlowConfig],
    ) -> Result<FlowSnapshot, String> {
        self.validate(root)?;
        Ok(self.closure_from_roots(root, extras))
    }

    /// 入口流程 + **名单成员** + 可达子流程闭包(入口恒为首个;名单按声明顺序紧随其后;
    /// 子流程按「发现顺序」= 逐层按引用声明顺序)。
    ///
    /// 复用 `sub_flow_edges` 这一条「什么算被引用」的判定(与保存期校验、删除保护同源),
    /// 因此闭包与校验对「可达」的理解不可能分叉;环/深度由调用方的 [`validate`] 挡住,
    /// 这里只做收集(`seen` 去重,不会死循环;悬空引用也在 validate 处拦下)。
    ///
    /// 二维批次 7b 扩了**起点集**:对比模式下被调用流程与子流程一样,必须在**冻结域**内
    /// ——「快照外的一律取不到」这条纪律不因调用方式(静态挂载 / 动态调用)而分叉。
    fn closure_from_roots(
        &self,
        root: &AgentFlowConfig,
        extras: &[AgentFlowConfig],
    ) -> FlowSnapshot {
        let by_id = flow_index(&self.library);
        let mut flows: Vec<AgentFlowConfig> = vec![root.clone()];
        let mut seen: BTreeSet<String> = BTreeSet::from([root.id.clone()]);
        for extra in extras {
            if seen.insert(extra.id.clone()) {
                flows.push(extra.clone());
            }
        }
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

    /// 导出**流程搬运包**(二维批次 7a):`root` 为 `Some(id)` 时导出该流程 + 其**可达子流程
    /// 闭包**,`None` 时导出整个流程库。
    ///
    /// 为什么必须带子流程闭包:节点挂载的是 `sub_flow_id` 引用,只导入口流程等于导出一份
    /// **引用了不存在的子流程**的文件——导入侧 `validate_sub_flows` 必然 400,「导出即坏文件」。
    /// 闭包取的是 `sub_flow_closure`(与任务快照 `build_snapshot`、删除保护同一个「什么算被引用」
    /// 的判定),故「导出的那些流程」与「跑任务时冻结的那些流程」不可能分叉。
    ///
    /// 与 `snapshot_for` 的差别:后者是**任务绑定**路径,要求流程已启用(任务绑定一套停用流程
    /// 没有意义);导出是**搬运**路径,停用流程(草稿、辅助子流程)同样要能带走,故这里只做
    /// [`validate`](Self::validate) 的结构 + 可达引用链校验,不看 `enabled`。
    ///
    /// 产物形状见 `docs/契约-协议与配置.md`「流程搬运契约」;**不含**连接定义与密钥
    /// (连接是本机 settings,见 D7 口径);节点级 `connection_id` 原样保留,跨机导入后由
    /// 编辑期警示 + 运行期报错兜住(二维批次 5b 口径)。
    pub fn export_bundle(&self, root: Option<&str>) -> Result<FlowBundle, String> {
        let (root_id, flows) = match root {
            Some(id) => {
                let cfg = self
                    .flow_by_id(id)
                    .ok_or_else(|| format!("流程不存在:{}", id))?;
                self.validate(cfg)?;
                let snapshot = self.closure_from_roots(cfg, &[]);
                (Some(snapshot.root_id), snapshot.flows)
            }
            // 全库导出:只读不校验——库一旦有脏引用(手改 JSON),导出仍应可用;
            // 导入侧才是写路径,校验在那里把关。
            None => (
                self.library.current_flow_id.clone(),
                self.library.flows.clone(),
            ),
        };
        Ok(FlowBundle {
            kedai_flow_bundle: FLOW_BUNDLE_VERSION,
            exported_at: crate::models::db::now_iso(),
            root_id,
            flows,
        })
    }

    /// 导入流程搬运包(二维批次 7a):**只新增,绝不改写库里既有 id 的既有流程**。
    ///
    /// 判定顺序(口径见 `docs/契约-协议与配置.md`「流程搬运契约」):
    ///  1. **内容已存在**(本库或本批已有逐字节相同的流程)→ 跳过。重复导入同一份文件因此
    ///     幂等——哪怕上次导入因 id 冲突被分配了新 id,这次的同一份内容仍会被认出来;
    ///     若已存在的那份 id 与文件不同,把批内对它的引用改指向**已存在的那份**(否则
    ///     「入口被导入、子流程被跳过」会留下悬空引用);
    ///  2. id 在本库不存在 → **沿用原 id**(跨机搬运时 id 保持稳定);
    ///  3. 同 id 但内容不同 → 分配新 id 并登记重映射(绝不覆盖用户已有流程);
    ///  4. id 为空 → 分配新 id。
    ///
    /// 重映射后按映射表改写**本批**所有 `steps[].sub_flow_id`——批内引用一律以「导入的这一份」
    /// 为准,不会静默绑到本库同 id 的另一份流程上(那会悄悄改变编排语义);只有「该流程被跳过」
    /// 这一种情形才指向本库的等价副本。批外引用由 [`validate_sub_flows`] 报错
    /// (同一个「什么算合法引用」的判定,此处不复制规则)。
    ///
    /// **原子性**:全部校验在**候选库**上做完才换库,故**校验失败**时 `self.library` 与磁盘
    /// 逐字节不变(逐个 PUT 的旧导入路径做不到这点:先导入的流程已落盘,后面的引用校验才失败)。
    /// 落盘本身失败(磁盘满 / 权限 / 杀软占用)时内存库回滚到导入前,避免「内存里有、磁盘上没有」
    /// 的幻影流程(重启即消失)——`set` / `select` / `remove` 没有这层回滚,那是既有形态。
    pub fn import_bundle(
        &mut self,
        flows: Vec<AgentFlowConfig>,
        root_id: Option<&str>,
        mode: ImportConflictMode,
    ) -> Result<FlowImportReport, String> {
        if flows.is_empty() {
            return Err("导入文件里没有流程".into());
        }
        // 批内重复 id:文件自相矛盾(同一 id 两份),不猜用户意图直接拒绝;
        // 若放行,两份都会沿用同一个 id,库内出现重复 id 的流程。
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for f in &flows {
            let id = f.id.trim();
            if id.is_empty() {
                continue;
            }
            if !seen.insert(id.to_string()) {
                return Err(format!("导入文件内存在重复的流程 id:{}", id));
            }
        }
        // 「内容已存在」的判定基准 = 本库 + 本批已接受的流程(逐字节指纹 → 其 id)。
        // 一次算好,避免逐条与全库反复序列化比较;值(已存在那份的 id)用于跳过后修引用。
        let mut known: BTreeMap<String, String> = BTreeMap::new();
        for f in &self.library.flows {
            known.insert(flow_fingerprint(f)?, f.id.clone());
        }

        let mut report = FlowImportReport::default();
        let mut remap: BTreeMap<String, String> = BTreeMap::new();
        // 覆盖模式(A 批 B4)下被覆盖的 id:建库时按 id 替换而非追加
        let mut replaced_ids: BTreeSet<String> = BTreeSet::new();
        let mut incoming: Vec<AgentFlowConfig> = Vec::with_capacity(flows.len());
        for mut cfg in flows {
            let old_id = cfg.id.trim().to_string();
            cfg.id = old_id.clone();
            let fingerprint = flow_fingerprint(&cfg)?;
            if let Some(kept_id) = known.get(&fingerprint).cloned() {
                // 内容已存在 → 跳过;id 与文件不同则把引用改指向已存在的那份(防悬空)
                if !old_id.is_empty() && old_id != kept_id {
                    remap.insert(old_id, kept_id);
                }
                report.skipped += 1;
                continue;
            }
            // id 处置:保留段(删不掉,见 RESERVED_FLOW_IDS)/ 冲突 → 分配新 id 并登记映射。
            // 「内容相同」的情形已在上面跳过,故冲突只发生在「内容不同」。
            let conflicts = !old_id.is_empty() && self.flow_by_id(&old_id).is_some();
            let reserved = is_reserved_flow_id(&old_id);
            // 覆盖模式(A 批 B4):同 id 且内容不同 → 覆盖本库那份,**id 保持不变**。
            // 保留段不在此列(那类 id 没有「本库同一份」可言,仍走重命名);
            // 批内引用因此天然指向同名 id,不需要 remap(覆盖前已按指纹跳过同内容项)。
            if mode == ImportConflictMode::Replace && conflicts && !reserved {
                report.replaced.push(FlowReplacement {
                    id: old_id.clone(),
                    name: flow_label(&cfg),
                });
                replaced_ids.insert(old_id.clone());
                known.insert(fingerprint, cfg.id.clone());
                incoming.push(cfg);
                continue;
            }
            if old_id.is_empty() || conflicts || reserved {
                let new_id = Uuid::new_v4().to_string();
                if !old_id.is_empty() {
                    // 有旧 id 就要登记映射(批内引用得跟着走);空 id 无从映射
                    remap.insert(old_id.clone(), new_id.clone());
                }
                if conflicts || reserved {
                    // 空 id 不算「重命名」(旧单流程导出本就没有 id),其余都如实点名
                    report.renamed.push(FlowIdRemap {
                        old_id,
                        new_id: new_id.clone(),
                        name: flow_label(&cfg),
                    });
                }
                cfg.id = new_id;
            }
            known.insert(fingerprint, cfg.id.clone());
            incoming.push(cfg);
        }
        // 全部条目都已在本库(且内容相同)→ 库与当前选择都不动,如实回报告
        if incoming.is_empty() {
            return Ok(report);
        }
        for cfg in &mut incoming {
            for step in &mut cfg.steps {
                let Some(sub_id) = step.sub_flow_ref() else {
                    continue;
                };
                if let Some(new_id) = remap.get(sub_id) {
                    step.sub_flow_id = Some(new_id.clone());
                }
            }
        }
        // 校验:逐条结构(与 set 同一套规则)+ 候选库引用链(单一出处,不复制判定)
        for cfg in &incoming {
            validate_flow(cfg, &self.registered_tools)
                .map_err(|e| format!("导入的流程「{}」无效:{}", flow_label(cfg), e))?;
        }
        let mut candidate = self.library.clone();
        for cfg in &incoming {
            // 覆盖模式下按 id 原地替换(B4):id 不变 → 引用它的流程不受影响;
            // 其余一律追加(与 7a 行为逐字节一致)
            if replaced_ids.contains(&cfg.id) {
                if let Some(slot) = candidate.flows.iter_mut().find(|f| f.id == cfg.id) {
                    *slot = cfg.clone();
                    continue;
                }
            }
            candidate.flows.push(cfg.clone());
        }
        validate_sub_flows(&candidate, &self.registered_tools)?;

        report.imported = incoming.len();
        // 换库 + 选中入口:文件的 root_id(经重映射)优先,否则本批第一个;
        // root_id 命中的若是「被跳过」的项,也会落到本库那份等价副本上(remap 已登记)
        let previous = std::mem::replace(&mut self.library, candidate);
        let selected = root_id
            .map(|id| remap.get(id).cloned().unwrap_or_else(|| id.to_string()))
            .filter(|id| self.library.flows.iter().any(|f| f.id == *id))
            .or_else(|| incoming.first().map(|f| f.id.clone()));
        if let Some(id) = selected {
            self.library.current_flow_id = Some(id);
        }
        if let Err(e) = self.save_library() {
            // 落盘失败:内存库一并回滚(否则 GET 会看到重启后就消失的幻影流程)
            self.library = previous;
            return Err(e);
        }
        Ok(report)
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
