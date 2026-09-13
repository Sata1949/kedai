# docs/ 索引

> 本目录分两类:**活文档**(架构/协议/契约/风险登记/在跟踪的计划,改代码时需同步)与
> **过程交接类**(计划、笔记、历史快照,完成后仅供溯源,不再更新)。
>
> **物理约定:过程交接类一律放 `docs/archive/`,`docs/` 根下只留活文档。**
> 新增、移动或归档文档后必须同步本索引;活文档中的 `文件:行号` 引文在改动后需复核。

## 活文档

### 架构与协议

| 文档 | 内容 |
|---|---|
| [ARCHITECTURE-3H.md](ARCHITECTURE-3H.md) | 「三结合」梯队架构总纲 |
| [mvu-protocol.md](mvu-protocol.md) | mvu 变量系统协议(UpdateVariable / MagVarUpdate 双格式) |
| [generate-render-protocol.md](generate-render-protocol.md) | 生成与渲染协议(双端一致性约束) |
| [inject-protocol.md](inject-protocol.md) | 提示词注入协议 |
| [模式提示词边界.md](模式提示词边界.md) | 角色扮演/任务双模式提示词的继承与隔离规则 |
| [授权模式.md](授权模式.md) | 三档授权模式(严格/宽松/放行)判定矩阵、任务工具策略与授权生命周期 |
| [任务引擎六模式.md](任务引擎六模式.md) | 六模式语义对照 |

### 变更契约(逐轮改动的对外契约;历史结论勿改)

| 文档 | 内容 |
|---|---|
| [任务模式重构-变更说明.md](任务模式重构-变更说明.md) | 任务模式重构的对外变更契约 |
| [任务模式实跑修复-变更说明.md](任务模式实跑修复-变更说明.md) | 2026-09-10 六模式实跑修复 F1~F8 的变更契约(含 F2 默认行为变更) |
| [实测八项修复-变更说明.md](实测八项修复-变更说明.md) | 2026-09-10 实测八项问题(任务对话呈现/team 收敛/呼吸灯条/agent 记录/状态栏/装饰条/首楼事件/关闭确认)的变更契约 |
| [实测三轮修复-变更说明.md](实测三轮修复-变更说明.md) | 2026-09-11 第三轮实测修复(赛马娘卡首楼交互 U1/U2、team 协作 T1~T7)的变更契约 |
| [实测四轮修复-变更说明.md](实测四轮修复-变更说明.md) | 2026-09-12 第四轮实测修复(世界书 depth 解析、CSS 注释吞声明、JS 授权提示、剪贴板失败语义 F1~F5)的变更契约 |
| [手机端设置面板窄屏适配-变更说明.md](手机端设置面板窄屏适配-变更说明.md) | 2026-09-12 Android 窄屏设置面板塌缩修复(CSS 网格/输入框宽度/下拉箭头的三档视口实测)变更契约 |
| [任务编排契约与plan模式加固-变更说明.md](任务编排契约与plan模式加固-变更说明.md) | 2026-09-12 任务编排工具契约与 plan 提示词加固(agentgo 部分派发、子任务截断不再假成功、read(type=subtask) 多键命中、todo 跨 agent 可见、agentend 可判)的变更契约 |
| [0.2.1-关于与提示词同源-变更说明.md](0.2.1-关于与提示词同源-变更说明.md) | 2026-09-12 0.2.1:关于/教程设置分区、任务 Agent 提示词丰富、Win 端提示词纳入代码默认(两端同源)、mock 工具白名单忠实化 |
| [角色扮演默认提示词内置-变更说明.md](角色扮演默认提示词内置-变更说明.md) | 2026-09-12 角色扮演默认提示词纳入代码默认(default_roleplay_agent_prompt + from_config/load 空值回填),修复 Android 首装提示词为空 |
| [命令执行层与Android三档执行器-变更说明.md](命令执行层与Android三档执行器-变更说明.md) | 2026-09-13 命令执行(bash 工具,四道闸+命令级硬门+审计)与 Android 三档执行器(ROOT/Shizuku/沙箱) |
| [架构收口五项-变更说明.md](架构收口五项-变更说明.md) | 2026-09-13 架构收口五项:事件 kind 类型化、版本单源、契约快照校验、执行路径收口(legacy/followup 上 ModeExecutor)、前端护栏与断环 |
| [0.3.0-beta-变更说明.md](0.3.0-beta-变更说明.md) | 2026-09-13 0.3.0-beta:版本号升级、「更新内容」重写、关于页排版修复、双端编译 |
| [0.3.0-beta-后续修复-变更说明.md](0.3.0-beta-后续修复-变更说明.md) | 2026-09-13 三项实测修复 + 批次 0–3 治理的逐批处置记录(§一–§十,风险登记见威胁模型文档) |
| [0.3.0-A-beta-变更说明.md](0.3.0-A-beta-变更说明.md) | 2026-09-13 0.3.0-A-beta(加固版):三项实测修复 + 批次 0-3 数据安全/性能/基础设施加固 + 批次 6 风险登记(含发布记录) |
| [抽象收敛与残余修复-变更说明.md](抽象收敛与残余修复-变更说明.md) | 2026-09-13 残余任务批次一:4.1 四份截断重试合一(utils::retry 纯函数)、3.7 用量统计事务化、3.1 消息删除的 message 作用域变量孤儿清理 |
| [前端结构债收敛-变更说明.md](前端结构债收敛-变更说明.md) | 2026-09-13 残余任务批次二(5.1+5.2):storeBridge 类型化注册(owner 必填 + 显式降级 + 14 例配对测试)、弹窗 MODALS 单点定义(App.vue/check-arch 白名单派生 + 9 例三方一致元测试) |
| [任务引擎拆分与工程文档补齐-变更说明.md](任务引擎拆分与工程文档补齐-变更说明.md) | 2026-09-13 残余任务批次三:4.2 TaskBackend 拆 8 个窄接口(算法上移 task_engine、锁纪律回收、中性 DTO)、6.5 EJS builtin 冻结门禁(规则 H)、3.2/6.2/6.3 三项评估与说明文档 |
| [前端工具链与审计升级-变更说明.md](前端工具链与审计升级-变更说明.md) | 2026-09-13 残余任务批次四:4.3 AgentEngine 依赖分组(18 参→4 结构体)、4.4 状态机迁移非静默(transition_best_effort + 8 处)、5.3 ESLint+Prettier、6.1 审计升级(双锁 cargo audit + npm audit 硬门禁),并收口前三批只读审查发现的 P1/P2 |

### 现状、风险与计划

| 文档 | 内容 |
|---|---|
| [威胁模型与残余风险.md](威胁模型与残余风险.md) | **安全/正确性残余风险登记**:T1–T5 安全边界、D1–D5 数据正确性、存而未用的配置、新增内置工具的 6 处登记清单。新增或处置风险时同步更新 |
| [代码签名与SmartScreen说明.md](代码签名与SmartScreen说明.md) | Windows 产物**未做 Authenticode 签名**的现状、SmartScreen「更多信息→仍要运行」指引、产物来路自查(构建指纹/哈希/APK 签名)与转正式签名的要点(残余计划 6.3) |
| [数据库版本与降级行为.md](数据库版本与降级行为.md) | 数据库 schema 无 `user_version` 的现状、旧库/降级/双端共用库/双库合并行为矩阵,以及引入 `PRAGMA user_version` 的最小方案与验收断言(残余计划 6.2 文档部分) |
| [构建指纹链评估-2026-09-13.md](构建指纹链评估-2026-09-13.md) | 构建指纹链(D2)两套算法并存的两案评估与推荐结论(维持现状 + 补文档;不选 build.rs 改 SHA256),附可选 `exe_sha256` 路径(残余计划 3.2) |
| [known-limitations.md](known-limitations.md) | 已知能力缺口清单(有意裁剪 vs 待办,防误判为 bug) |
| [perf-baseline.md](perf-baseline.md) | API 性能基线方法与记录 |
| [残余任务与后续计划-2026-09-13.md](残余任务与后续计划-2026-09-13.md) | **新会话接手指南 + 残余任务计划**(含环境命令/纪律/避坑):批次 4 抽象收敛、批次 5 前端、安全与数据残余、死配置、性能、工程化补充与建议执行顺序 |
| [优化实施方案-2026-09.md](优化实施方案-2026-09.md) | 架构详解 + 22 项优化方案 + 实施路线图(**执行中**:剩余项见残余任务与后续计划) |
| [android-port-plan.md](android-port-plan.md) | Android 移植方案:可行性评估、架构决策、分阶段计划、cfg 门控清单、风险表(**阶段 0/1/2/3/4 已完成,阶段 5 进行中**) |
| [平台分支说明.md](平台分支说明.md) | **分支模型单一事实来源**:主干 `main` + 平台分支 `kedai-Win` / `kedai-Android` 的拓扑、三条规则、平台文件归属映射与同步工作流 |
| [三结合彻底落实-实施计划.md](三结合彻底落实-实施计划.md) | 「三结合」分层从叙事落实为结构+护栏的实施计划(9 批次,含切点与验收断言)(**已完成**,证据见核对表) |
| [三结合落实核对表.md](三结合落实核对表.md) | 上述计划的任务 ID → 状态 → 验证命令 → 证据 `文件:行号` 跟踪表(**已完成**:45 项已验证;未做项 E.1 转 [known-limitations.md](known-limitations.md) L12) |

## 过程交接类(已归档到 docs/archive/,仅供溯源)

| 文档 | 说明 |
|---|---|
| [archive/CHANGELOG-role-mode-fixes.md](archive/CHANGELOG-role-mode-fixes.md) | 2026-09-06 角色模式 5+3 个 bug 的修复记录(历史快照,测试计数为当时数值) |
| [archive/TASK-MODE-FIX-PLAN.md](archive/TASK-MODE-FIX-PLAN.md) | 任务模式修复计划(「Kimi 续做」交接) |
| [archive/任务模式修复-变更说明.md](archive/任务模式修复-变更说明.md) | 上一轮任务模式修复的变更记录(被 [任务模式重构-变更说明.md](任务模式重构-变更说明.md) 取代) |
| [archive/任务模式重构-实施计划-v2.md](archive/任务模式重构-实施计划-v2.md) | 重构实施计划 v2 |
| [archive/plan2-variable-scopes.md](archive/plan2-variable-scopes.md) | 变量作用域批次计划 |
| [archive/plan4-slash-quick-replies.md](archive/plan4-slash-quick-replies.md) | slash 命令与快速回复批次计划 |
| [archive/plan5-audio-render-panels.md](archive/plan5-audio-render-panels.md) | 音频与渲染面板批次计划 |
| [archive/plan6-ecosystem-devtools.md](archive/plan6-ecosystem-devtools.md) | 生态与开发者工具批次计划 |
| [archive/learn-harness-2026-08.md](archive/learn-harness-2026-08.md) / [archive/learn-deepseek-harness.md](archive/learn-deepseek-harness.md) | harness 学习笔记 |
| [archive/kedai-agent-coordination.md](archive/kedai-agent-coordination.md) | 多 agent 协作约定 |
| [archive/context-optimization-brief.md](archive/context-optimization-brief.md) | 上下文优化简报(其中「1 秒轮询/不走 SSE」已被 WP4/WP5 取代) |
| [archive/contract-drift-2026-09-09.md](archive/contract-drift-2026-09-09.md) | 前后端契约漂移登记(#1-#12 全数修复,漂移检查已由 `tools/check-contract.mjs` 自动执行) |
