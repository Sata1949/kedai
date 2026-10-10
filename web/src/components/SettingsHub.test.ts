// @vitest-environment jsdom
// SettingsHub 组件测试(阶段 A 补测):332 行,设置中心两层导航,此前无测试。
// 覆盖:导航区渲染、一级域切换、二级项点击切分区、以及 task 模式下的分区过滤
// (角色扮演专属项被隐藏;「执行流程」必须可见——IFW-3 入口错位回归)。
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { nextTick } from 'vue';
import { mount } from '@vue/test-utils';
import { createPinia, setActivePinia } from 'pinia';

const memStorage = new Map<string, string>();
vi.stubGlobal('localStorage', {
  getItem: (k: string) => memStorage.get(k) ?? null,
  setItem: (k: string, v: string) => void memStorage.set(k, String(v)),
  removeItem: (k: string) => void memStorage.delete(k),
  clear: () => memStorage.clear(),
  key: (i: number) => [...memStorage.keys()][i] ?? null,
  get length() { return memStorage.size; },
});

vi.mock('../api', async (importOriginal) => {
  const orig = await importOriginal<typeof import('../api')>();
  return { ...orig, listModels: vi.fn().mockResolvedValue([]), getSettings: vi.fn().mockResolvedValue({}) };
});
vi.mock('../api/health', () => ({
  health: vi.fn().mockResolvedValue({ ok: true, version: '0.0.0-test' }),
}));

// 屏蔽分区懒加载(本用例只验证「设置中心导航 + 任务模式下分区过滤」,与分区内部无关)。
//
// 必要性(2026-09-16 实测):SettingsModal 的 9 个分区经 lazyModal→defineAsyncComponent
// 动态 import,「点击域」会触发加载。测试随即结束、import 仍在飞行,环境拆除后该 Promise
// 才 reject → lazyModal 的 onError 重试一次 → 二次失败触发 console.error(报错文案与
// uiPrefs.modalLoadError 写入)→ 该 console 调用在 worker 关闭时呈 pending(vitest 报
// EnvironmentTeardownError: Closing rpc while "onUserConsoleLog" was pending)→ **退出码 1**,
// 而断言其实全绿。它只在全量套件下复现(单文件跑 10 次 0 失败;加入第 89 个测试文件后
// 触发率约 4/24),因为能否命中取决于 vitest 的 worker 调度——属测试生命周期缺陷而非产品缺陷。
// 屏蔽懒加载后本文件不再产生任何挂起的动态 import,判定确定。
vi.mock('../asyncModal', () => ({
  lazyModal: () => ({ name: 'StubSettingsSection', render: () => null }),
}));

import { useAppStore } from '../store';
import SettingsHub from './SettingsHub.vue';
import { STEP_COPY } from '../utils/tutorialCopy';
import type { SettingsNavRequest, TutorialPanelKey, TutorialStepId } from '../onboarding';

function mountHub() {
  setActivePinia(createPinia());
  return { store: useAppStore(), wrapper: mount(SettingsHub) };
}

describe('SettingsHub 组件(阶段 A 补测)', () => {
  beforeEach(() => {
    memStorage.clear();
  });

  it('挂载渲染设置中心容器与左侧导航', () => {
    const { wrapper } = mountHub();
    expect(wrapper.find('.sv-modal-mask').exists()).toBe(true);
    expect(wrapper.find('.sv-hub-nav').exists()).toBe(true);
    expect(wrapper.findAll('.sv-hub-nav-item').length).toBeGreaterThan(0);
  });

  it('一级域按钮带序号渲染(01/02…)', () => {
    const { wrapper } = mountHub();
    expect(wrapper.find('.sv-hub-nav-idx').text()).toBe('01');
  });

  it('点击一级域后该域呈 active 态', async () => {
    const { wrapper } = mountHub();
    const items = wrapper.findAll('.sv-hub-nav-item');
    expect(items.length).toBeGreaterThan(1);

    await items[1].trigger('click');
    expect(items[1].classes()).toContain('active');
  });

  it('点击二级 section 项切换右侧内容区(active 态转移)', async () => {
    const { wrapper } = mountHub();
    const subs = wrapper.findAll('.hub-sub-item');
    expect(subs.length).toBeGreaterThan(0);
    await subs[0].trigger('click');
    // section 项点击后应有一个二级项为 active
    const activeCount = wrapper.findAll('.hub-sub-item.active').length;
    expect(activeCount).toBe(1);
  });

  it('任务模式下「执行流程」入口可见(IFW-3 回归)', async () => {
    const { store, wrapper } = mountHub();
    store.appMode = 'task';
    await nextTick();

    // 「对话与提示词」域在任务模式下整域隐藏,故一级域下标与角色扮演模式不同,按文案定位
    const agentDomain = wrapper
      .findAll('.sv-hub-nav-item')
      .find((b) => b.text().includes('Agent 与任务'));
    expect(agentDomain).toBeTruthy();
    await agentDomain?.trigger('click');

    const labels = wrapper.findAll('.hub-sub-item').map((it) => it.text());
    expect(labels).toContain('执行流程');
  });

  it('任务模式下角色扮演专属 section 仍隐藏,但「对话与提示词」域的双模式入口可见(IMPHUB 步骤 C)', async () => {
    const { store, wrapper } = mountHub();
    store.appMode = 'task';
    await nextTick();

    // 「对话与提示词」域含双模式 action(综合导入 / 导出)→ 域整体保留;
    // 但角色扮演专属 section(提示词注入)不得出现
    const dialogDomain = wrapper
      .findAll('.sv-hub-nav-item')
      .find((b) => b.text().includes('对话与提示词'));
    expect(dialogDomain, '任务模式应有「对话与提示词」域(综合导入为两模式入口)').toBeTruthy();
    await dialogDomain?.trigger('click');
    const labels = wrapper.findAll('.hub-sub-item').map((it) => it.text());
    // action 项尾部带「→」箭头(提示点开是独立面板),故用包含判定
    expect(labels.some((l) => l.includes('综合导入 / 导出'))).toBe(true);
    expect(labels).not.toContain('提示词注入');
  });

  it('「授权与命令执行」是独立二级项且两种模式下都可见(2026-09-17:授权入口曾被埋在 Agent 设置内)', async () => {
    const { store, wrapper } = mountHub();
    for (const mode of ['roleplay', 'task'] as const) {
      store.appMode = mode;
      await nextTick();

      const agentDomain = wrapper
        .findAll('.sv-hub-nav-item')
        .find((b) => b.text().includes('Agent 与任务'));
      expect(agentDomain, `${mode} 模式应有「Agent 与任务」域`).toBeTruthy();
      await agentDomain?.trigger('click');

      const labels = wrapper.findAll('.hub-sub-item').map((it) => it.text());
      expect(labels, `${mode} 模式应能看到授权入口`).toContain('授权与命令执行');
    }
  });
});

// 分区定位请求(新手教程「带我去设置」,2026-09-27):openSettingsAt 置 uiPrefs.settingsNav,
// 本组件 watch 它并把两级导航落到目标分区。断言按右栏标题(「功能域 · 分区」)判定——
// 那是用户实际看到的定位结果,而不是内部 ref 的镜像。
describe('SettingsHub 分区定位(新手教程「带我去设置」)', () => {
  beforeEach(() => {
    memStorage.clear();
  });

  it('openSettingsAt 落到目标分区并同时切到其所属功能域', async () => {
    const { store, wrapper } = mountHub();
    store.openSettingsAt('exec');
    await nextTick();

    expect(store.settingsOpen).toBe(true);
    // 1 步跨域定位:授权与命令执行在「Agent 与任务」域下,而默认激活域是「连接与模型」
    expect(wrapper.find('.sv-hub-content-title').text()).toBe('Agent 与任务 · 授权与命令执行');
    const activeNav = wrapper.findAll('.sv-hub-nav-item.active').map((b) => b.text());
    expect(activeNav[0]).toContain('Agent 与任务');
  });

  it('请求先于挂载(首次「带我去设置」的真实路径):挂载后即定位到目标分区,并消费掉请求', async () => {
    setActivePinia(createPinia());
    const store = useAppStore();
    // 教程点击时设置弹窗尚未挂载(chunk 懒加载):请求先落到 store,mount 才发生
    store.openSettingsAt('exec');
    const wrapper = mount(SettingsHub);
    await nextTick();

    expect(wrapper.find('.sv-hub-content-title').text()).toBe('Agent 与任务 · 授权与命令执行');
    expect(store.settingsNav).toBeNull();
  });

  it('请求被消费后,用户自己再打开综合设置不会被上一次请求带着跳分区', async () => {
    setActivePinia(createPinia());
    const store = useAppStore();
    store.openSettingsAt('exec');
    const first = mount(SettingsHub);
    await nextTick();
    expect(first.find('.sv-hub-content-title').text()).toBe('Agent 与任务 · 授权与命令执行');

    // 用户关掉设置后自己再点「综合设置」(没有新请求)→ 落回默认分区,不沿用上次定位
    store.settingsOpen = false;
    await nextTick();
    const second = mount(SettingsHub);
    await nextTick();
    expect(second.find('.sv-hub-content-title').text()).toBe('连接与模型 · API 连接');
  });

  it('连续两次请求同一分区都生效(nonce 递增,不因值相同被跳过)', async () => {
    const { store, wrapper } = mountHub();
    store.openSettingsAt('exec');
    await nextTick();
    expect(wrapper.find('.sv-hub-content-title').text()).toBe('Agent 与任务 · 授权与命令执行');

    // 用户手动切走后再来一次同一分区:必须重新落回去
    const apiDomain = wrapper.findAll('.sv-hub-nav-item').find((b) => b.text().includes('连接与模型'));
    await apiDomain?.trigger('click');
    expect(wrapper.find('.sv-hub-content-title').text()).not.toBe('Agent 与任务 · 授权与命令执行');

    store.openSettingsAt('exec');
    await nextTick();
    expect(wrapper.find('.sv-hub-content-title').text()).toBe('Agent 与任务 · 授权与命令执行');
  });

  it('目标分区在当前模式下不可见时保持原分区(不改动、不报错)', async () => {
    const { store, wrapper } = mountHub();
    store.appMode = 'task';
    await nextTick();
    const before = wrapper.find('.sv-hub-content-title').text();

    // 「提示词注入」是角色扮演专属分区,任务模式下整域隐藏
    store.openSettingsAt('prompt');
    await nextTick();

    expect(wrapper.find('.sv-hub-content-title').text()).toBe(before);
  });
});

// TUTSYNC OB-9 + OB-12(2026-10-10):action 型落点与「教程每个落点必须落得下去」。
//
// 为什么这两组断言必须存在:设置二级项有两种形态——section(切内容区)与 action(打开独立面板)。
// 教程原先只支持 section,于是 IMPHUB 后的「综合导入 / 导出」「附件管理」根本没法带用户去;
// 而「教程引用的分区/面板改名或删掉」这件事此前**没有任何信号**(2026-10-10 实测:教程承诺的
// 「预设导入」早在 IMPHUB 步骤 C 被删,54 例测试全绿)。本组断言把「落点存在」变成机器约束。
describe('SettingsHub · action 型落点与教程落点穷尽校验', () => {
  beforeEach(() => {
    memStorage.clear();
  });

  /** panel 键 → 独立面板开关(store 门面键) */
  const PANEL_FLAG: Record<TutorialPanelKey, string> = {
    importBundle: 'importBundleOpen',
    assets: 'assetsOpen',
    memory: 'memoryOpen',
  };

  it('openSettingsPanel 执行同名 action:打开独立面板并关掉 Hub', async () => {
    const { store } = mountHub();
    store.openSettingsPanel('importBundle');
    await nextTick();

    expect(store.importBundleOpen).toBe(true);
    expect(store.settingsOpen).toBe(false); // openPanel 的既有语义:不叠两层弹窗
    expect(store.settingsNav).toBeNull(); // 请求已被消费
  });

  it('三种 panel 键都能落到各自的独立面板', async () => {
    for (const panel of ['importBundle', 'assets', 'memory'] as const) {
      setActivePinia(createPinia());
      const store = useAppStore();
      store.openSettingsPanel(panel);
      const wrapper = mount(SettingsHub);
      await nextTick();
      const flag = PANEL_FLAG[panel] as keyof ReturnType<typeof useAppStore>;
      expect(store[flag], `${panel} 没有对应的 action 项`).toBe(true);
      wrapper.unmount();
    }
  });

  it('找不到同名 action 时不报错(请求仍被消费)', async () => {
    setActivePinia(createPinia());
    const store = useAppStore();
    store.openSettingsPanel('assets');
    store.assetsOpen = false; // 模拟面板被用户随手关掉后,Hub 再次挂载不应重复弹面板
    const wrapper = mount(SettingsHub);
    await nextTick();
    expect(store.settingsNav).toBeNull();
    wrapper.unmount();
  });

  it('教程每个步骤的落点都存在:section 型标题逐字命中、panel 型面板打开', async () => {
    const navSteps = (Object.keys(STEP_COPY) as TutorialStepId[]).filter(
      (id) => STEP_COPY[id].nav,
    );
    // 断言「有落点的步骤」数量本身也是信号:落点集合变动时这里会提醒复核教程
    expect(navSteps.length).toBe(10);

    for (const id of navSteps) {
      setActivePinia(createPinia());
      const store = useAppStore();
      const nav = STEP_COPY[id].nav as SettingsNavRequest;
      const label = STEP_COPY[id].navLabel ?? '';
      if (nav.kind === 'section') store.openSettingsAt(nav.section);
      else store.openSettingsPanel(nav.panel);
      const wrapper = mount(SettingsHub);
      await nextTick();

      if (nav.kind === 'section') {
        // section 型:右栏标题 = 「功能域 · 分区」,应与教程写的 navLabel **逐字相同**
        expect(
          wrapper.find('.sv-hub-content-title').text(),
          `${id} 的 navLabel 与真实标题不符`,
        ).toBe(label);
      } else {
        // panel 型:能置起开关 = Hub 里存在同名 action 项(它被删掉时这里会红,
        // 因为 watch 找不到 item 就什么都不做)。navLabel 与 action 标签的逐字比对
        // 归 tools/check-tutorial.mjs(它读得到 SettingsHub 的 domains 字面量)。
        expect(
          store[PANEL_FLAG[nav.panel] as keyof ReturnType<typeof useAppStore>],
          `${id} 指向的独立面板不存在(action 项被删或改名?)`,
        ).toBe(true);
      }
      wrapper.unmount();
    }
  });
});
