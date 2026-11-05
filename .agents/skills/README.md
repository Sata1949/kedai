# Kedai 项目级技能

本目录是为这个仓库专门写的 agent 技能。它们不教通用方法论（那是 `~/.zcode/skills/` 那 257 个技能的职责），只装**这个仓库特有、不写下来就会返工**的东西。

## 为什么放在仓库里

`.agents/skills/` 是 ZCode 的**项目级**技能目录，随仓库走：

- 换机器、换会话、换 agent 都还在。
- 技能里写的坐标（文件路径、门禁语义、裁决记录）可以和代码一起被 review、一起演进。
- 与 `~/.zcode/skills/` 的同名技能冲突时，`.zcode/skills` 优先级更高——所以这些技能刻意都带 `kedai-` 前缀，不覆盖任何通用技能。

## 清单

| 技能 | 管什么 |
|---|---|
| `kedai-start` | 入口与分流。先读这个。 |
| `kedai-batch-dev` | 批次全流程：查证 → 失败测试 → 实现 → 验证 → 回填 → 提交 |
| `kedai-gate-runner` | 门禁 17 段的分段语义、档位、失败指纹对照 |
| `kedai-backend-core` | 后端分层 SSOT、prompt_kit、任务引擎守卫、新增工具 8 处登记 |
| `kedai-frontend-core` | 前端分层、四条门禁、SSE 刷新链路、样式 ratchet |
| `kedai-verify-real-model` | 真实模型实测（验收唯一基准）、隔离实例、探针、证据 |
| `kedai-doc-ledger` | 六类文档互斥判定（R1–R6）、ID 纪律、锚点体例 |
| `kedai-ui-design` | 通用设计技能接到本仓库约束上（既有裁决、性能禁区） |

## 设计原则

**只写文档里没有的，或不读就会踩的。** 凡是 `AGENTS.md` 和六类台账里已经写清楚的，这里只给指针，不复述——复述必然漂移，而漂移的文档比没有更糟。

**声明的事实必须能被验证。** 本目录所有路径引用都经过脚本核对；分层表与 `tools/arch-layers.json` 逐项比对过；技能里给的命令都实跑过。

**给判别能力，不给口号。** 例如门禁失败时最需要的是"这个报错形态真实原因是什么"，不是"请仔细检查"。

## 维护

改动本目录时：

1. 技能里引用的路径若被移动，同步更新——用下面的脚本自查。
2. 分层归属变了，先改 `tools/arch-layers.json`，再改 `kedai-backend-core` / `kedai-frontend-core` 的表。
3. 门禁阶段或档位语义变了，改 `kedai-gate-runner`。
4. 新增裁决（如某处"知情接受"）时，在 `docs/遗留.md` 登记后，视情况在 `kedai-ui-design` 那类技能里加一句指针。

路径自查脚本：

```bash
python -c "
import os,re,glob
root=os.getcwd()
exts={'.rs','.ts','.vue','.mjs','.json','.md','.ps1','.css','.toml','.yml','.sql'}
pat=re.compile(r'\`([A-Za-z0-9_\-][A-Za-z0-9_\-./]*\.[A-Za-z0-9]+)\`')
bad=[]
for p in glob.glob('.agents/skills/*/SKILL.md'):
    for m in pat.finditer(open(p,encoding='utf-8').read()):
        c=m.group(1)
        if os.path.splitext(c)[1].lower() not in exts or '*' in c: continue
        if not os.path.exists(c) and not os.path.exists(os.path.join('web/src',c)):
            bad.append((p,c))
print(bad or '全部存在')"
```

（简写形式如 `tools/check-arch.mjs` 的短名 `check-arch.mjs` 会报出来，属预期——按语境判断即可。）
