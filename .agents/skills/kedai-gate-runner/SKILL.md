---
name: kedai-gate-runner
description: Kedai 门禁（npm run check / tools/check-all.ps1）的分段解读与失败判别手册。当门禁转红、需要判断「这是代码回归还是环境问题」、需要挑一个更快的开关档位、或要在推送前留证时使用。含 17 段的报错指纹、两个 Cargo.lock 的审计、-Quick/-StaticOnly/-LooseAudit 等档位的准确语义。触发词：门禁、check-all、npm run check、CI 红了、检查失败、fmt、clippy、cargo audit、ratchet、基线。
---

# Kedai 门禁：分段、档位与失败判别

门禁脚本是 `tools/check-all.ps1`，入口 `npm run check`。**本技能不重复它的阶段清单**——唯一展开处是 `MAINTENANCE.md` §2 的「全量检查」那一行，由 `check-doc-claims` 机器守护。这里讲的是**怎么读它的失败**。

## 一、先看顺序：越便宜的越先跑

2026-10-30 提效批把顺序改成了「按代价排序」。这个顺序本身就是诊断线索——**如果它跑到第 4 段就死了，说明前 17 段（全部秒级静态检查）都过了**，问题只可能在编译或测试。

| 段 | 内容 | 典型耗时 | 红了的含义 |
|---|---|---|---|
| 1 | docs×3 + 11 段纯 Node 静态检查 | < 3s | 口径漂移（文档、契约、分层、ratchet 基线） |
| 2 | 后端环境就绪（vcvars64 导入） | 秒级 | 本机 MSVC 环境问题，不是代码问题 |
| 3 | fmt + clippy | ~4s（缓存命中） | 格式或 lint |
| 4 | cargo test 全量 | 最长，可达 20+ 分钟 | 真实回归 |
| 5 | 前端 eslint / vue-tsc / vitest / vite build / 体积预算 | 数分钟 | 类型、测试或体积 |
| 审计 | cargo audit（双锁）+ npm audit | 秒级～分钟 | 依赖漏洞或 advisory DB 不可用 |

**关键推论**：跑到第 4 段才红 = 前面全部通过，不必回头怀疑文档或基线。

## 二、档位语义（别用错）

| 档位 | 实际行为 | 什么时候用 |
|---|---|---|
| 默认 | 全量 | 提交前、交付前 |
| `-Quick` | 跳 vite build 与 bundle budget | pre-push（配 `-Bundle`） |
| `-Quick -Bundle` | 上面的基础上**仍然**跑 vite build + 体积预算 | pre-push 的推荐形态，保住体积退化的防线 |
| `-SkipRust` | 跳后端编译与测试 | 只改前端时 |
| `-SkipWeb` | 跳前端四段，**并连带跳过那 11 段静态检查** | 少用；它制造的「全绿」是假信号 |
| `-StaticOnly` | 只跑文档三段 + 11 段静态 + 审计两段，跳过一切编译与构建 | CI 的 main 轻量档；「跳过编译但保留口径」 |
| `-AuditOnly` | 只跑两段审计 | 单独复核依赖 |
| `-LooseAudit` | cargo audit 降为警告档 | advisory DB 拉不到时 |

**`-SkipWeb` 与 `-StaticOnly` 的关键差别**：前者会连带跳过静态检查（历史上它们与前端归在同一块），后者不会。想要「不动编译但保住口径」必须用 `-StaticOnly`。

**GATE-1 教训**：文档三段与审计段**不受** `-SkipWeb` / `-SkipRust` 影响，只被 `-AuditOnly` 跳过。所以「我跳了前端所以文档不用管」是错的。

## 三、失败指纹对照表

这是本技能最值钱的部分。这些形态都真实发生过多次：

| 报错形态 | 真实原因 | 处置 |
|---|---|---|
| `failed to remove file ... os error 5` / `LNK1104` | 有别的 cargo 在跑，或开发中的 server 锁住了 target 产物 | 停掉那个进程；`check-all` 内部已做「让位改名」助手，通常重跑即可 |
| 「依赖 rlib 缺失」/ `E0463` / `STATUS_STACK_BUFFER_OVERRUN` | rustc 并行崩溃，**不是**依赖问题 | 降并发复跑（`-j 4`），**不要改 Cargo.toml** |
| 编译脚本报 `os error 5` 且 `build-script-build` 消失 | 360 安全卫士拦截新建 exe | 见经验条目；快照回填同名 exe 可绕过 |
| 门禁全红，但单独跑那一段全绿 | ① 日志来自修复前的旧构建 ② `settings_connector` 顺序竞争（TEST-ISO-1，进程级共享 app） | 先确认日志时间戳与构建时间；隔离复跑该测试 |
| 某帧缺失类断言偶发红（约 1/3） | SSE broadcast `Lagged` 允许丢帧 | 断言改成「SSE ∪ REST 补拉」的并集，不要求某帧必现 |
| `cargo audit` 报 advisory DB 拉取失败 | 走 Gitee 镜像，凭据/网络问题 | `-LooseAudit` 跑完余段，或 `--no-fetch` 离线复核两锁 |
| CI 单点失败、本地隔离绿 | 并行下的 flaky（如 `agents::state_machine::tests::test_best_effort_invalid_transition_emits_warn_log`，tracing 线程局部） | `gh run rerun --failed` |
| 门禁第一段就红然后全跳 | fmt 失败 | **先 `cargo fmt`**，这是最常见的整轮作废原因 |
| 文档段红：某个计数对不上 | 由代码派生的计数与文档字面不一致 | 用 `count-tests.mjs` 现取真值，别手改数字猜 |
| `check-arch` 报规则 I | 新增/改名/删除顶层模块但没同步 `tools/arch-layers.json` | 同步该文件；**不是**加豁免 |
| `check-arch` 报规则 J | 出现未登记的跨代依赖 | 要么改依赖方向，要么在 `registeredEdges` 登记（那条债务会进基线） |
| `check-file-size` / `css-discipline` / type-ratchet 红 | 越过只降不升的基线 | 拆分或重构；**确需上调必须在提交正文说明理由并同步改脚本** |
| `check-settings-parity` 红 | 加了设置字段但没登记 | 补 `VALUE_RULES` 登记表 + 三组对拍 |
| `check-test-ratchet` 红 | 新增源码文件没有测试 import | 补测试，或在显式清单里登记（只减不增） |

## 四、本机特有的两条

**cargo 必须先有 MSVC 环境。** 裸 shell 里没有，报 `link.exe not found`。Git Bash 的可靠形态（注意 `MSYS_NO_PATHCONV=1` 与 `cmd //c` 的配合——两者一起用出问题时是「假成功」，见经验 E86）：

```bash
MSYS_NO_PATHCONV=1 cmd //c "call \"C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat\" && cd /d D:\kedai\server-rs && cargo test -j 8"
```

**推送前留证的可靠路径**：`pre-push` 钩子在本机会被 SIGPIPE 杀掉（141），门禁过了但远端 ref 没创建。所以：

```bash
powershell -NoProfile -ExecutionPolicy Bypass -File tools/check-all.ps1 -Quick -Bundle
git push --no-verify
git ls-remote origin <branch>      # 验收 SHA,别读推送日志
```

**别用管道读门禁退出码**：`node tools/check-arch.mjs | tail` 拿到的是 `tail` 的退出码。要么直接跑，要么 `set -o pipefail`。

## 五、什么时候别信门禁

- **本机全量 check 会被 Clash 的 fake-ip 环境红截停**那类网络问题：用 `-SkipRust -LooseAudit` 补完余段，并在提交正文说明。
- **门禁绿 ≠ 功能对**。2026-10-06 起本仓库的收口基准是**真实模型实测**，mock 测试只是回归网。见 `kedai-verify-real-model`。
- **`check-docs` 的锚点校验是「尽量校验」**，覆盖不到时不报错。改了章节标题后要人核一次跨文档引用。

相关技能：`kedai-batch-dev`（全流程）、`kedai-verify-real-model`（收口实测）。
