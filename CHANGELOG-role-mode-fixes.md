# 角色模式 bug 修复变更说明(2026-09-06)

> 注:本文档为 2026-09-06 的历史快照,其中测试计数(如 vitest 445/445)为当时数值;
> 最新计数以 MAINTENANCE.md 为准(2026-09-07 实测:cargo 805 / vitest 485)。

本次修复覆盖用户报告的 5 个 bug 及排查中发现的隐藏问题,另含实测阶段新发现的 3 个
阻断性 bug。全部修复均经真实应用(IAB 实测)或探针环境验证。

## 用户报告的 5 个 bug

### 1. 重进后角色聊天记录丢失
- **根因(三重叠加)**:① 前端不记住上次角色/会话,重进永远落在最新创建的角色;
  ② 双数据目录分叉:桌面版写 `%APPDATA%\com.kedai.app\data`,浏览器版写 `项目\data`,
  两个库各自写入,一边写的聊天另一边永远看不到;③ 加载失败只 console 不提示。
- **修复**:记住上次位置(localStorage 存最后角色 id + 每角色最后会话 id,启动恢复);
  数据目录统一 `%APPDATA%\com.kedai.app\data`,项目库数据经一次性合并工具并入
  (合并前双备份、dry-run 验证);`/api/health` 增加 `data_dir`;加载失败显示可见错误条。
- **实测**:wuwa 卡聊 2 轮 → 刷新页面 → 角色/会话/消息/token 统计原样恢复。

### 2. 提示词查看功能失效(三个入口)
- **根因**:`web/src/store.ts` 门面漏转发 `queueSettingsSave`,OptimizeModal 调用必抛
  TypeError 且无 UI 反馈;弹窗懒加载 chunk 失败无兜底。
- **修复**:门面补转发 + 新增「门面完整性」回归测试(遍历子 store 导出键断言门面全部
  暴露);`defineAsyncComponent` 弹窗加加载失败兜底(提示+重试)。
- **实测**:综合设置→Agent 设置→最终提示词预览 ✓;提示词管理(6 层顺序面板)✓;
  优化面板→提示词检查(分层预览 #0 runtime_prompt ≈702token / #1 custom_template
  ≈1210token)✓。

### 3. 吸血鬼卡下载资源后不跳游戏界面
此卡修复链条最长,实测中逐层剥出 **6 个串联故障**,任一存在都会卡死:

| # | 故障 | 根因与修复 |
|---|------|-----------|
| 3a | iframe reload 后停在空白页 | nonce 一次性消费后丢失。改为 window.name 持有(reload 存活)+ 每次文档加载都发 ready + 宿主常驻监听重新 boot |
| 3b | reload 后下载成果丢失 | Cache/localStorage shim 全在内存。新增 postMessage 持久化桥:Cache 体存 IndexedDB、storage 快照存 localStorage,boot 时随快照下发(重启 App 仍有效) |
| 3c | 「宿主不允许访问父页面样式,无法进入宽屏」alert | 作者页宽屏机制读 `window.parent.document` 往父文档注入撑满样式(为同源酒馆设计),沙箱 iframe 下必抛 SecurityError。模板用 `[Replaceable]` 赋值遮蔽 `window.parent` 为代理(document 指向自身,postMessage 转发真身) |
| 3d | 开场消息/立绘/数据丢失 | 作者页解密 96MB 资源包后经 `URL.createObjectURL` 生成 blob: URL 加载资源,资源帧 CSP 不含 `blob:` 全部拦截。CSP 的 img/font/media/connect 四处放行 blob: |
| 3e | 开场白「文本为空喵」 | ① 角色列表接口不带 data_raw(体积大),世界书从未下发——boot 时补详情接口兜底;② 世界书条目名在 `comment` 字段(酒馆内部格式),作者页读 `name`——TavernHelper shim 归一化 `name ?? comment` |
| 3f | 点「进入游戏」后定格卡死 | **rAF 停发**:沙箱 iframe(无 allow-same-origin)在站点隔离下是独立进程,宿主窗口遮挡/最小化或 WebView 面板非激活时 rAF 无限期停发(探针实测 23 秒才触发一次,「RAF NEVER FIRED in 4s!」警告);作者页下载进度/解密分片/初始化动画全挂 rAF。模板把 rAF 映射到 setTimeout(16ms≈60fps,遮挡下仅降频不停摆) |

- **实测**:标题页 → 宽屏 → 进入游戏 → 完整游戏界面:世界书开场白(多段叙事)、
  梨奈立绘、数据面板(疲劳 68/欲望 5/意志 98/理智 100、5月1日 04:30)、特质雷达图、
  顶栏导航提示全部渲染。

### 4. wuwa 卡只有文字没界面
- **根因(四层)**:① 渲染引导无可发现性(已修:含界面脚本但纯文本显示时顶部引导条,
  一键开渲染/授权 JS);② CSS 清洗剥掉 position/fixed/z-index 等(渲染保真放宽,
  仅限用户逐卡主动开启渲染的卡);③ jQuery 读操作全是假值桩(已真实化:宿主 RPC 镜像);
  ④ **楼层深度语义丢失(本次实测新发现)**:酒馆脚本 `minDepth/maxDepth` 字段在归一化时
  被丢弃,「删除远楼层开场标记」(minDepth=2,空替换串)对当前开场(depth 0)误生效,
  先把占位符删成空串,「鸣潮开场」渲染脚本(65KB 界面 HTML)永远匹配不到 → 空白气泡。
- **修复**:后端归一化保留 `min_depth/max_depth`;前端渲染管线
  (renderScopedScripts/renderScriptedHtml/stripHiddenPlaceholders)按消息楼层深度过滤
  脚本;消息列表传 depth(0 = 最新);流式消息恒 depth 0。
- **实测**:引导条出现 → 开 HTML 渲染 → 授权 JS → 主开场渲染出鸣潮角色创建界面;
  聊天回复底部 MVU 交互栏(状态/剧情/牵绊/行动)渲染。
- **注意**:重进会话后第一条(主开场)气泡空白是**酒馆原生语义**——作者设计旧楼层
  不再显示创建界面(minDepth=2 的本意),与酒馆行为一致,不是回归。

### 5. 聊天记录面板失效
- 失效观感来自 bug 1(会话里只剩开场白)。随 bug 1 修复,实测刷新后消息完整恢复 ✓。

## 排查中发现的隐藏 bug(一并修复)
- `api/client.ts`:tokenPromise 缓存 rejected promise,bootstrap 一次失败后全部 API
  永久失败 → 失败不缓存,下次重试。
- `characters.rs`:expect panic → 改 500。
- 脚本沙箱 localStorage/sessionStorage 共享同一内存 store → 拆独立。
- `resource.rs`:GBK 页面 from_utf8_lossy 乱码 → 按 charset 解码。
- 头像一律存 .png 导致 mime 不符 → 按真实字节嗅探。
- 渲染面板同款 kdLoaded/reload 脆弱 → 随 3a/3b 链路修复。
- 作者页 TavernHelper API 面补齐:getCharData/getVariables/insertOrAssignVariables/
  getCharWorldbookNames(同步)/getWorldbook/updateWorldbookWith/generate/generateRaw
  (经 postMessage RPC 桥到宿主,宿主调新端点 `POST /api/chat/generate-raw`)/
  getPreset/replacePreset/eventOn 等。
- 宿主宽屏按钮:资源卡片注入「宽屏/退出宽屏」切换(fixed 撑满视口,层叠阶梯
  `--z-resource-wide: 70`);宽屏切换时作者页收不到 resize(实测切换后舞台缩在左上角),
  模板轮询 innerWidth/innerHeight 补发 resize,双向切换均自动适配。

## 安全边界(未退步)
- 资源 iframe 仍 `sandbox="allow-scripts allow-modals"`,**无 allow-same-origin**;
  作者脚本运行在不透明来源,拿不到宿主 DOM/localStorage/API token。
- blob: 放行仅限资源帧文档自身创建的 URL(opaque origin 内),不放宽跨源边界。
- CSS/脚本放宽、JS 执行仅限用户逐卡主动开启渲染/授权的卡;授权按脚本内容版本绑定,
  脚本变化后自动失效。
- 数据合并前双备份;合并工具先 dry-run 打印统计再实写。

## 测试
- 前端 vitest **445/445**(新增:门面完整性、nonce 稳定、snake_case 键、清洗放宽、
  楼层深度过滤回归用例)。
- 后端 cargo test:模板断言(parent shim/rAF 兜底/resize 监护/TavernHelper 归一化)、
  深度字段归一化等全过。
- 探针环境(kdprobe/):probe5(parent 遮蔽 5/5)、probe6(模板链路 6/6)、
  probe2(全链路带 rAF shim)、probe7(无 rAF shim 复现冻结实锤)。

## 环境注意事项
- **IAB/内嵌 WebView 测试**:沙箱资源帧的 rAF 依赖宿主 WebView 可见性;窗口最小化或
  面板非激活时旧版会卡死。模板 rAF→setTimeout 兜底后该问题已消除,但浏览器自动化
  截图仍可能因 WebContents 忙而瞬时失败,重试即可。
- **构建顺序**:`include_dir!` 在编译期嵌入 web/dist,必须先 `vite build` 再
  `cargo build`,否则内嵌旧前端(本次实测踩过:服务端首页引用旧 bundle hash)。
  日常开发可用 `KEDAI_WEB_DIST=<项目>\web\dist` 让服务读磁盘最新 dist 免重编。
