# 任务模式质量对照（TQUAL-21）

这是开发评测，不是产品的通用交付门槛。五类合成题分别覆盖 UI 交互和持久化、既有 Git 仓库修改、多文件浏览器与图形规格、真实 HTTP API、长任务的跨文件约束。用同一题面、种子文件、Solo 模式、连接、授权档和资源上限比较两个后端版本；每轮交换同题的调度顺序，按题配对，最多两轮 20 个任务。模型不接触评分代码和独立期望值。

## 运行

需要 Node.js 22+、Git、Chromium/Edge，以及由待比较版本分别构建的 Kedai 服务端二进制。比较后端时可以嵌入同一份前端静态资源；评测通过 REST 调任务，不依赖前端产品页面。
真实对照需要平台可用的凭据存储；评测不会替服务开启明文存储。首轮正式对照使用 Windows/DPAPI。

1. 在仓库外准备配置和一个证据父目录。配置结构如下，替换二进制路径、Git SHA、文件 SHA-256 和两个空闲的本机端口。

   ```json
   {
     "rounds": 2,
     "browser": "/absolute/path/to/chromium",
     "arms": {
       "baseline": { "revision": "<40 位 Git SHA>", "sha256": "<64 位文件 SHA-256>", "binary": "/absolute/path/to/baseline", "port": 3095 },
       "candidate": { "revision": "<40 位 Git SHA>", "sha256": "<64 位文件 SHA-256>", "binary": "/absolute/path/to/candidate", "port": 3096 }
     }
   }
   ```

2. 先跑预检。验证二进制、Chromium、两个实例的 `DATA_DIR`、实际设置和任务 API；只用桩，不计为真实模型验收。

   ```text
   node tools/bench/task-quality.mjs preflight <config.json> <仓外新目录>
   ```

3. 获得真实模型额度授权后，只在本次进程的仓外环境设置 `KEDAI_BENCH_KEY`，运行正式对照。每对任务在各自实例并行，逐对推进；两组都以工作区档执行，并显式拒绝待确认的越界调用，不授予新权限。额度、限流或认证错误后不再开始新任务。保持其他重负载任务停用，减少耗时干扰。

   ```text
   node tools/bench/task-quality.mjs run <config.json> <另一个仓外新目录>
   ```

   `plan.json` 固定版本、二进制、题面和评分器指纹；`runs.json` 与 `automatic.json` 留存结果。任务与验收代码分目录；子进程不继承凭据环境变量，产物 Node 进程限制文件权限，浏览器仅从本机静态根加载。只用于受控合成题，不把本机工作区当成可安全运行任意第三方代码的 OS 沙箱。证据目录中的实例数据、日志和生成代码保持私有，**不要整目录提交或上传**。

   如果已写出部分结果，排除阻塞后可续跑。续跑核对原计划的版本、题面、模型和预算，独立复验已有的成对产物，只请求余下任务；旧目录原样保留。

   ```text
   node tools/bench/task-quality.mjs resume <config.json> <原对照目录> <仓外新目录>
   ```

   修正评分器后，可对完整对照运行 `rescore <config.json> <原对照目录> <仓外新目录>`。它只复验原产物，不启动 Kedai 服务或调用模型，盲评素材与映射保持不变。

4. 把 **`review/`** 单独交给评审者。索引只含题面、随机 ID、代码和截图；不要一起提供 `review-key.json`、自动判定或任务终态。按索引的 0–3 分锚点记录 `[{ "id": "...", "code": 2, "visual": null, "note": "理由" }]`。先冻结评分文件，再解盲汇总：

   ```text
   node tools/bench/task-quality.mjs report <正式对照目录> <盲评分.json>
   ```

## 判据

- **独立通过率**：所有硬断言通过才算通过，不读取模型自评。硬要求覆盖率按通过的断言 / 已评分断言计；误报完成率按「终态 done 但独立验收失败」/ 已验的 done 计。缺样、环境不可用、未形成有效配对单列，不能记为通过或失败来改变分母。
- **图形专项**：只执行题面声明的材质、面剔除、AO、纹理差异与过滤判据。独立浏览器取实际画布像素和翻面后的帧；没有指定时，双面、透明混合、没有 AO 或线性过滤都不是错误。不搜索 `DoubleSide`、`vertexColors`、纹理种子等关键词判质量。原 Minecraft 产物保留为仓外历史夹具；回归用合成样本逐项重现缺陷，避免把历史产物硬套到新校准坐标上。
- **盲评与成本**：视觉/代码为独立的有序评分，报告样本数和均值；资源成本为服务记录的输入、输出、推理 token 和墙钟耗时，无账单就不折算金额。并行运行和网关缓存可能影响耗时；两轮只能暴露回归或给出趋势，不能据此推断总体收益。

无模型自测：

```text
node --test tools/bench/task-quality/quality.test.mjs tools/bench/task-quality/acceptance.test.mjs
```

设 `KEDAI_HEADLESS_BROWSER` 为本机 Chromium/Edge 绝对路径后，同一命令也会运行实际鼠标、输入、刷新、截图和像素的两项浏览器回归。
