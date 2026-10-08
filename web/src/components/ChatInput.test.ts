// @vitest-environment jsdom
// ChatInput 组件测试(阶段 A 补测):390 行,此前无测试。输入栏是用户主输入路径,
// 并承载 Agent 模式切换(M5 已把直改 state 改为 setAgentMode action,此处锁死防回退)。
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

vi.mock('../api', async (importOriginal) => {
  const orig = await importOriginal<typeof import('../api')>();
  return {
    ...orig,
    listModels: vi.fn().mockResolvedValue([]),
    getSlashCommands: vi.fn().mockResolvedValue([]),
    listQuickReplies: vi.fn().mockResolvedValue([]),
  };
});

import { useAppStore } from '../store';
import ChatInput from './ChatInput.vue';
// 源码级契约(?raw 由 vite 提供,仓内先例:modals.test.ts / ExitConfirmModal.test.ts)。
// 注意读的是 .vue 而不是 .css:本仓 vitest 配置为 css:false,`?raw` 对 .css 返回**空串**
// (2026-09-27 实测 length=0),对 .vue/.ts 才拿得到文本。
import chatInputSource from './ChatInput.vue?raw';

function mountInput() {
  setActivePinia(createPinia());
  return { store: useAppStore(), wrapper: mount(ChatInput) };
}

describe('ChatInput 组件(阶段 A 补测)', () => {
  beforeEach(() => {
    memStorage.clear();
  });

  it('挂载渲染输入栏与发送按钮', () => {
    const { wrapper } = mountInput();
    expect(wrapper.find('.sv-inputbar').exists()).toBe(true);
    expect(wrapper.find('.sv-btn-send').exists()).toBe(true);
    expect(wrapper.find('.sv-inputbox').exists()).toBe(true);
  });

  it('模式按钮点击走 setAgentMode action(不由组件直改 state)', async () => {
    const { store, wrapper } = mountInput();
    const setSpy = vi.spyOn(store, 'setAgentMode');
    const modeButtons = wrapper.findAll('.sv-mode button');
    expect(modeButtons.length).toBeGreaterThan(0);

    await modeButtons[0].trigger('click');
    expect(setSpy).toHaveBeenCalled();
    // store 值同步更新(证明走的是真实 action 而非空壳)
    expect(['fast', 'deep', 'agent', 'custom']).toContain(store.agentMode);
  });

  it('生成中切换为「停止」按钮,不再显示发送(防重复提交)', async () => {
    const { store, wrapper } = mountInput();
    // 未生成:发送按钮在场、停止按钮不在
    expect(wrapper.find('.sv-btn-send').exists()).toBe(true);
    expect(wrapper.find('.sv-btn-send.stop').exists()).toBe(false);

    store.generating = true;
    await wrapper.vm.$nextTick();
    // 生成中:替换为停止按钮
    expect(wrapper.find('.sv-btn-send.stop').exists()).toBe(true);
    expect(wrapper.find('.sv-btn-send').attributes('title')).toBe('停止生成');
  });

  it('无角色或无输入时发送按钮禁用', () => {
    const { wrapper } = mountInput();
    // 初始无 currentCharacterId 且无输入 → 禁用
    expect(wrapper.find('.sv-btn-send').attributes('disabled')).toBeDefined();
  });

  it('停止中:停止按钮禁用并标注「正在停止」(SENDFIX-2 过渡态)', async () => {
    const { store, wrapper } = mountInput();
    store.generating = true;
    await wrapper.vm.$nextTick();
    store.stopping = true;
    await wrapper.vm.$nextTick();

    const btn = wrapper.find('.sv-btn-send.stop');
    expect(btn.exists()).toBe(true);
    expect(btn.attributes('disabled')).toBeDefined();
    expect(btn.attributes('title')).toBe('正在停止…');
  });
});

describe('ChatInput 发送失败草稿保留(SENDFIX-2)', () => {
  it('未被受理(409/网络等)时原文留在输入框,不清空', async () => {
    const { store, wrapper } = mountInput();
    store.currentCharacterId = 'c1';
    const sendSpy = vi.spyOn(store, 'sendMessage').mockResolvedValue({ accepted: false });

    await wrapper.find('textarea').setValue('这条会被拒');
    await wrapper.find('.sv-btn-send').trigger('click');
    await vi.waitFor(() => expect(sendSpy).toHaveBeenCalled());
    await wrapper.vm.$nextTick();

    expect((wrapper.find('textarea').element as HTMLTextAreaElement).value, '发送失败必须保留原文').toBe('这条会被拒');
  });

  it('受理成功后清空输入框(正常路径不受影响)', async () => {
    const { store, wrapper } = mountInput();
    store.currentCharacterId = 'c1';
    const sendSpy = vi.spyOn(store, 'sendMessage').mockResolvedValue({ accepted: true });

    await wrapper.find('textarea').setValue('正常发送');
    await wrapper.find('.sv-btn-send').trigger('click');
    await vi.waitFor(() => expect(sendSpy).toHaveBeenCalled());
    await vi.waitFor(() => expect((wrapper.find('textarea').element as HTMLTextAreaElement).value).toBe(''));
  });
});

// 为什么这条断言在测试里而不是靠人看:.sv-model-select 的三角是 data-URI SVG,只带 viewBox
// (无 width/height),缺 background-size 时浏览器按定位区高度渲染(2026-09-27 实测 35×35px,
// 而右侧只预留 22px) → 三角压住模型名。该缺陷长期没人发现,因为全仓没有任何门禁盯样式,
// 故用源码级断言把它钉住(样式再搬家时,断言目标要跟着搬)。
describe('模型选择器样式(.sv-model-select 的自绘三角)', () => {
  it('三角必须显式给定像素尺寸(缺 background-size 会被放大到元素全高)', () => {
    const styleBlock = /<style[^>]*>([\s\S]*?)<\/style>/.exec(chatInputSource)?.[1] ?? '';
    expect(styleBlock, '样式必须留在组件 scoped 块内(D-5)').not.toBe('');
    const rules = [...styleBlock.matchAll(/\.sv-model-select\s*\{([^}]*)\}/g)].map((m) => m[1]);
    const caretRule = rules.find((body) => body.includes('background-image'));
    expect(caretRule, '.sv-model-select 必须有一条带 background-image 的规则').toBeDefined();
    const size = /background-size:\s*([^;]+);/.exec(caretRule ?? '')?.[1]?.trim() ?? '';
    expect(size, 'background-size 必须是像素对;`auto` 会把 8×8 三角按元素全高渲染').toMatch(
      /^\d+px\s+\d+px$/,
    );
  });
});

describe('ChatInput 图像附件(视觉能力包 D2)', () => {
  it('图片只留 [图片: 名称] 标记且不拼 base64;载荷走 attachments;预览 URL 每附件一次且用完回收', async () => {
    const createUrl = vi.fn(() => 'blob:mock-1');
    const revokeUrl = vi.fn();
    Object.defineProperty(URL, 'createObjectURL', { value: createUrl, writable: true, configurable: true });
    Object.defineProperty(URL, 'revokeObjectURL', { value: revokeUrl, writable: true, configurable: true });

    const { store, wrapper } = mountInput();
    store.currentCharacterId = 'c1';
    const sendSpy = vi.spyOn(store, 'sendMessage').mockResolvedValue({ accepted: true });

    const file = new File([new Uint8Array([0x89, 0x50, 0x4e, 0x47])], '图标.png', { type: 'image/png' });
    const fileInput = wrapper.find('input[type="file"]');
    Object.defineProperty(fileInput.element, 'files', { value: [file], configurable: true });
    await fileInput.trigger('change');
    await wrapper.vm.$nextTick();

    // 预览条:同一附件在模板中两次取值仍只创建一次对象 URL(修复双倍泄漏)
    expect(wrapper.find('.sv-attach-chip img').exists()).toBe(true);
    expect(createUrl).toHaveBeenCalledTimes(1);

    await wrapper.find('textarea').setValue('看这张图');
    await wrapper.find('.sv-btn-send').trigger('click');
    // FileReader 异步读取:等待提交
    await vi.waitFor(() => expect(sendSpy).toHaveBeenCalled());

    const [content, payload] = sendSpy.mock.calls[0] as [
      string,
      Array<{ name: string; mime: string; data_url: string }>,
    ];
    expect(content).toContain('看这张图');
    expect(content).toContain('[图片: 图标.png]');
    expect(content).not.toContain('base64');
    expect(payload).toHaveLength(1);
    expect(payload[0].name).toBe('图标.png');
    expect(payload[0].mime).toBe('image/png');
    expect(payload[0].data_url.startsWith('data:image/png;base64,')).toBe(true);
    // 发送后回收预览 URL(不再泄漏);清空/回收发生在受理结果返回之后(SENDFIX-2),故等它落定
    await vi.waitFor(() => expect(revokeUrl).toHaveBeenCalledWith('blob:mock-1'));
  });

  it('移除附件后回收其预览 URL,且不再渲染该预览(FE-2 验收)', async () => {
    const createUrl = vi.fn(() => 'blob:mock-rm');
    const revokeUrl = vi.fn();
    Object.defineProperty(URL, 'createObjectURL', { value: createUrl, writable: true, configurable: true });
    Object.defineProperty(URL, 'revokeObjectURL', { value: revokeUrl, writable: true, configurable: true });

    const { wrapper } = mountInput();
    const file = new File([new Uint8Array([1, 2, 3])], '待移除.png', { type: 'image/png' });
    const fileInput = wrapper.find('input[type="file"]');
    Object.defineProperty(fileInput.element, 'files', { value: [file], configurable: true });
    await fileInput.trigger('change');
    await wrapper.vm.$nextTick();
    expect(wrapper.find('.sv-attach-chip img').exists()).toBe(true);
    expect(createUrl).toHaveBeenCalledTimes(1);

    await wrapper.find('.sv-attach-chip .remove').trigger('click');

    expect(revokeUrl).toHaveBeenCalledWith('blob:mock-rm');
    expect(wrapper.find('.sv-attach-chip').exists()).toBe(false);
  });

  it('组件卸载时回收未发送附件的预览 URL(FE-2 验收)', async () => {
    const createUrl = vi.fn(() => 'blob:mock-unmount');
    const revokeUrl = vi.fn();
    Object.defineProperty(URL, 'createObjectURL', { value: createUrl, writable: true, configurable: true });
    Object.defineProperty(URL, 'revokeObjectURL', { value: revokeUrl, writable: true, configurable: true });

    const { wrapper } = mountInput();
    const file = new File([new Uint8Array([9])], '未发送.png', { type: 'image/png' });
    const fileInput = wrapper.find('input[type="file"]');
    Object.defineProperty(fileInput.element, 'files', { value: [file], configurable: true });
    await fileInput.trigger('change');
    await wrapper.vm.$nextTick();
    expect(createUrl).toHaveBeenCalledTimes(1);
    expect(revokeUrl).not.toHaveBeenCalled();

    wrapper.unmount();

    expect(revokeUrl).toHaveBeenCalledWith('blob:mock-unmount');
  });
});
