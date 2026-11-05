---
name: kedai-verify-real-model
description: Kedai 的真实模型实测执行手册——本仓库自 2026-10-06 起的验收唯一基准。当批次要收口、要证明功能真的能跑、要判断「测试全绿能不能算过」、要起隔离实例做端到端验证、或实测报 429/401/额度问题时使用。含 provider 与模型、凭据的仓外存放规则、隔离实例的启动与 data_dir 核对、三种探针的用法、证据落盘体例、以及「确实不可测」的登记方式。触发词：真实模型、实测、收口验收、隔离实例、探针、e2e-probe、task-probe、证据、额度、429。
---

# 真实模型实测：本仓库的验收唯一基准

## 一、这条口径是什么

**mock / 桩只是回归网，不构成验收证据。**「测试全绿」只有在真实模型实测同样通过时才算数。单测与集成测试照常带 mock 隔离跑，它们的职责是回归保护。

纯文档 / 样式批次可以豁免到「至少跑一次最小连通冒烟」，但要在提交正文里说明。确实不可测时（离线、额度耗尽）**显式说明理由**，不许含糊过去。

## 二、固定的 provider 与模型

| 项 | 值 |
|---|---|
| 网关 | `https://api.commandcode.ai/provider/v1` |
| 模型 | `deepseek/deepseek-v4.1-flash` |

设置页可整段粘贴含 `/chat/completions` 的完整 URL，后端会自动剥后缀。

**这个模型带推理（`reasoning_tokens`），有一个必须知道的坑**：探针 `max_tokens` 给小了会出现「HTTP 200 但 `content` 为空」。所以——

- 连通性判据看**状态码与 `usage`**，不要只看 `content` 是否非空。
- 要拿可见回复时，`max_tokens` 抬到 **≥256**。

## 三、凭据：只能放仓外

本仓库是**公开仓**。密钥只存在两个地方：

- `D:\kedai-bench-run\realmodel.env`（本机环境变量文件）
- 各隔离实例 `DATA_DIR` 下的 `settings.json`

**禁止**把 key 写进仓库任何文件、提交正文或文档值。

**配置回退链有个陷阱**：目标 `DATA_DIR` 已有 `settings.json` 时，`.env` 里的 `OPENAI_*` **不生效**（机制见 `经验.md` E76）。所以换新 DATA_DIR 时必须显式经 `PUT /api/settings` 配连接，不能指望环境变量。

## 四、流程：隔离实例

```bash
MSYS_NO_PATHCONV=1 \
  DATA_DIR=D:/kedai-bench-run/data-<批次> \
  PORT=30xx \
  LOG_DIR=D:/kedai-bench-run/<批次>-logs \
  server-rs/target/debug/kedai-server.exe
```

三步纪律，顺序不能乱：

1. **启动**。用环境变量前缀形态，不要写 `.cmd` 里 `set` 后后台跑（那样不生效，见 `经验.md` E75）。
2. **核对 data_dir**。日志里找 `"data_dir":"..."` 那一行，**确认指向隔离目录后再做任何写操作**。这一步省了就会对着真实用户数据做实验。
3. **配连接**。`PUT /api/settings` 传 `base_url` / `api_key` / `model`。注意字段名是 **`openai_base_url` / `openai_api_key`** 这一组（`base_url`/`api_key` 是无效的旧名）。

端口选 30xx 段，避开 3001（用户可能常驻着便携版）。用完**必须杀进程**。

## 五、三种探针

```bash
# 角色扮演端到端（消费 SSE，记录 TTFT / 截断自愈 / 反思重生成 / token 用量）
node tools/bench/e2e-probe.mjs <mode> [maxTokens]
#   mode: fast | deep | agent | custom

# 任务模式端到端（创建 → 运行 → 订阅 SSE → 事件链 / 耗时 / token 增长）
node tools/bench/task-probe.mjs <task_mode> [maxWaitMs]
#   task_mode: legacy | solo | multi | plan | team | custom

# 最小连通：POST /api/chat/generate-raw
# 或直连上游：D:\kedai-bench-run\probe-provider.mjs
```

环境变量：`KEDAI_BASE`（默认 `http://127.0.0.1:3001`）、`KEDAI_PROBE_TOKEN`（不给则自动从 `/api/bootstrap` 领）、`KEDAI_PROBE_CHAR`。

**任务探针有两条必知的语义**（读源码确认的，别凭猜）：

- **任务目标就是 `title`。** `POST /api/tasks` 的 body 没有 `goal`/`input` 字段；`POST /api/tasks/{id}/run` 的 handler **不接 body**。把题面发在 `/run` 的 `{input}` 里会被静默忽略——曾经因此一直在跑时间戳标题，规划器看不到目标只能凭空编。
- **到点没进终态必须主动 stop**，否则留下继续烧 token 的孤儿任务。

## 六、证据落盘

命令与关键输出（JSON / 日志）一律落 `D:\kedai-bench-run\<批次>-evidence\`。

提交正文写三件事：**真实模型实测通过 + 证据路径 + 关键结论**。没有证据路径的"实测通过"在评审里不算数。

## 七、额度与不可测的判别

这个网关历史上出过几种不同的失败形态，**不要混为一谈**：

| 现象 | 含义 | 处置 |
|---|---|---|
| HTTP 200 但 `content` 为空 | `max_tokens` 太小，推理吃完了预算 | 抬到 ≥256，看 `usage` |
| 429 + 有重置时间 | 周额度耗尽 | 按「确实不可测」登记，写明重置时间 |
| 400 `insufficient credits` | 账号余额耗尽（**不是** 429，**没有**重置时间） | 同「不可测」，但理由写准确 |
| 401 | key 失效 | 换 key；官方号曾余额为负 |
| 503 `model_not_found` | 某个中转网关不认这个模型名 | 换回 commandcode 网关，不是配置错 |

**认清这几种的区别很重要**：把「额度耗尽」误判成「配置错误」会白折腾一轮配置；把「配置错误」误判成额度问题会漏掉真正该修的 bug。

## 八、UI 类批次怎么实测

纯后端逻辑用探针就够，但**界面改动必须真渲染取证**，因为探针看不见视觉。

- 不重编 exe 也能看新前端：`PORT` + 隔离 `DATA_DIR` + `KEDAI_WEB_DIST=web\dist` 三件套。
- 真实 exe 的 API 探针**要带 token**（`GET /api/bootstrap` 取 Bearer，loopback 限定）。
- 换端口 = 换 origin = localStorage 天然为空，首启类功能免手清。
- 截图超时是宿主问题，可退回 DOM 断言；必要时用确定性补证（固定页面 + 断言关键文本/尺寸）。
- **收工必杀进程**。

## 九、一次实测的自检清单

收口前对照：

- [ ] 用的是隔离 `DATA_DIR`，且核对过日志里的 `data_dir`
- [ ] 经 `PUT /api/settings` 配的连接（不是靠 `.env`）
- [ ] `max_tokens` ≥256，判据看了 `usage` 而不只是 `content`
- [ ] 任务跑到了终态，或已主动 stop
- [ ] 证据落在 `D:\kedai-bench-run\<批次>-evidence\`
- [ ] 提交正文写了证据路径与关键结论
- [ ] 相关进程已杀

相关技能：`kedai-batch-dev`（批次流程）、`kedai-gate-runner`（门禁）。
