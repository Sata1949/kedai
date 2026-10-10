#!/usr/bin/env node
// check-tutorial.mjs — 新手教程内容与真实界面的**防漂移机检**(TUTSYNC OB-12,2026-10-10)。
//
// 为什么要有这个脚本:2026-09-28 首启引导落地后,界面被 IMPHUB / RPFLOW / PLGM / 视觉文学编码
// 三个能力包等批次反复改名与删入口,而**全仓没有任何一条门禁**校验教程内容——2026-10-10 实测
// 教程仍在向用户承诺「预设导入」(IMPHUB 步骤 C 已删),63 例教程测试全绿。
// 既有测试只锁结构不变量与硬编码步数,读不出「界面名变了」这一类散文漂移,故补本脚本。
//
// 设计取向(与 `tools/check-arch.mjs` 解析 modals.ts 的做法同源):
//   - 文案与步骤表**不靠正则**读:它们是纯 TS 数据,直接 `import`(Node 24 原生类型剥离);
//   - 综合设置的导航表在 SFC `<script setup>` 里,导不出来 → 按**行契约**解析(见 SettingsHub.vue
//     的 domains 注释)。二级项必须写成单独一行 `{ type: 'section', key: '…', label: '…' }` /
//     `{ type: 'action', key: '…', label: '…', run: … }`,功能域头必须写成
//     `key: '…', label: '…', dot: '…',`——改格式先改本脚本。
//   - **只做能被机械判定的方向**:界面名是否存在、落点是否可达、步骤是否有文案。
//     不去解析散文语义(那必然脆,也必然误报挡住后续批次)。
//
// 用法: node tools/check-tutorial.mjs [--verbose]
// 退出: 任一断言失配 → exit 1,输出「哪一条 + 现值 + 期望 + 该改哪一侧」。
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const WEB = path.join(ROOT, 'web', 'src');
const VERBOSE = process.argv.includes('--verbose');

const fails = [];
const infos = [];
const fail = (rule, msg) => fails.push(`[${rule}] ${msg}`);
const info = (msg) => infos.push(msg);

const read = (rel) => fs.readFileSync(path.join(WEB, rel), 'utf8');

// ===== 1. 真实界面名词表:教程引用的每个界面名,必须在指定源文件里找得到该字面 =====
// 改名/删按钮 = 这里红。`file` 相对 web/src,`term` 是屏幕上真实的字(逐字)。
// 短词或有歧义(同名字串也出现在 tooltip / 注释里)时给 `re`(按 m 标志匹配源文件),
// 把断言钉在「它真的是一个渲染出来的标签」上——`re` 存在时 `term` 只用于报错文案。
const UI_NOUNS = [
  // 综合设置两级导航(SettingsHub.vue 的 domains)
  { term: '综合设置', file: 'components/SettingsHub.vue' },
  { term: '综合导入 / 导出', file: 'components/SettingsHub.vue' },
  { term: '附件管理', file: 'components/SettingsHub.vue' },
  { term: '提示词注入', file: 'components/SettingsHub.vue' },
  { term: 'Agent 设置', file: 'components/SettingsHub.vue' },
  { term: '连接配置', file: 'components/SettingsHub.vue' },
  { term: '执行流程', file: 'components/SettingsHub.vue' },
  { term: '授权与命令执行', file: 'components/SettingsHub.vue' },
  { term: 'MCP 服务', file: 'components/SettingsHub.vue' },
  { term: '编码能力包', file: 'components/SettingsHub.vue' },
  { term: '文学能力包', file: 'components/SettingsHub.vue' },
  { term: '视觉与截图', file: 'components/SettingsHub.vue' },
  { term: '数据管理', file: 'components/SettingsHub.vue' },
  { term: '向量化模型', file: 'components/SettingsHub.vue' },
  { term: '记忆库', file: 'components/SettingsHub.vue' },
  { term: '世界书', file: 'components/SettingsHub.vue' },
  { term: '关于与教程', file: 'components/SettingsHub.vue' },
  // 聊天顶栏
  { term: '开场', file: 'components/ChatTopbar.vue', re: '^\\s*开场\\s*$' },
  { term: 'HTML', file: 'components/ChatTopbar.vue' },
  // 「JS 已授权」/「JS 禁用」是模板里 `JS {{ authorized ? '已授权' : '禁用' }}` 的两态拼接,
  // 连续字面在源码里不存在,故登记两个片段(按钮整颗被删/改名时这里仍会红)。
  { term: '已授权', file: 'components/ChatTopbar.vue' },
  { term: '禁用', file: 'components/ChatTopbar.vue' },
  { term: '安全 HTML 渲染', file: 'components/ChatTopbar.vue' },
  { term: 'AGENT', file: 'components/ChatTopbar.vue' },
  // Agent 合并面板的两块页签:源码标签是 `Agent 状态`(CSS 把页签整体 uppercase,用户看到的是
  // 「AGENT 状态」)。名词表锚的是**源码字面**——改名/删页签时这里会红,大小写差异不影响判定。
  { term: 'Agent 状态', file: 'components/AgentPanel.vue' },
  { term: '调用情况', file: 'components/AgentPanel.vue' },
  // 侧栏与移动端导航
  { term: '导入角色卡', file: 'components/Sidebar.vue' },
  { term: '搜索角色', file: 'components/Sidebar.vue' },
  { term: '下达目标', file: 'components/Sidebar.vue' },
  { term: '角色扮演', file: 'components/Sidebar.vue' },
  { term: '角色库', file: 'App.vue', re: '>\\s*角色库\\s*<' },
  // 任务工作台
  { term: '任务工作台', file: 'components/TaskBoard.vue' },
  { term: '计划步骤', file: 'components/TaskBoard.vue' },
  { term: '子任务执行', file: 'components/TaskBoard.vue' },
  { term: '流程步骤进度', file: 'components/TaskBoard.vue' },
  { term: '最终计划', file: 'components/TaskBoard.vue' },
  { term: '对话记录', file: 'components/TaskBoard.vue' },
  // 执行模式与授权档位
  { term: '三段式', file: 'api/labels.ts' },
  { term: '单 Agent', file: 'api/labels.ts' },
  { term: '多 Agent', file: 'api/labels.ts' },
  { term: '先规划后批准', file: 'api/labels.ts' },
  { term: '团队协作', file: 'api/labels.ts' },
  { term: '自定义流程', file: 'api/labels.ts' },
  { term: '始终需授权', file: 'components/settings/AuthorizationSection.vue' },
  { term: '严格', file: 'components/settings/AuthorizationSection.vue' },
  { term: '宽松', file: 'components/settings/AuthorizationSection.vue' },
  { term: '放行', file: 'components/settings/AuthorizationSection.vue' },
  { term: '默认模式', file: 'components/settings/UiSection.vue' },
];

// ===== 2. 已删除/已改名界面的旧叫法:不得出现在步骤标题或跳转标签里 =====
// 只管 title/navLabel(散文正文里作为「已被取代」的历史说明是允许的,如 s-assets 那句)。
const RETIRED_LABELS = ['预设导入', 'Agent 执行流程', '楼层位置', '技能库', '插件库', '执行器档位'];

// ===== 3. 解析 SettingsHub 的两级导航(行契约,见文件头) =====
function parseHubNav() {
  const src = read('components/SettingsHub.vue');
  const start = src.indexOf('const domains: Domain[] = [');
  if (start < 0) throw new Error('check-tutorial: 找不到 `const domains: Domain[] = [`');
  const sections = new Map(); // sectionKey -> 「域 · 分区」
  const actions = new Map(); // actionKey -> 项标签
  let domainLabel = '';
  // 只扫到 domains 数组结束(以行首 `];` 为界,数组后紧跟 ROLEPLAY_ONLY)
  const body = src.slice(start, src.indexOf('const ROLEPLAY_ONLY', start));
  for (const line of body.split('\n')) {
    const d = line.match(/^\s*key: '([^']+)', label: '([^']+)', dot:/);
    if (d) {
      domainLabel = d[2];
      continue;
    }
    const it = line.match(/type: '(section|action)', key: '([^']+)', label: '([^']+)'/);
    if (!it) continue;
    if (it[1] === 'section') sections.set(it[2], `${domainLabel} · ${it[3]}`);
    else actions.set(it[2], it[3]);
  }
  return { sections, actions };
}

// ===== 主流程:文案与步骤表用真实 import(纯 TS 数据,Node 原生类型剥离) =====
const hub = parseHubNav();
const { STEP_COPY, ABOUT_SECTIONS, ABOUT_OMIT, aboutSectionLines } = await import(
  pathToFileURL(path.join(WEB, 'utils', 'tutorialCopy.ts')).href
);
const onb = await import(pathToFileURL(path.join(WEB, 'onboarding.ts')).href);

// R1 步骤 id 联合 = STEP_COPY 键集合(有 id 无文案 / 有文案无 id 都是红)
const unionSrc = read('onboarding.ts');
const m = unionSrc.match(/export type TutorialStepId =([\s\S]*?);/);
if (!m) {
  fail('R1', 'onboarding.ts 里找不到 `export type TutorialStepId = …;`(改了写法要同步本脚本)');
} else {
  const ids = [...m[1].matchAll(/'([a-z]-[a-z-]+)'/g)].map((x) => x[1]);
  const copyIds = Object.keys(STEP_COPY);
  const missing = ids.filter((id) => !copyIds.includes(id));
  const extra = copyIds.filter((id) => !ids.includes(id));
  if (missing.length) fail('R1', `步骤 id 有类型无文案: ${missing.join(', ')}`);
  if (extra.length) fail('R1', `文案有步骤 id 不在类型联合里: ${extra.join(', ')}`);
  if (!missing.length && !extra.length) info(`R1 步骤 id ↔ 文案 ${copyIds.length} 条一一对应`);
}

// R2 每个跳转落点必须可达,且 navLabel 等于真实标签
for (const [id, c] of Object.entries(STEP_COPY)) {
  if (!c.nav) continue;
  const label = c.navLabel ?? '';
  if (!label) {
    fail('R2', `${id} 有 nav 却没有 navLabel(浮条与步骤条要显示位置)`);
    continue;
  }
  if (c.nav.kind === 'section') {
    const real = hub.sections.get(c.nav.section);
    if (!real) {
      fail('R2', `${id} 指向分区 '${c.nav.section}',但综合设置导航里没有这个 section(已删/改名?)`);
    } else if (real !== label) {
      fail('R2', `${id} 的 navLabel「${label}」≠ 真实标题「${real}」`);
    }
  } else {
    const real = hub.actions.get(c.nav.panel);
    if (!real) {
      fail('R2', `${id} 指向面板 '${c.nav.panel}',但综合设置导航里没有这个 action(已删/改名?)`);
    } else if (label !== real && !label.endsWith(real)) {
      fail('R2', `${id} 的 navLabel「${label}」里没有真实项名「${real}」`);
    }
  }
}

// R3 界面名词表:每个登记的界面名必须还在源码里(改名 = 红)
for (const n of UI_NOUNS) {
  let src;
  try {
    src = read(n.file);
  } catch {
    fail('R3', `名词表指向的文件不存在: web/src/${n.file}`);
    continue;
  }
  const hit = n.re ? new RegExp(n.re, 'm').test(src) : src.includes(n.term);
  if (!hit) {
    fail(
      'R3',
      `界面名「${n.term}」在 web/src/${n.file} 里已找不到${n.re ? '(按 re 匹配)' : ''}` +
        ` —— 教程引用的界面改名或被删了`,
    );
  }
}

// R4 退役叫法不得复现于标题与跳转标签
for (const [id, c] of Object.entries(STEP_COPY)) {
  for (const field of [c.title, c.navLabel ?? '']) {
    for (const bad of RETIRED_LABELS) {
      if (field.includes(bad)) {
        fail('R4', `${id} 的标题/跳转标签写着退役名「${bad}」:「${field}」`);
      }
    }
  }
}
// R4b nav 目标不得指向已退役的 section key(类型上已不存在,这里防有人加回来)
for (const [id, c] of Object.entries(STEP_COPY)) {
  if (c.nav?.kind === 'section' && !hub.sections.has(c.nav.section)) {
    fail('R4b', `${id} 的 nav.section='${c.nav.section}' 不是当前设置分区键`);
  }
}

// R5 关于页速览覆盖:每个步骤要么进 ABOUT_SECTIONS,要么在 ABOUT_OMIT 里写明理由
{
  const covered = new Set(ABOUT_SECTIONS.flatMap((s) => s.steps));
  const omitted = new Map((ABOUT_OMIT ?? []).map((o) => [o.id, o.reason]));
  const allIds = Object.keys(STEP_COPY);
  for (const id of allIds) {
    if (covered.has(id)) continue;
    if (!omitted.has(id)) fail('R5', `步骤 ${id} 既不在关于页速览里,也没在 ABOUT_OMIT 写明理由`);
    else if (!omitted.get(id) || !omitted.get(id).trim()) fail('R5', `ABOUT_OMIT 的 ${id} 缺理由`);
  }
  for (const id of covered) {
    if (!allIds.includes(id)) fail('R5', `ABOUT_SECTIONS 引用了不存在的步骤 ${id}`);
  }
  // 速览正文不得为空(引用步骤后 lines 为空说明文案被删空)
  for (const s of ABOUT_SECTIONS) {
    if (aboutSectionLines(s).length === 0) fail('R5', `关于页段落「${s.heading}」一行正文都没有`);
  }
}

// R6 步数一致性 + 报告(内部一致:都需要 = 首分支 + 次分支去掉共同段)
{
  const plan = (preference, scope, isAndroid) =>
    onb.buildTutorialPlan({ preference, scope, isAndroid });
  const bad = [];
  for (const isAndroid of [false, true]) {
    for (const preference of ['roleplay', 'task']) {
      const p1 = plan(preference, 'preferred', isAndroid);
      const pBoth = plan(preference, 'both', isAndroid);
      const other = preference === 'roleplay' ? 'task' : 'roleplay';
      const p2 = plan(other, 'preferred', isAndroid);
      const shared = p2.filter((id) => id.startsWith('s-'));
      const expected = p1.length + (p2.length - shared.length);
      if (pBoth.length !== expected) {
        bad.push(`${preference}/Android=${isAndroid}: 都需要 ${pBoth.length} 步 ≠ 派生 ${expected} 步`);
      }
      // 共同段在两条分支都讲时只能出现一次
      for (const id of shared) {
        if (pBoth.filter((x) => x === id).length !== 1) bad.push(`共同段 ${id} 在「都需要」里重复`);
      }
      if (!bad.length) {
        info(
          `R6 步数(偏好=${preference}, Android=${isAndroid}):单分支 ${p1.length} / 都需要 ${pBoth.length}`,
        );
      }
    }
  }
  for (const b of bad) fail('R6', b);
}

// R7 panel 键 → store 开关名映射必须与弹窗注册表 `modals.ts` 的 flag 同源
// (教程结束时靠这张表关掉自己打开的面板;flag 改名这里就红)。
{
  const modalsSrc = read('modals.ts');
  const flags = new Set([...modalsSrc.matchAll(/modal\('([A-Za-z]+)',/g)].map((x) => x[1]));
  if (flags.size === 0) fail('R7', 'modals.ts 里解析不到任何 modal(\'flag\', …) 声明(注册表写法变了?)');
  for (const [panel, flag] of Object.entries(onb.PANEL_STORE_FLAG)) {
    if (!flags.has(flag)) {
      fail('R7', `panel '${panel}' 映射到开关 '${flag}',但弹窗注册表里没有这个 flag`);
    }
  }
  if (!fails.some((f) => f.startsWith('[R7]'))) info(`R7 panel 映射 ${Object.keys(onb.PANEL_STORE_FLAG).length} 条均命中 modals.ts flag`);
}

// ===== 输出 =====
if (VERBOSE) for (const i of infos) console.log(`  INFO ${i}`);
if (fails.length) {
  console.log('========== 新手教程防漂移机检 ==========');
  for (const f of fails) console.log(`  [FAIL] ${f}`);
  console.log(`\n[FAIL] ${fails.length} 处失配 —— 教程内容与真实界面已分叉,按上面提示改**代码侧**`);
  console.log('        (界面确实改了 → 改 utils/tutorialCopy.ts 的步骤表/名词表;`--verbose` 看步数现值)');
  process.exit(1);
}
console.log('[ OK ] 新手教程防漂移机检:步骤表/跳转落点/界面名词/退役名/速览覆盖/步数/panel 开关映射 七类一致');
if (!VERBOSE) console.log('        （`--verbose` 打印步数与逐条派生值）');
