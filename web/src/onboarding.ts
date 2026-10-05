// 首启引导(新手教程)的机制层:偏好/完成标记的存储层 + 教程步骤表纯函数。
// 单独成根级模块(L1,零依赖)的两个理由:
//   ① 判定与存储逻辑要能在纯 node 环境单测(注入 StorageLike,同 renderHtmlPreference.ts
//      的写法)——浏览器传 localStorage,测试传内存实现,不需要 jsdom;
//   ② `SettingsSectionKey` 的**单一来源**放这里:该字面量联合被 stores(L2) 的
//      openSettingsAt 与 components(L3) 的 SettingsHub 共用,类型若留在 SettingsHub
//      会让 L2 反向依赖 L3(check-arch 规则 J 判 FAIL)。
// **本文件不含任何面向用户的文案**:教程文案随 OnboardingModal.vue 懒加载,不膨胀首屏 chunk。

/** 首启引导完成标记键:无此键 = 第一次使用(损坏值按未走过处理,只会多弹一次) */
export const ONBOARDING_KEY = 'kedai.onboarding.v1';
/**
 * 启动默认模式偏好键(缺省 = 未设)。
 *
 * 与粘性键 `kedai.appMode`(由 task store 拥有,记「上次用过的模式」)的区别:
 * 本键是用户在首启引导或「综合设置 → 界面」里显式选定的**默认**模式;
 * 设了它就严格按它决定每次启动的模式,会话内的临时切换不再跨启动生效。
 * 未设时行为与改动前完全一致(仍走粘性键)——老用户升级上来零影响。
 */
export const DEFAULT_APP_MODE_KEY = 'kedai.default-app-mode.v1';

/** 顶层运行模式(与 task store 的 appMode 同域) */
export type AppMode = 'roleplay' | 'task';

/**
 * 设置分区键(单一来源)。
 * 取值必须与 SettingsHub 的 domains 导航表一致——两组集合**手工对齐**(暂无自动校验:
 * 原注释声称由 SettingsHub.test.ts 反向校验,2026-09-28 核对为不存在,已如实修正;
 * 若要自动化,需新增一条「两组集合相等」的断言)。
 */
export type SettingsSectionKey =
  | 'api' | 'connections' | 'model' | 'mcp' | 'embedding'
  | 'prompt' | 'preset'
  | 'agent' | 'flow' | 'exec' | 'coding' | 'literary' | 'vision'
  | 'data'
  | 'ui'
  | 'about';

/**
 * 教程覆盖范围(首启引导第二问的答案)。
 * 语义:both = 两条分支都讲(顺序由偏好决定);preferred = 只讲偏好对应分支;none = 不讲。
 */
export type TutorialScope = 'both' | 'preferred' | 'none';

/**
 * 教程步骤 id。命名前缀表明归属:
 *   r-* 角色扮演分支专属;t-* 任务分支专属;s-* 设置导航共同段(只随**第一条**分支出现,
 *   两条分支都讲时不重复讲一遍设置)。
 * 文案与目标分区的映射在 OnboardingModal.vue 内(随组件懒加载)。
 */
export type TutorialStepId =
  | 'r-intro' | 'r-display' | 'r-prompt' | 'r-memory'
  | 't-switch' | 't-mode' | 't-custom-flow' | 't-board' | 't-agent' | 't-workspace'
  | 's-enter' | 's-api' | 's-flow' | 's-exec' | 's-android-exec';

/** 存储接口(注入式;浏览器传 localStorage,单测传内存实现) */
export interface StorageLike {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

/** 首启引导的落盘状态 */
export interface OnboardingState {
  version: 1;
  /** 恒为 true(读侧只认 true;缺/false 均视为未走过) */
  done: true;
  /** 用户选定的默认模式 */
  preferredMode: AppMode;
  /** 已讲过的分支(按次数累计取并集;重开引导不会丢掉旧记录) */
  tutorials: AppMode[];
  /** 完成时间(ISO 字符串;仅留档,不参与判定) */
  completedAt: string;
}

function isAppMode(v: unknown): v is AppMode {
  return v === 'roleplay' || v === 'task';
}

/** 合并两串分支名单:保序去重(结果可复现,便于断言) */
function mergeBranches(a: readonly AppMode[], b: readonly AppMode[]): AppMode[] {
  const out: AppMode[] = [];
  for (const v of [...a, ...b]) {
    if (!out.includes(v)) out.push(v);
  }
  return out;
}

/**
 * 读首启引导状态;未走过/值损坏一律返回 null(= 需要弹引导)。
 * 损坏值按「未走过」处理而不是按「已完成」:后者会让用户永远看不到引导,
 * 前者最坏只是多弹一次(用户点一次「不需要」即写回干净值)。
 */
export function readOnboarding(storage: StorageLike): OnboardingState | null {
  let raw: string | null;
  try {
    raw = storage.getItem(ONBOARDING_KEY);
  } catch {
    return null;
  }
  if (!raw) return null;
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return null;
  }
  if (!parsed || typeof parsed !== 'object') return null;
  const o = parsed as Partial<OnboardingState>;
  if (o.done !== true) return null;
  return {
    version: 1,
    done: true,
    preferredMode: isAppMode(o.preferredMode) ? o.preferredMode : 'roleplay',
    tutorials: Array.isArray(o.tutorials) ? mergeBranches([], o.tutorials.filter(isAppMode)) : [],
    completedAt: typeof o.completedAt === 'string' ? o.completedAt : '',
  };
}

/** 是否需要弹首启引导(未走过引导即需要) */
export function shouldShowOnboarding(storage: StorageLike): boolean {
  return readOnboarding(storage) === null;
}

/**
 * 写首启引导状态(完成/跳过都写)。
 * `tutorials` 与既有记录取并集:重开引导只讲一条分支时,不会抹掉之前已讲过的另一条。
 */
export function writeOnboarding(
  storage: StorageLike,
  patch: { preferredMode: AppMode; tutorials: readonly AppMode[]; completedAt?: string },
): void {
  const prev = readOnboarding(storage);
  const state: OnboardingState = {
    version: 1,
    done: true,
    preferredMode: patch.preferredMode,
    tutorials: mergeBranches(prev?.tutorials ?? [], patch.tutorials),
    completedAt: patch.completedAt ?? new Date().toISOString(),
  };
  try {
    storage.setItem(ONBOARDING_KEY, JSON.stringify(state));
  } catch {
    /* 忽略:写失败时下次启动再弹一次,不影响本次使用 */
  }
}

/** 读启动默认模式偏好;未设或值非法返回 null(= 回退粘性记忆) */
export function readDefaultAppMode(storage: StorageLike): AppMode | null {
  try {
    const v = storage.getItem(DEFAULT_APP_MODE_KEY);
    return isAppMode(v) ? v : null;
  } catch {
    return null;
  }
}

/** 写启动默认模式偏好(设置项与首启引导共用) */
export function writeDefaultAppMode(storage: StorageLike, mode: AppMode): void {
  try {
    storage.setItem(DEFAULT_APP_MODE_KEY, mode);
  } catch {
    /* 忽略 */
  }
}

/**
 * 启动模式判定(纯函数):偏好优先,无偏好回退粘性键的既有语义。
 *
 * 粘性值为 'task' 才算任务模式,其余(含 null/空串/异常值)一律角色扮演——与改动前
 * task store 的 readStoredAppMode 逐字一致,保证老用户升级后行为不变。
 */
export function resolveLaunchMode(preference: AppMode | null, stickyRaw: string | null): AppMode {
  if (preference === 'roleplay' || preference === 'task') return preference;
  return stickyRaw === 'task' ? 'task' : 'roleplay';
}

/** 设置导航共同段(角色扮演/任务分支共用;Android 专属的执行器等级按平台门控) */
function settingsSteps(isAndroid: boolean): TutorialStepId[] {
  const steps: TutorialStepId[] = ['s-enter', 's-api', 's-flow', 's-exec'];
  if (isAndroid) steps.push('s-android-exec');
  return steps;
}

/** 角色扮演分支步骤(includeSettings = 是否带设置共同段) */
function roleplaySteps(includeSettings: boolean, isAndroid: boolean): TutorialStepId[] {
  const steps: TutorialStepId[] = ['r-intro', 'r-display'];
  if (includeSettings) steps.push(...settingsSteps(isAndroid));
  steps.push('r-prompt', 'r-memory');
  return steps;
}

/** 任务分支步骤(includeSettings = 是否带设置共同段) */
function taskSteps(includeSettings: boolean, isAndroid: boolean): TutorialStepId[] {
  const steps: TutorialStepId[] = ['t-switch', 't-mode', 't-custom-flow', 't-board', 't-agent'];
  if (includeSettings) steps.push(...settingsSteps(isAndroid));
  steps.push('t-workspace');
  return steps;
}

/**
 * 生成本次要讲的步骤序列(第二问作答后冻结,教程期间不再重算)。
 *
 * 三条规则:
 *   ① scope='none' → 空表(不讲课);
 *   ② scope='preferred' → 只讲偏好对应分支;
 *   ③ scope='both' → 先偏好的分支(带设置共同段),再另一分支(**不带**设置段——
 *      设置导航只讲一遍,第二次到那里没有新信息)。
 * Android 专属步骤由调用方传入 isAndroid 决定,不用运行期 UA 探测(便于单测两态)。
 */
export function buildTutorialPlan(opts: {
  preference: AppMode;
  scope: TutorialScope;
  isAndroid: boolean;
}): TutorialStepId[] {
  if (opts.scope === 'none') return [];
  const roleplayFirst = opts.preference === 'roleplay';
  const first = roleplayFirst
    ? roleplaySteps(true, opts.isAndroid)
    : taskSteps(true, opts.isAndroid);
  if (opts.scope === 'preferred') return first;
  const second = roleplayFirst
    ? taskSteps(false, opts.isAndroid)
    : roleplaySteps(false, opts.isAndroid);
  return [...first, ...second];
}

/**
 * 本次讲解覆盖的分支名单(写入教程记录用;顺序 = 讲解顺序)。
 * 与 buildTutorialPlan 分开导出:记录分支不需要遍历步骤表,单测也更直白。
 */
export function planBranches(scope: TutorialScope, preference: AppMode): AppMode[] {
  if (scope === 'none') return [];
  if (scope === 'preferred') return [preference];
  return [preference, preference === 'roleplay' ? 'task' : 'roleplay'];
}
