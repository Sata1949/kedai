// @vitest-environment jsdom
// ExitConfirmModal 组件测试(2026-09-16 批次 5):退出确认从 Tauri 原生对话框
// 改为前端自绘弹窗,弹窗行为与「取消要复位壳兜底标记」的契约在此锁定。
//
// 覆盖:
//   - 至上主义结构(黑条头 + 装饰方块/斜线 + 后果说明)确实渲染;
//   - 「取消」:关弹窗 + 发 kedai://close-cancelled(缺此事件壳会把下一次关闭直接放行);
//   - 「退出」:发 kedai://exit-app;
//   - 壳事件接线是源码级契约:App.vue 必须 listen 且 onUnmounted 注销(否则重复弹窗)。
import { beforeEach, describe, expect, it, vi } from 'vitest';
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

// vi.mock 工厂被提升到文件顶部,不能引用外部变量;断言时经 vi.mocked 取用。
// 组件经动态 import 调 emit,故 mock 该模块即可。
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn().mockResolvedValue(undefined) }));

import { emit } from '@tauri-apps/api/event';
import { useUiPrefsStore } from '../stores/uiPrefs';
import { MODALS, MODAL_FLAGS } from '../modals';
import ExitConfirmModal from './ExitConfirmModal.vue';
// 源码级接线契约(?raw 由 vite 提供,仓内既有先例:modals.test.ts)
import appSource from '../App.vue?raw';

function mountModal() {
  setActivePinia(createPinia());
  const uiPrefs = useUiPrefsStore();
  const wrapper = mount(ExitConfirmModal);
  return { uiPrefs, wrapper };
}

/** 组件里 emit 走动态 import(@tauri-apps/api/event):模块解析跨真实异步边界,
 *  单纯 await Promise.resolve() 刷不干净,断言会看到 0 次调用。
 *  用 vi.waitFor 轮询到调用落地,避免写成「依赖微任务轮数」的脆弱断言。 */
async function flush(): Promise<void> {
  await vi.waitFor(() => {
    if (vi.mocked(emit).mock.calls.length === 0) throw new Error('emit 尚未落地');
  });
}

describe('ExitConfirmModal 渲染(至上主义语言)', () => {
  beforeEach(() => {
    memStorage.clear();
    vi.mocked(emit).mockClear();
  });

  it('渲染黑条头部 + 红色装饰方块 + 后果说明', () => {
    const { wrapper } = mountModal();
    const html = wrapper.html();
    expect(html).toContain('sv-modal-mask');
    expect(html).toContain('sv-modal');
    expect(html).toContain('sv-modal-head');
    expect(html).toContain('sv-supreme red');
    expect(html).toContain('退出 Kedai');
    // 后果说明:两平台共用同一文案,用户点之前就知道代价
    expect(html).toContain('未保存的编辑内容将丢失');
    expect(html).toContain('正在生成或执行中的任务会被中断');
    // 页脚按钮:取消(primary)/ 退出(danger)
    expect(html).toContain('sv-btn primary');
    expect(html).toContain('sv-btn danger');
  });
});

describe('ExitConfirmModal 行为', () => {
  beforeEach(() => {
    memStorage.clear();
    vi.mocked(emit).mockClear();
  });

  it('点「取消」:关闭弹窗并通知壳复位二次关闭兜底', async () => {
    const { uiPrefs, wrapper } = mountModal();
    uiPrefs.exitConfirmOpen = true;
    const buttons = wrapper.findAll('button');
    const cancelBtn = buttons.find((b) => b.text() === '取消');
    expect(cancelBtn, '应有「取消」按钮').toBeTruthy();
    await cancelBtn!.trigger('click');
    await flush();
    expect(uiPrefs.exitConfirmOpen).toBe(false);
    expect(vi.mocked(emit)).toHaveBeenCalledWith('kedai://close-cancelled');
  });

  it('点「退出」:发 kedai://exit-app,且不发取消事件', async () => {
    const { uiPrefs, wrapper } = mountModal();
    uiPrefs.exitConfirmOpen = true;
    const confirmBtn = wrapper.findAll('button').find((b) => b.text() === '退出');
    expect(confirmBtn, '应有「退出」按钮').toBeTruthy();
    await confirmBtn!.trigger('click');
    await flush();
    const calls = vi.mocked(emit).mock.calls.map((c) => c[0]);
    expect(calls).toContain('kedai://exit-app');
    expect(calls).not.toContain('kedai://close-cancelled');
  });

  it('✕ 关闭按钮与「取消」同义(也通知壳复位兜底)', async () => {
    const { uiPrefs, wrapper } = mountModal();
    uiPrefs.exitConfirmOpen = true;
    const closeBtn = wrapper.findAll('button').find((b) => b.text() === '✕');
    expect(closeBtn, '应有 ✕ 关闭按钮').toBeTruthy();
    await closeBtn!.trigger('click');
    await flush();
    expect(uiPrefs.exitConfirmOpen).toBe(false);
    expect(vi.mocked(emit)).toHaveBeenCalledWith('kedai://close-cancelled');
  });

  it('点遮罩空白处等同取消(不致弹窗卡住不可退)', async () => {
    const { uiPrefs, wrapper } = mountModal();
    uiPrefs.exitConfirmOpen = true;
    await wrapper.find('.sv-modal-mask').trigger('click');
    await flush();
    expect(uiPrefs.exitConfirmOpen).toBe(false);
  });
});

describe('退出确认的注册表与壳事件接线(源码级契约)', () => {
  it('exitConfirmOpen 已进弹窗注册表,且声明在末尾(叠在最上层)', () => {
    expect(MODAL_FLAGS).toContain('exitConfirmOpen');
    const decl = MODALS.find((m) => m.flag === 'exitConfirmOpen');
    expect(decl, 'exitConfirmOpen 应在 MODALS 中').toBeTruthy();
    expect(decl!.label).toBe('退出确认');
    expect(MODAL_FLAGS[MODAL_FLAGS.length - 1]).toBe('exitConfirmOpen');
  });

  it('App.vue 监听壳下发的关闭请求并在卸载时注销监听', () => {
    expect(appSource).toContain('kedai://close-requested');
    expect(appSource).toContain('listen(');
    // 必须注销:热更新/重挂载累积监听器会让一次关闭弹出多个确认框
    expect(appSource).toContain('unregisterCloseRequestListener');
    expect(appSource).toMatch(/onUnmounted\(\s*\(\)\s*=>\s*\{[\s\S]*unregisterCloseRequestListener\(\)/);
  });

  it('Android 返回键无处可退时改为打开同一确认弹窗(不再直接退出)', () => {
    const back = appSource.slice(appSource.indexOf('async function handleAndroidBack'));
    const body = back.slice(0, back.indexOf('\n}'));
    expect(body).toContain('store.exitConfirmOpen = true');
    expect(body, '返回键分支不应再直接发退出事件').not.toContain('kedai://exit-app');
  });
});
