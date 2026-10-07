# Kedai 示范 Skill 与工具插件

本目录是 Kedai 的**示范模板**,演示两套可扩展体系的标准写法:

| 体系 | 文件位置 | 生效方式 | 用途 |
|---|---|---|---|
| Skill 库 | `examples/skills/*.json` | `POST /api/skills` 导入(SQLite,同名覆盖) | 给 agent 的提示词技能,agent 用 `read(type=skill)` 按名称/关键词读取 |
| 工具插件 | `examples/plugins/tools/*.json` | 复制到 `data/plugins/tools/` 后 `POST /api/plugins/tools/reload`(或 `POST /api/plugins/tools/upload` 上传) | agent 模式可调用的自定义工具(白名单脚本,不使用 eval) |

> `data/` 已被 `.gitignore` 忽略,因此模板放在 `examples/`,需要时复制过去即可。

## 一、示范 Skill:写作风格指南

文件:`examples/skills/写作风格指南.json`。

- **作用**:deep/agent 模式下,agent 续写/润色前调用 `read`(type=skill) 加载规范,约束文风。
- **示范点**:
  1. 字段结构:`{ name, description, content }` —— `name` 唯一且语义化,`description` 给 agent 判断何时读取,`content` 是正文;
  2. 正文推荐结构:适用范围 → 规则 → 正反示例 → 使用方式 → 元信息;
  3. `name` 与 `content` 里放足关键词(文风/风格/写作/小说/润色),便于 `read` 按 `keywords` 检索。

导入方式(服务运行中):

```powershell
curl.exe -X POST http://127.0.0.1:3001/api/skills `
  -H "Content-Type: application/json" `
  --data-binary "@examples/skills/写作风格指南.json"
```

验证:`curl.exe http://127.0.0.1:3001/api/skills`,删除:`DELETE /api/skills/{id}`(id 从列表取)。

## 二、示范工具插件

### 1. `score_eval.json` — 评分判定(简单)

- **作用**:给草稿/回答打分判定,返回 `{passed, grade, score}`,适合创作自检。
- **示范点**:`parameters` 用 OpenAI 兼容 JSON Schema;`script` 用 `result = <表达式>` 形式,演示**三元表达式 + 对象字面量返回**(对象自动 JSON 序列化)。

### 2. `clamp.json` — 数值限幅(进阶)

- **作用**:把数值约束在 `[min, max]` 区间,适合限制生成参数/章节字数不越界。
- **示范点**:演示**嵌套白名单函数调用** `Math.max(args.min, Math.min(args.max, args.value))`。

### 3. `tag_brief.json` — 标签导语(多级链与中间变量)

- **作用**:取用户名与标签首项、总数,拼成一句导语;演示 2026-10-07 求值器升级后的新能力。
- **示范点**:**多级属性链** `args.user.name`、**下标** `args.tags[0]`、`.length`、**中间变量**
  (`who = ...` / `first = ...` / `count = ...`,换行分隔)与**字符串拼接**(`"「" + first + "」..."`)。

启用方式(两种任选):

```powershell
# 方式 A:复制文件到运行时目录后热重载
Copy-Item examples/plugins/tools/*.json data/plugins/tools/
curl.exe -X POST http://127.0.0.1:3001/api/plugins/tools/reload

# 方式 B:直接上传(API 会写入 data/plugins/tools 并注册)
curl.exe -X POST http://127.0.0.1:3001/api/plugins/tools/upload -F "file=@examples/plugins/tools/score_eval.json"
```

验证:`curl.exe http://127.0.0.1:3001/api/plugins/tools` 的 `tools` 列表里出现 `score_eval` / `clamp` / `tag_brief`。

## 三、脚本语法边界(重要)

白名单求值器(2026-10-07 升级)支持以下能力;**语法形态错误在加载期**(上传/reload)即报,
取值期错误(未知成员/越界/类型不符)显式报错并给出定位信息:

| 能力 | 示例 | 说明 |
|---|---|---|
| 多级参数读取 | `args.user.name` / `args.tags[0]` / `args.tags.length` | 多级链与下标均可用;下标须为非负整数 |
| 中间变量与语句序列 | `who = args.user.name` … 末句 `return ...` / `result = ...` | 多条语句用换行或 `;` 分隔;**末句必须是 return / result =** |
| 字面量 | `"文本"` / `42` / `true` / `null` / `{...}` / `[...]` | 对象/数组字面量可嵌套,值须为下方「可用表达式」 |
| 算术与拼接 | `1 + 2 * 3` / `"n=" + args.n` / `"a" + "b"` | `+` 两侧均为数值 → 算术;否则字符串拼接(null 视作空串);`- * / %` 仍要求数值 |
| 比较 + 逻辑 + 三元 | `args.score >= args.pass && args.score > 0 ? "合格" : "待改进"` | 三元在表达式最外层可用;`&&` / `\|\|` 可组合 |
| 白名单函数 | `Math.max/min/floor/ceil/round`、`JSON.stringify/parse`、`String`、`Number` | 参数可为任意可用表达式 |
| 字符串方法 | `.toUpperCase()` / `.toLowerCase()` / `.includes(x)` / `.trim()` | 作用于字符串型值/变量 |

**缺失与报错语义(2026-10-07)**:

- `args.x` 直接属性缺失 → **空串**(兼容既有「参数可能缺省」写法;整条链短路);
- 嵌套对象缺键 / 下标越界 / 类型不符(如对数值取成员)→ **显式报错**,消息含可用键或长度提示;
- 脚本语法问题在**上传/reload 阶段**就会被拒,不再等模型调用才失败。

**常见陷阱**:

1. **整段脚本不要用引号包住**:`'你好,' + args.nickname` 若把整段写成引号包围的「一个字符串」,参数不展开、原样返回。
2. **条件语句/循环不存在**:`if (...) { ... }` 不可用——用三元 `条件 ? A : B` 表达分支。
3. **返回值**:字符串原样返回;对象/数组/数字自动 JSON 序列化,模型可直接解析。

> 在 `server-rs/src/plugins/mod.rs` 的 `#[cfg(test)] mod tests` 里可以直接加脚本用例跑 `cargo test tool_plugin_`,验证新插件脚本的真实行为。

## 四、写新模板的检查清单

- [ ] `name` 唯一、小写英文/下划线(插件);中文可读名(skill)
- [ ] `parameters` 每个属性写清 `type` 与 `description`(模型据此填参)
- [ ] `script` 只用上表「可用表达式」,避开三种陷阱
- [ ] 用 `cargo test tool_plugin_` 或运行时 reload + `GET /api/plugins/tools` 验证
- [ ] description 里给出 1 个调用示例,降低模型误用率
