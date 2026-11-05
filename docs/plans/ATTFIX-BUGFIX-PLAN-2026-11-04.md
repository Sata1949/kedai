# ATTFIX 缺陷整合与修复方案

> 日期：2026-11-04；基线 `main` @ `13b3504`（TMSHOW-4 收口）。
> 来源：两轮独立排查合并——本会话（实证为主）+ 并行会话（广度为主）。
> 本文是**执行稿**：每条含落点、判据、修法、验证方式。落地后按仓库纪律进
> `docs/计划.md` 立条目、`docs/遗留.md` 登记残余、`docs/经验.md` 记教训。

## 零、这份文档解决什么

2026-11-04 对同一基线做了两轮独立缺陷排查，两轮发现互不重叠、各有独占项：

| | 本会话 | 并行会话 |
|---|---|---|
| 强项 | 真实测试实证、边界推演 | 覆盖面广（代码/文档/卫生/门禁） |
| 独占发现 | multipart 内容截断（P0）、上传批次非原子（P1） | 轨迹缺口永久丢失（P1）、单文件判据双口径、SSE 回放闲置 |
| 共同发现 | 附件段污染 goal_text、legacy 拼接粘连、`rows.flatten()` 吞错 | 同左 |

合计确认 **13 条真实缺陷** + **1 条口径冲突**。本文把它们整理成可执行的修复批次。

**两轮共同漏掉的反思**：都从「最近的提交」入手，都漏了「旧代码被新用途复用」这条线——
multipart 解析器是 2026 年早期代码，ATTACH-1 首次把它用于任意文件才暴露缺陷。
后续排查应固定加一条「本次改动复用了哪些旧原语，它们的隐含假设还成立吗」。

---

## 一、缺陷总表

| ID | 级别 | 位置 | 一句话 | 实证状态 |
|---|---|---|---|---|
| ATTFIX-1 | **P0** | `api/characters.rs:114` | multipart 匹配 `\r\n--` 缺 boundary，内容被静默截断 | **已实证**（28→13 字节） |
| ATTFIX-2 | P1 | `api/tasks/attachments.rs:60-79` | 批量上传非原子，400 却已落库前 N 个 | **已实证**（left=1/right=0） |
| ATTFIX-3 | P1 | `useTaskTrace.ts:184/190/219` | 缺口补拉只取一页 + 游标先推进 → 中段永久丢失 | **已复现**（188 条丢失） |
| ATTFIX-4 | P1 | `TaskAttachmentList.vue:30` | `length>0` 判缓存，删除最后一个附件后仍显示 | **已实证** |
| ATTFIX-5 | P1 | `mod.rs:390` | legacy 任务 followup 收到「无读取工具」的错误提示 | 已核实（followup 走 `run_agent_loop`） |
| ATTFIX-6 | P1 | `useTaskAttachments.ts:94` | `pendingFiles` 等三个 API 零 UI 消费，附件盲传 | 已核实（grep 为空） |
| ATTFIX-7 | P2 | `mod.rs:382/432` | 附件名污染 `goal_text`，误判单文件交付 | 已核实 |
| ATTFIX-8 | P2 | `legacy_attachments.rs:66` | 附件段与步骤目标粘成一行 | 已核实 |
| ATTFIX-9 | P2 | `attachments.rs:125-141` | 并发上传撞名，删一个误删另一个的文件 | 代码推演 |
| ATTFIX-10 | P2 | `attachments.rs:199` | `rows.flatten()` 静默丢坏行 | 已核实 |
| ATTFIX-11 | P2 | `useTaskAttachments.ts:149` | `uploadTo` 把待上传队列算进当前任务配额 | 已核实 |
| ATTFIX-12 | P2 | `useTaskAttachments.ts:203` | `forgetTask` 零调用，缓存永久驻留 | 已核实（grep 为空） |
| ATTFIX-13 | P3 | `attachments.rs:46` | 注释称「客户端上报 mime」，实际按扩展名猜 | 已核实 |
| ATTFIX-14 | P3 | `trace.rs:147` | `trace_by_seq` 用 `.ok()` 吞掉读池失败 | 两轮独立确认 |

另有已核实但不建议本批处理的项，列在 §五。

---

## 二、修复批次划分

按「同一模块 + 可一起验证」聚合，四个提交。**每批都可独立回归，不跨批依赖。**

### 提交 1 — 后端附件链正确性（ATTFIX-1 / 2 / 8 / 9 / 10）

这是最要紧的一批：ATTFIX-1 会静默损坏用户数据，ATTFIX-2 会造成重复入库。

#### ATTFIX-1 multipart 内容截断

**落点**：`server-rs/src/api/characters.rs:114`

```rust
// 现状
let Some(next_rel) = find_subsequence(rest, b"\r\n--") else { break; };
```

**问题**：只匹配 `\r\n--`，不含 boundary。内容里出现 `\r\n--` 就在此截断，
而写入的 `size` 也是截断后的长度，三层自洽，无人报错。

**修法**：

```rust
let terminator = format!("\r\n--{boundary}");
let Some(next_rel) = find_subsequence(rest, terminator.as_bytes()) else { break; };
```

**回归用例**（加进 `tests/task_attachments.rs`）：
- CRLF 换行的 Markdown（front matter `---`）→ 入库 size == 原始长度
- 内容含 `\r\n--` 但非 boundary 的文本 → 不截断
- 内容含 `\r\n--{boundary}` 字面量 → 按协议属非法输入，需明确行为（建议报 400）

**验证**：`cargo test --test task_attachments -j 8`。

#### ATTFIX-2 上传批次原子性

**落点**：`server-rs/src/api/tasks/attachments.rs:60-79` + `services/task_service/attachments.rs:140-170`

**问题**：循环内 `?` 提前返回，但已写入的行与文件不回滚。函数文档声称「整批失败」，
实现造出了它要避免的「部分成功」。

**修法**（推荐方案 A，改动最小）：

- **方案 A — 先校验后写入**：循环体只做校验（空、25MB、累计个数），把通过的
  `(name, mime, bytes)` 收集成 vec；全部通过后再统一 `store_attachment`。
  校验阶段任一失败直接返回 400，此时磁盘与库都未动。
- **方案 B — 回滚**：失败时按已写入的 `out` 逆序删库行 + 删文件。
  比 A 复杂且回滚本身可能失败，**不推荐**。

**注意**：方案 A 下「累计个数」要按整批算（现有代码在每次 `store_attachment` 内
单独查 `existing.len()`，批内不会累加，一次传 12 个也能全进——这是附带缺陷）。

**回归用例**：
- `[合法, 空]` → 400 且清单为空（现有实证用例）
- `[合法×10, 合法]` → 400 且清单为空（批内累计上限）
- `[合法, 合法]` → 201 且两条都在

**验证**：`cargo test --test task_attachments -j 8`。

#### ATTFIX-8 legacy 附件段粘连

**落点**：`server-rs/src/services/task_service/legacy_attachments.rs:66-72`

```rust
// 现状
format!("{}{step_goal}", suffix.trim_start())
// 修法
format!("{}\n\n{step_goal}", suffix.trim_start())
```

**回归用例**：带附件的 legacy 任务，步骤 user 消息里附件段尾与 `step_goal`
之间必须有两个换行。

**注意**：`legacy_summary_user_message` 用的是 `task.title + suffix` 拼接，
`attachment_suffix` 自带 `\n\n` 前缀，那边是对的——只改 `legacy_step_user_message`。

#### ATTFIX-9 并发上传撞名

**落点**：`services/task_service/attachments.rs:125-141`、`next_seq()` at `:340`

**问题**：`next_seq` 读目录取最大值 +1，两个并发请求算出同一 `seq`，
`std::fs::write` 后者覆盖前者，库里两行指向同一物理文件。

**修法**：`next_seq` 改为「占位循环」——用 `OpenOptions::new().write(true).create_new(true)`
尝试创建，`AlreadyExists` 则 `seq+1` 重试，拿到句柄后再写入。

```rust
fn reserve_seq(dir: &Path, safe: &str) -> Result<(u64, File), String> {
    let mut seq = next_seq(dir);
    for _ in 0..64 {
        let path = dir.join(format!("{seq}-{safe}"));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(f) => return Ok((seq, f)),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => { seq += 1; }
            Err(e) => return Err(format!("附件写入失败({}):{e}", path.display())),
        }
    }
    Err("附件序号分配失败(目录内文件过多)".into())
}
```

**回归用例**：并发起 8 个上传到同一任务，断言 8 行记录 8 个不同 `rel_path`、
磁盘 8 个文件。

#### ATTFIX-10 坏行静默跳过

**落点**：`services/task_service/attachments.rs:199`

```rust
// 现状
Ok(rows.flatten().collect())
// 修法
rows.collect::<Result<Vec<_>, _>>()
    .map_err(|e| format!("读取附件清单失败: {e}"))
```

与同仓 `scan_events` / `scan_trace` 的严格口径对齐（它们对坏行明确 `map_err`）。

---

### 提交 2 — 轨迹缺口补拉（ATTFIX-3）

**落点**：`web/src/composables/useTaskTrace.ts:180-197`（`pullTraceGap`）

**问题**：两个缺陷叠加——

1. **只拉一页**：`pullTraceGap` 没有 `loadTaskTrace` 那样的 `while` 分页循环。
2. **游标先推进**：`appendTrace` 在 `void pullTraceGap(...)` 之后同步执行
   `lastSeqByTask.set(taskId, trace.seq)`，把游标推到缺口之后；
   而 `pullTraceGap` 结束时又用 `merged` 末条（含那帧大 seq）覆盖游标。

**时序复现**（已用脚本验证）：已见 `seq=11` → 断线 → 收到 `seq=700` →
补拉只回 `12..511` → 游标落 `700` → `512..699` 共 188 条永久丢失，
后续 `seq=701` 相对 700 连续，缺口检测永不触发。

**修法**（两条都要，缺一不可）：

```ts
// 1) pullTraceGap 改成分页，与 loadTaskTrace 同构
async function pullTraceGap(taskId: string, from: number): Promise<void> {
  if (pullRunning.has(taskId)) return;
  pullRunning.add(taskId);
  try {
    let after = from;
    const collected: api.TaskTrace[] = [];
    for (;;) {
      const page = await api.getTaskTrace(taskId, after, TRACE_PAGE_LIMIT);
      if (ownerTaskId.value !== taskId) return;
      if (page.truncated) truncated.value = true;
      if (page.trace.length === 0) break;
      collected.push(...page.trace);
      const nextAfter = page.trace[page.trace.length - 1].seq;
      if (nextAfter <= after) break;              // 游标打转防御
      after = nextAfter;
      if (page.trace.length < TRACE_PAGE_LIMIT) break;
      if (collected.length >= TRACE_MAX_ROWS) break;
    }
    if (collected.length === 0) return;
    rows.value = flattenTrace([...rows.value, ...collected]);
    // 2) 游标只按"补拉覆盖到的最大连续 seq"推进，绝不跨过尚未拉回的区间
    lastSeqByTask.set(taskId, after);
  } catch (e) {
    console.warn('[kedai] 任务轨迹缺口补拉失败', e);
    // 失败时不改游标，下一次缺口/重连会重试
  } finally {
    pullRunning.delete(taskId);
  }
}
```

**关键点**：`lastSeqByTask` 的语义收紧为「**已连续覆盖**的最大 seq」，
`appendTrace` 里那行 `lastSeqByTask.set(taskId, trace.seq)` 要改成
「仅当 `trace.seq === last + 1` 时推进」，否则保持不动等补拉追平。

**回归用例**（`useTaskTrace.test.ts`）：
- 缺口 600 条 → 补拉两页全部拉回，`rows` 无空洞
- 补拉中途失败 → 游标不前移，下次缺口检测仍能命中
- 连续帧（无缺口）→ 不触发补拉

**验证**：`npm test -w web`（该文件）。

---

### 提交 3 — 前端附件体验（ATTFIX-4 / 6 / 11 / 12 / 13）

#### ATTFIX-4 删除后仍显示

**落点**：`web/src/components/task/TaskAttachmentList.vue:26-32`

```ts
// 现状：length>0 无法区分"已加载且为空"与"从未加载"
const cached = attachments.attachmentsOf(id);
if (cached.length > 0) return cached;
return currentTask.value?.attachments ?? [];

// 修法：用 has() 判"是否已有权威缓存"
const list = computed<TaskAttachment[]>(() => {
  const id = currentTaskId.value;
  if (!id) return [];
  if (attachments.hasLoaded(id)) return attachments.attachmentsOf(id);
  return currentTask.value?.attachments ?? [];
});
```

composable 侧补 `hasLoaded(taskId)`（内部就是 `listsByTask.value.has(taskId)`）。

**回归用例**：删除唯一附件后 `findAll('li')` 长度为 0
（**这条现在是红的，正是它暴露了缺陷**）。

#### ATTFIX-6 待上传队列无 UI

**落点**：`web/src/components/sidebar/TaskComposer.vue`

**问题**：`pendingFiles` / `removePending` / `clearPending` 三个 API 零 UI 消费，
用户未选中任务时选的附件完全不可见、不可移除。

**修法**：在 composer 输入框下方渲染待上传清单（文件名 + 移除按钮），
空态不占版面；`uploading` 期间禁用「创建并执行」或给出进度提示。

#### ATTFIX-11 配额误算

**落点**：`web/src/composables/useTaskAttachments.ts:149`

```ts
// 现状：pending 属于"还没创建的任务"，与当前任务配额无关
const existing = attachmentsOf(taskId).length + pending.value.length;
// 修法
const existing = attachmentsOf(taskId).length;
```

#### ATTFIX-12 forgetTask 零调用

**落点**：`web/src/stores/task.ts:1091`（`deleteTask`）与 `deleted` 事件分支

**修法**：删除任务后清该任务的附件缓存。store 不宜直接依赖 composable，
按既有 `uiPrefsBridge` 同形态加一个桥回调。

#### ATTFIX-13 mime 口径冲突

二选一，**推荐 (a)**：

- **(a) 修注释**：把 `attachments.rs:46` 改成「按扩展名推断的 MIME；未知为 `null`」。
  实现不动——扩展名推断对「文本 vs 图像」这一条判定够用，且解析器不支持
  part 级 `Content-Type`，改实现要动 `parse_multipart` 的返回结构，收益不匹配。
- (b) 修实现：让 `MultipartPart` 带 `content_type`，`store_attachment` 用它。
  改动面大（解析器是所有上传端点的公共实现），本批不做。

---

### 提交 4 — followup 能力位（ATTFIX-5）

**落点**：`server-rs/src/services/task_engine/mod.rs:390`

**问题**：能力位 `tools_available` 从 `task.task_mode` 推断，
但 followup 由 `FollowupExecutor` 执行、走 `run_agent_loop`（**有完整工具集**）。
legacy 任务续跑时模型会被告知「本模式没有文件读取工具」，而它其实能读——
这正是 ATTACH-1-L2 刚修掉的「提示词在说谎」，换了个入口重现。

**修法**：能力位不从 `task_mode` 推，由派发点在调用 `run_inner` 时按**实际执行器**
传入。签名加一个 `executor_has_tools: bool` 参数：

| 入口 | 值 |
|---|---|
| `run_mode(Legacy)` | `false` |
| `run_mode(Solo/Multi/Plan/Team/Custom)` | `true` |
| `run_approved`（plan 批准）= | `true` |
| `run_followup` = | `true` |

**回归用例**：legacy 任务 done 后追加指令，user 消息里**不得**出现
「本模式没有文件读取工具」。

---

## 三、验证矩阵

按批次各自的最小验证，**不跑全量**（全量留到收口）：

| 提交 | 验证命令 | 通过判据 |
|---|---|---|
| 1 | `cargo test --test task_attachments -j 8` | 新增 6 例 + 既有 6 例全绿 |
| 2 | `npm test -w web -- useTaskTrace` | 新增 3 例全绿 |
| 3 | `npm test -w web -- TaskAttachmentList useTaskAttachments` | 新增用例全绿，删除用例转绿 |
| 4 | `cargo test --test task_attachment_injection -j 8` | 新增 followup 用例全绿 |

**收口时**（四批全落）才跑：
```
cargo test --manifest-path server-rs/Cargo.toml -j 8
npm test -w web
node tools/count-tests.mjs --check    # 数字必须回填
npm run check                          # 全量门禁
```

**真实模型实测**：ATTFIX-1/2 属数据正确性，建议用一个 CRLF Markdown 附件 +
一个含 `\r\n--` 的文本附件跑一次真实模型任务，确认模型读到完整内容。
按 AGENTS.md「确实不可测」条款，若额度不可用需在提交正文登记。

---

## 四、文档回填清单

按 `kedai-doc-ledger` 的六类互斥判定：

| 文档 | 写什么 |
|---|---|
| `docs/计划.md` | 新增 **ATTFIX** 批次条目（ID 前缀先到先得，需先 grep 确认未被占用） |
| `docs/功能-变更史.md` | 新增一章：四提交的实际改动与验证 |
| `docs/经验.md` | 新增 **E149**：旧原语被新用途复用时，必须重审其隐含假设（multipart 解析器的教训） |
| `docs/遗留.md` | 登记本批**不做**的项（见 §五） |
| `MAINTENANCE.md` | 测试计数更新 |

---

## 五、本批明确不做（转遗留）

以下已核实但**不建议**塞进本批——混批是本仓库最常见的返工原因。

| 项 | 现状 | 为何不做 |
|---|---|---|
| `attachments.rs:231` 删除时 `mkdir -p` | 目录已清理时会重建 `.kedai/attachments/` | 副作用轻微（空目录），且与 ATTFIX-9 的文件句柄改造有耦合，另批处理 |
| `single_file_requested` 否定窗口仅 8 字符 | 远距离否定会误判 | 属启发式精度问题，需先定口径（产品判断），不是纯 bug |
| `sink.rs:63` 思考缓冲无上限 | 内存浪费，功能无错 | 属性能优化 |
| `api/tasks/trace.rs:20` `after` 收 `i64` | 负值误报 truncated | 前端不会传负值，属防御性加固 |
| SSE 订阅不带 `task_id`/`after`，服务端回放成死路径 | 设计取舍 | 需产品判断是否启用回放，不是缺陷 |
| `TaskComposer` 上传失败仍起跑 | 附件缺失时任务照样执行 | 需产品定「阻断 vs 提醒」，建议提醒+二次确认 |
| `SidebarFooter` 错误合并显示、切模式不清 | 小瑕疵 | 归 UI 收尾批 |
| `TaskTranscript.vue:278` observer 归属校验 | `await` 后无 taskId 校验 | 归前端重构批 |
| `TaskTraceItem.vue:35` `open` 不跟随 | 轨迹补全后展开态不更新 | 归前端重构批 |

---

## 六、子代理成本控制（本轮附带教训）

本轮两轮排查合计派出 **6 个子代理**，单个最大 `model-io` 记录 **1029 MB**，
额度消耗异常。已在 `~/.zcode/agents/*.md` 为全部 5 个定义加上硬约束：

1. **禁止再派发子代理**（层层嵌套是成本爆炸主因）
2. 窄查询读文件，不通读大文件
3. 只跑最小测试目标，不跑全量
4. 明确收敛时限（15~30 分钟）
5. 输出上限 2000~4000 字，发现条数上限 15

**主智能体侧同步收紧**（写进本仓库 `.agents/skills/` 的约定）：
- 单批次排查**最多派 2 个子代理**（一个后端、一个前端），不按维度切四五个
- 派发前先自己做一轮 Grep 定位，把范围缩到具体文件再派
- 简单查找（单文件、单符号）自己查，不派
