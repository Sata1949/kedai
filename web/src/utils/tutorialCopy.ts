// 新手教程文案表(TUTSYNC OB-10):全仓**唯一**一份教程文字。
//
// 为什么要把它从 OnboardingModal.vue 里搬出来:教程有三处载体(分步弹窗 / 关于页文字速览 /
// 台账),原设计靠 `OnboardingModal.vue` 头顶的一句注释约定「改功能时同步此文件」。2026-10-10
// 盘点实测该约定已经烂掉——弹窗与关于页同时存在互不一致的旧界面名,而 54 例教程测试**全绿**
// (它们只锁结构不变量与硬编码步数,读不出散文)。搬成共享常量后,同一句话在 `web/src` 只可能
// 命中一处定义,分叉从「没人能发现」变成「编译期就写不出来」。
//
// 分层:本文件是 L1 纯数据(`utils` 已登记 L1),被 L3 的 OnboardingModal.vue 与
// components/settings/AboutSection.vue 共用(L3 → L1 是允许方向)。两个引用方都是懒加载 chunk,
// 故本模块被 Vite hoist 成共享异步 chunk,不进首屏 index——收口时以 `tools/check-bundle.mjs` 复核。
//
// 文案纪律:
//   ① 界面名一律用**屏幕上真实存在的字**(导航 label / 按钮文字 / 分区标题),不得用内部枚举名;
//   ② `lines` 是**中性事实句**,弹窗与关于页共用;只向导才说的话放 `tourNote`;
//   ③ 不得出现已删除的 UI 名(「预设导入」「楼层位置」「技能库」「Agent 执行流程」…),
//      由 `tools/check-tutorial.mjs` 黑名单机检。
import type { SettingsNavRequest, TutorialStepId } from '../onboarding';

/** 一步的文案 */
export interface StepCopy {
  title: string;
  /** 中性事实句(弹窗与关于页共用) */
  lines: string[];
  /** 跳转落点;非空时该步带「带我去设置」按钮(按钮旁与浮条显示 `navLabel`) */
  nav?: SettingsNavRequest;
  /** 该落点在综合设置里的显示名(= SettingsHub 的二级项 label;导航校验用它) */
  navLabel?: string;
  /** 只在向导里说的过渡语(不进关于页速览) */
  tourNote?: string;
}

/** 讲解步骤文案(与 docs/功能-变更史.md 的 OB 章、AboutSection「使用教程」保持同步) */
export const STEP_COPY: Record<TutorialStepId, StepCopy> = {
  'r-import': {
    title: '第一步:导入角色卡',
    lines: [
      '桌面:左栏底部点「导入角色卡」,选 PNG 卡片或 JSON,导入后会自动选中该角色。',
      '移动端:底部导航点「角色库」打开抽屉,里面有同一颗「导入角色卡」按钮。',
      '列表里已经有卡时,直接在左栏点一张卡即可开始对话。',
    ],
  },
  'r-intro': {
    title: '角色卡与开场',
    lines: [
      '左栏的角色列表点一张卡就开始对话(这一列没有标题,上方是「搜索角色」输入框)。',
      '顶栏「开场」用于切换多条开场白,切换会清空当前会话并重新开始;只有配了多条开场白的角色卡才会出现这颗按钮。',
    ],
  },
  'r-display': {
    title: '卡片显示不全怎么办',
    lines: [
      '只有文字、缺排版或折叠块:打开顶栏「HTML」开关(提示条写的是「安全 HTML 渲染」,按角色卡分别记忆)。开启后消息正文里的结构标签按白名单渲染,样式与事件属性会被剥掉。',
      '界面能显示、但下拉或按钮点了没反应:点顶栏的「JS 禁用」按钮去授权,会先弹风险确认;授权只对当前角色与当前脚本版本生效,脚本改动后自动失效。已授权时同一颗按钮显示「JS 已授权」,再点即撤销。',
    ],
  },
  'r-sendfail': {
    title: '发送失败怎么办',
    lines: [
      '发送出错时对话区上方出现常驻错误条,点「重试」即可再发一次;失败不会丢内容,草稿保留在输入框里。',
      '点停止后输入框显示「正在停止…」,这是在等上游收尾的正常过渡态,不用重复点。',
    ],
  },
  's-enter': {
    title: '综合设置',
    lines: [
      '绝大部分系统设置都整合在「综合设置」里:桌面在左上角侧栏底部,移动端在底部导航「设置」。',
    ],
    nav: { kind: 'section', section: 'api' },
    // navLabel 一律写**落点自己的真实标题**(不是步骤主题):浮条与步骤条显示它,
    // 机检拿它比对 SettingsHub 的实际标题。这一步讲的是「综合设置」这个总入口,
    // 而落点是Hub 的默认分区「连接与模型 · API 连接」,所以这里写后者。
    navLabel: '连接与模型 · API 连接',
    tourNote: '请现在点开它 —— 下面几步带您走一遍最常打交道的几项,可随时点「继续教程」回到这里。',
  },
  's-api': {
    title: '连接与模型 · API 连接',
    lines: [
      'BASE URL / API KEY / MODEL 三项在这里填;相关内容获取请前往各大模型厂商的云文档与 API／套餐界面。',
      '默认连接就在本区编辑;需要多套连接(不同厂商、不同密钥)可在「连接配置」里保存并切换。',
    ],
    nav: { kind: 'section', section: 'api' },
    navLabel: '连接与模型 · API 连接',
  },
  's-import': {
    title: '对话与提示词 · 综合导入 / 导出',
    lines: [
      '一个文件可以同时导入多段内容:①预设条目 / ②自定义执行流程 / ③④提示词 / ⑤技能 / ⑥预设正则,逐段勾选、逐段执行,失败互不影响并写进导入回执。',
      '导出把当前这些资产打成一个综合导出件(不含 API 连接与密钥)。任务模式下 ①预设条目与 ⑥预设正则不适用,会自动跳过。',
    ],
    nav: { kind: 'panel', panel: 'importBundle' },
    navLabel: '综合导入 / 导出',
  },
  's-assets': {
    title: '对话与提示词 · 附件管理',
    lines: [
      '技能 / 流程 / 插件 / 预设正则 四类附件集中在四个页签里管理,可用虚拟文件夹分组,支持「移动到…」与导入导出。',
      '原先散在各处的「技能库」「插件」独立窗口已由本面板取代。',
    ],
    nav: { kind: 'panel', panel: 'assets' },
    navLabel: '附件管理',
  },
  's-ext': {
    title: '连接与模型 · MCP 服务与三个能力包',
    lines: [
      '「MCP 服务」管理外部 MCP 实例:状态徽章、失败原因与一键重连都在这里;工具卡上会标出这次调用来自插件还是 MCP。',
      '「编码能力包」「文学能力包」「视觉与截图」三个能力包默认关,开启后才影响对应模式的缺省提示词与工具面。',
    ],
    nav: { kind: 'section', section: 'mcp' },
    navLabel: '连接与模型 · MCP 服务',
  },
  's-flow': {
    title: 'Agent 与任务 · 执行流程',
    lines: [
      '这里自定义模型的执行流程:节点＝一次模型调用,连线＝先后依赖,另有列表与画布两种编辑方式。',
      '编排好的流程会在任务模式的「自定义流程」执行模式中释放:新建任务时选中它并绑定(绑定即冻结,之后改流程不影响该任务)。',
    ],
    nav: { kind: 'section', section: 'flow' },
    navLabel: 'Agent 与任务 · 执行流程',
  },
  's-exec': {
    title: 'Agent 与任务 · 授权与命令执行',
    lines: [
      '授权模式三档(严格 / 宽松 / 放行)决定工具执行前的确认策略,选定的档位会被记住;「始终需授权」可点名哪些工具必须逐个确认。',
      '命令执行总开关与执行审计也在本区。破坏性与提权命令(删除/格式化/su/sudo/包管理等)不受授权模式豁免,一律逐条确认;任务模式下这类高危命令不可用(无确认通道,直接拒绝)。',
    ],
    nav: { kind: 'section', section: 'exec' },
    navLabel: 'Agent 与任务 · 授权与命令执行',
  },
  's-android-exec': {
    title: '执行器等级(Android)',
    lines: [
      'Android 环境通过执行器等级进行权限区分:沙箱档 / Shizuku(ADB 权限)/ ROOT 提权(最高风险)。',
      '「执行审计」按来源与风险(只读 / 写入 / 破坏性 / 提权·系统)列出每一次命令执行记录。',
    ],
    nav: { kind: 'section', section: 'exec' },
    navLabel: 'Agent 与任务 · 授权与命令执行',
  },
  'r-prompt': {
    title: '对话与提示词',
    lines: [
      '「提示词注入」配置常驻楼层:可拖拽排序、逐层启停、指定角色与正文。注入位置固定为按从上到下顺序,已不再单独配置。',
      '导入预设走「综合导入 / 导出」的 ①预设条目段,写进的就是这些楼层。',
      '「Agent 设置」里角色扮演与任务各有独立的提示词(按页面上的模式徽标区分当前在改哪一套),互不影响。',
    ],
    nav: { kind: 'section', section: 'prompt' },
    navLabel: '对话与提示词 · 提示词注入',
  },
  'r-memory': {
    title: '世界书与记忆',
    lines: [
      '世界书条目按关键词自动激发;角色卡内嵌的世界书随卡导入。',
      '「向量化模型」开启后可做语义召回;「记忆库」面板可查看与蒸馏长期记忆。',
    ],
    nav: { kind: 'panel', panel: 'memory' },
    navLabel: '数据与记忆 · 记忆库',
  },
  't-switch': {
    title: '切到任务模式',
    lines: [
      '左栏顶部(移动端是底部导航)切到「任务」,在「下达目标」里写下目标并提交。',
      '目标会成为任务标题,模型据此拆解计划并执行。',
    ],
  },
  't-mode': {
    title: '执行模式六选一',
    lines: [
      '「模式」下拉六选一:模式:三段式(默认)/ 模式:单 Agent / 模式:多 Agent / 模式:先规划后批准 / 模式:团队协作 / 模式:自定义流程。',
      '复杂目标建议「先规划后批准」(先出计划、您批准后执行)或「团队协作」(分派子目标并做审计)。',
    ],
  },
  't-custom-flow': {
    title: '自定义流程',
    lines: [
      '选「自定义流程」后可绑定某份流程(绑定即冻结,之后改流程不影响该任务)。',
      '流程模式还能选「对比」:把名单内的流程作为工具交给模型自主调用。',
    ],
  },
  't-board': {
    title: '任务工作台',
    lines: [
      '「计划步骤」「子任务执行」「流程步骤进度」「最终计划」与「对话记录」都在任务工作台里;被取消或超时的任务也会保留已完成的部分成果。',
    ],
  },
  't-agent': {
    title: 'Agent 面板',
    lines: [
      '面板是一个入口两块页签:「AGENT 状态」放推理链与工具调用,「调用情况」放调用记录(页签标签在界面上是大写体);工具行的徽章标明这次调用来自插件还是 MCP。',
      '生成或任务开始时它会自动展开一次;收起后本会话不再自动弹出,需要时点右缘的「AGENT」条(移动端是底部导航「AGENT」)。',
    ],
  },
  't-workspace': {
    title: '编码类目标',
    lines: [
      '给任务绑定工作区(项目目录)后,任务会用文件读写与搜索工具在目录内工作,越界路径会被拦截。',
      '建任务时可看到工作区画像;任务详情里能查文件变更 —— 单文件看 diff 与回滚、整任务全部回滚、导出 patch。',
    ],
  },
};

/** 关于页「使用教程」的一段:标题 + 由哪些步骤供正文 + 段专属补充句 */
export interface AboutSectionCopy {
  heading: string;
  /** 引用的步骤 id(正文逐句取 `STEP_COPY[id].lines`,`tourNote` 不进这里) */
  steps: TutorialStepId[];
  /** 不属于任何讲解步骤、只写在速览里的句子 */
  extraLines?: string[];
}

/**
 * 关于页文字速览(2026-10-10 TUTSYNC OB-11:由「另一份手写文案」改成「步骤表的另一种排布」)。
 * 顺序即页面渲染顺序。
 */
export const ABOUT_SECTIONS: AboutSectionCopy[] = [
  {
    heading: '角色扮演模式',
    steps: ['r-import', 'r-intro', 'r-display', 'r-sendfail'],
  },
  {
    heading: '任务模式',
    steps: ['t-switch', 't-mode', 't-board', 't-agent'],
  },
  {
    heading: '导入与附件',
    steps: ['s-import', 's-assets', 's-ext'],
  },
  {
    heading: '自定义流程',
    steps: ['t-custom-flow', 's-flow'],
    extraLines: [
      '节点可单独配置连接、上下文上限、工具轮次、超时/重试与严格/宽松档位;流程可整体导出/导入。',
      '开启编码能力包后,内置流程库会多出编码流程预设,可在任务里一键选用。',
    ],
  },
  {
    heading: '编码类目标',
    steps: ['t-workspace'],
  },
  {
    heading: '记忆与世界书',
    steps: ['r-memory'],
    extraLines: ['「世界书」与「记忆库」都在「数据与记忆」域里。'],
  },
  {
    heading: '提示词与授权',
    steps: ['r-prompt', 's-exec'],
  },
  {
    heading: '快捷键与输入',
    steps: [],
    extraLines: ['Enter 发送,Shift+Enter 换行;输入「/」触发命令联想。'],
  },
];

/**
 * 关于页速览**不收**的步骤及理由(缺这张表 = `tools/check-tutorial.mjs` 判 FAIL)。
 * 目的是逼一次显式决定:每个讲解步骤要么进速览,要么写下「为什么不进」,
 * 避免「弹窗讲了、速览烂了」这类无人认领的分叉(2026-10-10 实测正是这么烂的)。
 */
export const ABOUT_OMIT: { id: TutorialStepId; reason: string }[] = [
  { id: 's-enter', reason: '关于页本身就在综合设置里,「综合设置」入口在段首「新手引导」一行已述' },
  { id: 's-api', reason: 'API 连接属首次配置而非日常功能,由首启引导讲解' },
  { id: 's-android-exec', reason: 'Android 专属档位,Windows 侧关于页不讲' },
];

/** 把速览段展开成「标题 + 正文行」(关于页与 `tools/check-tutorial.mjs` 共用同一派生) */
export function aboutSectionLines(section: AboutSectionCopy): string[] {
  const fromSteps = section.steps.flatMap((id) => STEP_COPY[id].lines);
  return [...fromSteps, ...(section.extraLines ?? [])];
}
