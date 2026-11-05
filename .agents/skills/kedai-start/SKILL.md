---
name: kedai-start
description: Kedai 项目工作入口。任何在本仓库（D:\kedai）的实质性工作——改代码、修缺陷、加功能、写测试、跑验证、写文档、评审设计——都先读本技能，它会指出该加载哪个专项技能。触发词：Kedai、这个项目、本仓库、继续上次的活、接手、看看这个问题。
---

# Kedai 工作入口

本仓库有 7 个专项技能。先花 30 秒读这里，别急着动手——**加载错技能比不加载更费时间**。

## 按任务分流

| 你要做的事 | 加载 | 为什么 |
|---|---|---|
| 改功能 / 修 bug / 提交 | `kedai-batch-dev` | 批次顺序：查证 → 失败测试 → 实现 → 验证 → 回填 → 提交 |
| 门禁红了 / 要跑检查 / 要推送 | `kedai-gate-runner` | 17 段的报错指纹与档位语义，避免误判成环境问题 |
| 动后端 `server-rs/` | `kedai-backend-core` | 三结合分层、prompt_kit、任务引擎守卫、新增工具 8 处登记 |
| 动前端 `web/` | `kedai-frontend-core` | 前端分层、四条门禁、SSE 刷新链路、样式 ratchet |
| 要收口 / 证明能跑 | `kedai-verify-real-model` | 真实模型实测是唯一验收基准，mock 不算 |
| 写文档 / 回填台账 | `kedai-doc-ledger` | 六类互斥判定（R1–R6）、ID 纪律、锚点体例 |
| 改界面观感 / 做 UI | `kedai-ui-design` | 通用审美要服从既有裁决与性能禁区 |

**典型组合**：改一个功能并收口 = `kedai-batch-dev` → `kedai-backend-core`（或 `kedai-frontend-core`）→ `kedai-gate-runner` → `kedai-verify-real-model` → `kedai-doc-ledger`。

## 三条压倒一切的约束

在加载任何专项技能之前，先记住这三条——它们来自 `AGENTS.md`，覆盖一切默认行为：

1. **`AGENTS.md` 优先于任何默认行为**。它就在仓库根，与本目录同级。
2. **不顺手实现未要求的东西**。混批是本仓库最常见的返工原因。
3. **不泄露聊天正文、API Key 或本地真实数据**。本仓库是公开仓。

## 常用入口

```bash
npm run check          # 全量门禁(先用 kedai-gate-runner 读懂它)
npm test -w web        # 前端测试
npm run watch          # 日常增量构建
.\build.ps1            # 升版/交付前全量构建(不是日常用的)
```

后端 cargo 必须先有 MSVC 环境，Git Bash 里的正确写法见 `kedai-gate-runner`。

## 交接提示

- 台账在 `docs/`：契约 / 功能 / 计划 / 展望 / 经验 / 遗留。**先读再改**。
- 分层 SSOT 是 `tools/arch-layers.json`，不是文档叙述。
- 测试数字以 `MAINTENANCE.md` 为唯一真值源，用 `node tools/count-tests.mjs` 现取——**不要背数字**。
