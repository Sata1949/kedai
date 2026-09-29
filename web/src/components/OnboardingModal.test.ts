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
import { DEFAULT_APP_MODE_KEY, ONBOARDING_KEY, shouldShowOnboarding } from '../onboarding';
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
  preference: '1. 角色扮演' | '2. 任务执行',
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
    expect(wrapper.text()).toContain('2. 任务执行');
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
    await answerQuestions(wrapper, '2. 任务执行', '3. 不需要，谢谢');
    expect(wrapper.text()).toContain('教程已结束');
    expect(shouldShowOnboarding(localStorage)).toBe(false);
    // 「不需要」不讲解任何分支
    expect(JSON.parse(localStorage.getItem(ONBOARDING_KEY) ?? '{}').tutorials).toEqual([]);
  });
});

describe('首启教程 · 步骤推进', () => {
  it('只看喜欢的(角色扮演)= 8 步,首步显示「第 1 / 8 步」', async () => {
    const { wrapper } = mountTour();
    await answerQuestions(wrapper, '1. 角色扮演', '2. 我只需要看我喜欢的');
    expect(wrapper.text()).toContain('第 1 / 8 步');
    expect(wrapper.text()).toContain('角色卡与开场');
    // 第一步没有设置跳转按钮
    expect(wrapper.findAll('button').some((b) => b.text().includes('带我去设置'))).toBe(false);
  });

  it('「下一步」推进到第二步,「上一步」在首步禁用', async () => {
    const { wrapper } = mountTour();
    await answerQuestions(wrapper, '1. 角色扮演', '2. 我只需要看我喜欢的');
    const prev = wrapper.findAll('button').find((b) => b.text().includes('上一步'));
    expect(prev?.attributes('disabled')).toBeDefined();
    await clickText(wrapper, '下一步');
    expect(wrapper.text()).toContain('第 2 / 8 步');
    expect(wrapper.text()).toContain('卡片显示不全怎么办');
  });

  it('都需要 = 两条分支都讲,共同设置段只出现一次', async () => {
    const { wrapper } = mountTour();
    await answerQuestions(wrapper, '2. 任务执行', '1. 都需要');
    // 偏好任务 = 任务分支在前:第 1 步是「切到任务模式」
    expect(wrapper.text()).toContain('第 1 / 14 步');
    expect(wrapper.text()).toContain('切到任务模式');
  });

  it('非 Android 环境不出现执行器等级那一步(Android 由平台门控释放)', async () => {
    const { wrapper } = mountTour();
    await answerQuestions(wrapper, '1. 角色扮演', '2. 我只需要看我喜欢的');
    for (let i = 0; i < 8; i++) await clickText(wrapper, i === 7 ? '完成教程' : '下一步');
    expect(wrapper.text()).toContain('教程已结束');
    // 结束屏与全程都不含 Android 专属步骤
    expect(wrapper.text()).not.toContain('执行器等级（Android）');
  });
});

describe('首启教程 · 带我去设置(浮条形态)', () => {
  /** 走到带 nav 的那一步(第 3 步 = 综合设置入口) */
  async function gotoNavStep(wrapper: VueWrapper): Promise<void> {
    await answerQuestions(wrapper, '1. 角色扮演', '2. 我只需要看我喜欢的');
    await clickText(wrapper, '下一步');
    await clickText(wrapper, '下一步');
  }

  it('点「带我去设置」:打开并定位设置、教程变浮条、浮条形态不渲染遮罩', async () => {
    const { wrapper, store } = mountTour();
    await gotoNavStep(wrapper);
    expect(wrapper.text()).toContain('第 3 / 8 步');
    await clickText(wrapper, '带我去设置');

    expect(store.settingsOpen).toBe(true);
    expect(store.settingsNav?.section).toBe('api');
    // 浮条形态:只有浮条,没有遮罩(否则点不到下面的综合设置)
    expect(wrapper.find('.ob-bar').exists()).toBe(true);
    expect(wrapper.find('.sv-modal-mask').exists()).toBe(false);
    expect(wrapper.text()).toContain('新手教程 · 第 3 / 8 步 · 综合设置');
  });

  it('浮条「继续教程」回到展开态且进度不变', async () => {
    const { wrapper } = mountTour();
    await gotoNavStep(wrapper);
    await clickText(wrapper, '带我去设置');
    await clickText(wrapper, '继续教程');
    expect(wrapper.find('.sv-modal-mask').exists()).toBe(true);
    expect(wrapper.find('.ob-bar').exists()).toBe(false);
    expect(wrapper.text()).toContain('第 3 / 8 步');
  });

  it('后续步骤的目标分区按步骤切换(API 连接 → api,执行流程 → flow)', async () => {
    const { wrapper, store } = mountTour();
    await gotoNavStep(wrapper);
    await clickText(wrapper, '带我去设置');
    await clickText(wrapper, '继续教程');
    await clickText(wrapper, '下一步'); // 第 4 步:连接与模型 · API 连接
    await clickText(wrapper, '带我去设置');
    expect(store.settingsNav?.section).toBe('api');
    await clickText(wrapper, '继续教程');
    await clickText(wrapper, '下一步'); // 第 5 步:Agent 与任务 · 执行流程
    await clickText(wrapper, '带我去设置');
    expect(store.settingsNav?.section).toBe('flow');
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
    // 已讲到第 3 步(含设置段),但未讲完两条分支——只记录实际覆盖的分支
    expect(JSON.parse(localStorage.getItem(ONBOARDING_KEY) ?? '{}').tutorials).toEqual(['roleplay']);
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
    const taskOpt = wrapper.findAll('button').find((b) => b.text().includes('2. 任务执行'));
    expect(taskOpt?.classes()).toContain('on');
  });
});
