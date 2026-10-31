// @vitest-environment jsdom
// 新手教程组件交互测试(jsdom + @vue/test-utils 真实挂载;沿用
// AgentFlowSection.canvasLoadError.test.ts 2026-09-21 起的挂载测试模式)。
//
// 覆盖的是**状态机**而不是静态文案:三问作答 → 步骤推进 → 「带我去设置」最小化为浮条
// (且浮条形态不渲染遮罩,综合设置仍可操作)→ 浮条返回 → 完成/跳过写标记。
// 步骤表与文案的纯逻辑在 onboarding.test.ts;本文件只管「点下去会怎样」。
import { beforeEach, describe, expect, it } from 'vitest';
import { flushPromises, mount, type VueWrapper } from '@vue/test-utils';
import { createPinia, setActivePinia } from 'pinia';
import OnboardingModal from './OnboardingModal.vue';
import {
  DEFAULT_APP_MODE_KEY,
  ONBOARDING_KEY,
  shouldShowOnboarding,
  buildTutorialPlan,
  type TutorialStepId,
} from '../onboarding';
import { STEP_COPY } from '../utils/tutorialCopy';
import { useAppStore } from '../store';

/** 挂载教程组件(组件从 uiPrefs 取动作,需要活动 pinia) */
function mountTour(): { wrapper: VueWrapper; store: ReturnType<typeof useAppStore> } {
  const wrapper = mount(OnboardingModal);
  return { wrapper, store: useAppStore() };
}

/** 按可见文本点按钮(仓库既有写法:findAll('button') + text 匹配) */
async function clickText(wrapper: VueWrapper, text: string): Promise<void> {
  const btn = wrapper.findAll('button').find((b) => b.text().includes(text));
  expect(btn, `未找到按钮:${text}`).toBeTruthy();
  await btn?.trigger('click');
  await flushPromises();
}

/** 走完前两问(偏好 + 范围),落在第一步骤屏 */
async function answerQuestions(
  wrapper: VueWrapper,
  preference: '1. 角色扮演' | '2. 任务',
  scope: string,
): Promise<void> {
  await clickText(wrapper, preference);
  await clickText(wrapper, scope);
}

beforeEach(() => {
  localStorage.clear();
  setActivePinia(createPinia());
});

describe('首启教程 · 三问', () => {
  it('第一问渲染两个选项与「决定每次启动」的说明', () => {
    const { wrapper } = mountTour();
    expect(wrapper.text()).toContain('偏好选择');
    expect(wrapper.text()).toContain('1. 角色扮演');
    expect(wrapper.text()).toContain('2. 任务');
    expect(wrapper.text()).toContain('此次选择将决定每次启动 Kedai 后默认进入的模式');
  });

  it('第一问作答即落盘偏好并进入第二问', async () => {
    const { wrapper } = mountTour();
    await clickText(wrapper, '1. 角色扮演');
    expect(localStorage.getItem(DEFAULT_APP_MODE_KEY)).toBe('roleplay');
    expect(wrapper.text()).toContain('是否开始教学流程');
    expect(wrapper.text()).toContain('3. 不需要，谢谢');
  });

  it('第二问可选「返回上一步」回到偏好选择', async () => {
    const { wrapper } = mountTour();
    await clickText(wrapper, '1. 角色扮演');
    await clickText(wrapper, '返回上一步');
    expect(wrapper.text()).toContain('偏好选择');
  });

  it('选「不需要」直接落到结束屏,并写入已完成标记', async () => {
    const { wrapper } = mountTour();
    await answerQuestions(wrapper, '2. 任务', '3. 不需要，谢谢');
    expect(wrapper.text()).toContain('教程已结束');
    expect(shouldShowOnboarding(localStorage)).toBe(false);
    // 「不需要」不讲解任何分支
    expect(JSON.parse(localStorage.getItem(ONBOARDING_KEY) ?? '{}').tutorials).toEqual([]);
  });
});

// 步数与标题**从步骤表派生**(TUTSYNC OB-12):原先硬编码「8 / 14 / 第 3 步」,
// 每次增减步骤都要同步改数字——那不是断言而是复读,一旦漏改就把旧步数焊死在测试里。
/** 偏好角色扮演的单分支计划(jsdom 下 isAndroidTauri 恒 false) */
const RP_PLAN = buildTutorialPlan({ preference: 'roleplay', scope: 'preferred', isAndroid: false });
/** 偏好任务的双分支计划 */
const BOTH_TASK_PLAN = buildTutorialPlan({ preference: 'task', scope: 'both', isAndroid: false });
/** 第 n 步(1 起)的标题 */
const titleAt = (plan: TutorialStepId[], n: number): string => STEP_COPY[plan[n - 1]].title;

describe('首启教程 · 步骤推进', () => {
  it('只看喜欢的(角色扮演)= 13 步,首步是「导入角色卡」且没有跳转按钮', async () => {
    const { wrapper } = mountTour();
    await answerQuestions(wrapper, '1. 角色扮演', '2. 我只需要看我喜欢的');
    expect(wrapper.text()).toContain(`第 1 / ${RP_PLAN.length} 步`);
    expect(wrapper.text()).toContain(titleAt(RP_PLAN, 1));
    expect(titleAt(RP_PLAN, 1)).toBe('第一步：导入角色卡');
    // 首步没有设置跳转按钮
    expect(wrapper.findAll('button').some((b) => b.text().includes('带我去设置'))).toBe(false);
  });

  it('「下一步」推进到第二步,「上一步」在首步禁用', async () => {
    const { wrapper } = mountTour();
    await answerQuestions(wrapper, '1. 角色扮演', '2. 我只需要看我喜欢的');
    const prev = wrapper.findAll('button').find((b) => b.text().includes('上一步'));
    expect(prev?.attributes('disabled')).toBeDefined();
    await clickText(wrapper, '下一步');
    expect(wrapper.text()).toContain(`第 2 / ${RP_PLAN.length} 步`);
    expect(wrapper.text()).toContain(titleAt(RP_PLAN, 2));
  });

  it('都需要 = 两条分支都讲(任务偏好时任务在前),共同设置段只出现一次', async () => {
    const { wrapper } = mountTour();
    await answerQuestions(wrapper, '2. 任务', '1. 都需要');
    expect(BOTH_TASK_PLAN[0]).toBe('t-switch');
    expect(wrapper.text()).toContain(`第 1 / ${BOTH_TASK_PLAN.length} 步`);
    expect(wrapper.text()).toContain('切到任务模式');
    // 共同段在双分支计划里各只出现一次(步骤表层面的不变量,渲染层按它编号)
    expect(BOTH_TASK_PLAN.filter((id) => id === 's-enter')).toHaveLength(1);
    expect(BOTH_TASK_PLAN.filter((id) => id === 's-import')).toHaveLength(1);
  });

  it('非 Android 环境不出现执行器等级那一步(Android 由平台门控释放)', async () => {
    const { wrapper } = mountTour();
    await answerQuestions(wrapper, '1. 角色扮演', '2. 我只需要看我喜欢的');
    for (let i = 0; i < RP_PLAN.length; i++) {
      await clickText(wrapper, i === RP_PLAN.length - 1 ? '完成教程' : '下一步');
    }
    expect(wrapper.text()).toContain('教程已结束');
    // 结束屏与全程都不含 Android 专属步骤
    expect(wrapper.text()).not.toContain('执行器等级（Android）');
    expect(RP_PLAN).not.toContain('s-android-exec');
  });
});

describe('首启教程 · 带我去设置(浮条形态)', () => {
  /** 走到第一个带 nav 的步骤(角色扮演分支的第 4 步 = 综合设置入口) */
  async function gotoNavStep(wrapper: VueWrapper): Promise<void> {
    await answerQuestions(wrapper, '1. 角色扮演', '2. 我只需要看我喜欢的');
    const navIdx = RP_PLAN.findIndex((id) => STEP_COPY[id].nav);
    for (let i = 0; i < navIdx; i++) await clickText(wrapper, '下一步');
    expect(RP_PLAN[navIdx]).toBe('s-enter');
  }

  it('点「带我去设置」:打开并定位设置、教程变浮条、浮条形态不渲染遮罩', async () => {
    const { wrapper, store } = mountTour();
    await gotoNavStep(wrapper);
    const n = RP_PLAN.findIndex((id) => id === 's-enter') + 1;
    expect(wrapper.text()).toContain(`第 ${n} / ${RP_PLAN.length} 步`);
    await clickText(wrapper, '带我去设置');

    expect(store.settingsOpen).toBe(true);
    expect(store.settingsNav?.request).toEqual({ kind: 'section', section: 'api' });
    // 浮条形态:只有浮条,没有遮罩(否则点不到下面的综合设置)
    expect(wrapper.find('.ob-bar').exists()).toBe(true);
    expect(wrapper.find('.sv-modal-mask').exists()).toBe(false);
    expect(wrapper.text()).toContain(
      `新手教程 · 第 ${n} / ${RP_PLAN.length} 步 · ${STEP_COPY['s-enter'].navLabel}`,
    );
  });

  it('浮条「继续教程」回到展开态且进度不变', async () => {
    const { wrapper } = mountTour();
    await gotoNavStep(wrapper);
    const n = RP_PLAN.findIndex((id) => id === 's-enter') + 1;
    await clickText(wrapper, '带我去设置');
    await clickText(wrapper, '继续教程');
    expect(wrapper.find('.sv-modal-mask').exists()).toBe(true);
    expect(wrapper.find('.ob-bar').exists()).toBe(false);
    expect(wrapper.text()).toContain(`第 ${n} / ${RP_PLAN.length} 步`);
  });

  it('后续步骤的目标按步骤切换:分区型取 section,面板型取 panel', async () => {
    const { wrapper, store } = mountTour();
    await gotoNavStep(wrapper);
    // s-enter → api(section 型)
    await clickText(wrapper, '带我去设置');
    expect(store.settingsNav?.request).toEqual({ kind: 'section', section: 'api' });
    await clickText(wrapper, '继续教程');
    await clickText(wrapper, '下一步'); // s-api
    await clickText(wrapper, '带我去设置');
    expect(store.settingsNav?.request).toEqual({ kind: 'section', section: 'api' });
    await clickText(wrapper, '继续教程');
    await clickText(wrapper, '下一步'); // s-import(独立面板型)
    await clickText(wrapper, '带我去设置');
    expect(store.settingsNav?.request).toEqual({ kind: 'panel', panel: 'importBundle' });
  });

  it('完成时把由教程打开的综合设置一并关掉', async () => {
    const { wrapper, store } = mountTour();
    await gotoNavStep(wrapper);
    await clickText(wrapper, '带我去设置');
    await clickText(wrapper, '继续教程');
    await clickText(wrapper, '✕'); // 跳过教程(顶部 ✕ 与浮条 ✕ 同语义)
    expect(store.onboardingOpen).toBe(false);
    expect(store.settingsOpen).toBe(false);
    expect(shouldShowOnboarding(localStorage)).toBe(false);
    // 已讲到设置段,但未讲完两条分支——只记录实际覆盖的分支
    expect(JSON.parse(localStorage.getItem(ONBOARDING_KEY) ?? '{}').tutorials).toEqual(['roleplay']);
  });

  it('结束时关掉由教程打开的独立面板;没被教程打开过的不碰', async () => {
    const { wrapper, store } = mountTour();
    await answerQuestions(wrapper, '1. 角色扮演', '2. 我只需要看我喜欢的');
    // 走到第 6 步(s-import,panel 型):s-enter(4) → s-api(5) → s-import(6)
    for (let i = 0; i < 5; i++) await clickText(wrapper, '下一步');
    expect(store.settingsNav).toBeNull(); // 还没点跳转
    await clickText(wrapper, '带我去设置');
    expect(store.settingsNav?.request).toEqual({ kind: 'panel', panel: 'importBundle' });
    // 真实链路由 SettingsHub 执行 action 置起开关(本用例没挂 Hub),这里等价模拟
    store.importBundleOpen = true;
    store.memoryOpen = true; // 用户自己开着的记忆库:教程不该碰它
    await clickText(wrapper, '✕');
    expect(store.importBundleOpen).toBe(false);
    expect(store.memoryOpen).toBe(true);
    expect(store.onboardingOpen).toBe(false);
  });
});

describe('首启教程 · 跳过与完成', () => {
  it('第一问就点 ✕ 跳过:偏好已落盘、标记已写、教程关闭', async () => {
    const { wrapper, store } = mountTour();
    await clickText(wrapper, '1. 角色扮演');
    await clickText(wrapper, '✕');
    expect(localStorage.getItem(DEFAULT_APP_MODE_KEY)).toBe('roleplay');
    expect(shouldShowOnboarding(localStorage)).toBe(false);
    expect(store.onboardingOpen).toBe(false);
    // 用户自己没开过设置,结束时不碰它
    expect(store.settingsOpen).toBe(false);
  });

  it('结束屏「开始使用」关闭教程,二次挂载仍从第一问开始(认真学习过的用户已写标记)', async () => {
    const { wrapper, store } = mountTour();
    await answerQuestions(wrapper, '1. 角色扮演', '3. 不需要，谢谢');
    await clickText(wrapper, '开始使用');
    expect(store.onboardingOpen).toBe(false);
    expect(shouldShowOnboarding(localStorage)).toBe(false);
  });

  it('已设过偏好时第一问预选该偏好(关于页重开场景)', () => {
    localStorage.setItem(DEFAULT_APP_MODE_KEY, 'task');
    const { wrapper } = mountTour();
    const taskOpt = wrapper.findAll('button').find((b) => b.text().includes('2. 任务'));
    expect(taskOpt?.classes()).toContain('on');
  });
});
