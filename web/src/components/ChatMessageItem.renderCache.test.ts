// @vitest-environment jsdom
// ChatMessageItem 渲染缓存跨实例复用测试(2026-09-17 P-9)。
//
// 断言的核心语义:**组件卸载后重新挂载,同一消息内容不得重算渲染**。
// 这正是虚拟滚动场景——离开缓冲区的行会被卸载(`v-if` 降级为占位 div),滚回来重新挂载;
// 修复前 `html` 是组件内 computed,缓存随实例销毁,来回滚动反复付
// renderMarkdown / renderScopedScripts(含 sanitize-html)的成本。
//
// 反向断言同样重要:内容变化、renderHtml 开关变化、scriptHash 变化都必须**重算**,
// 否则界面会显示陈旧内容(这是本批次唯一真风险)。
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
  get length() {
    return memStorage.size;
  },
});

// 计数包装:保持真实渲染行为,只统计调用次数
const renderMarkdownSpy = vi.fn();
const renderScopedScriptsSpy = vi.fn();

vi.mock('../markdown', async (importOriginal) => {
  const orig = await importOriginal<typeof import('../markdown')>();
  return {
    ...orig,
    renderMarkdown: (text: string) => {
      renderMarkdownSpy(text);
      return orig.renderMarkdown(text);
    },
  };
});

vi.mock('../render', async (importOriginal) => {
  const orig = await importOriginal<typeof import('../render')>();
  return {
    ...orig,
    renderScopedScripts: (...args: Parameters<typeof orig.renderScopedScripts>) => {
      renderScopedScriptsSpy(...args);
      return orig.renderScopedScripts(...args);
    },
  };
});

import ChatMessageItem from './ChatMessageItem.vue';
import { useAppStore } from '../store';
import { messageHtmlCache } from '../renderCache';
import type { UiMessage } from '../sseReducer';
import type { RegexScript } from '../api';

function msg(over: Partial<UiMessage> = {}): UiMessage {
  return { id: 1, role: 'assistant', content: '你好', extra: {}, ...over } as UiMessage;
}

function mountItem(m: UiMessage, over: Record<string, unknown> = {}) {
  return mount(ChatMessageItem, {
    props: {
      m,
      editing: false,
      avatarUrl: null,
      characterName: '测试角色',
      renderHtml: false,
      scripts: [],
      depth: 0,
      scriptHash: 'h0',
      ...over,
    },
  });
}

/** 一条会命中 scripts 分支的正则脚本(替换串含宏,便于验证 charName 参与缓存键) */
const HTML_SCRIPT: RegexScript = {
  id: 's1',
  script_name: '状态栏',
  enabled: true,
  find_regex: '<Bar/>',
  replace_string: '<div class="bar">{{char}}</div>',
  markdown_only: false,
};

describe('ChatMessageItem 渲染缓存跨实例复用', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    memStorage.clear();
    void useAppStore();
    renderMarkdownSpy.mockClear();
    renderScopedScriptsSpy.mockClear();
    messageHtmlCache.clear();
  });

  it('卸载后重新挂载同一内容:markdown 分支零重算', () => {
    const m = msg({ content: '**加粗**文本' });
    const first = mountItem(m);
    expect(renderMarkdownSpy).toHaveBeenCalledTimes(1);
    const firstHtml = first.find('.sv-msg-bubble').html();
    first.unmount();

    const second = mountItem(m);
    // 关键断言:重挂后未再调用渲染函数,且输出与首次一致
    expect(renderMarkdownSpy).toHaveBeenCalledTimes(1);
    expect(second.find('.sv-msg-bubble').html()).toBe(firstHtml);
  });

  it('卸载后重新挂载同一内容:scoped 脚本分支零重算', () => {
    const m = msg({ content: '正文<Bar/>尾巴' });
    const first = mountItem(m, { renderHtml: true, scripts: [HTML_SCRIPT] });
    expect(renderScopedScriptsSpy).toHaveBeenCalledTimes(1);
    const firstHtml = first.find('.sv-msg-bubble').html();
    first.unmount();

    const second = mountItem(m, { renderHtml: true, scripts: [HTML_SCRIPT] });
    expect(renderScopedScriptsSpy).toHaveBeenCalledTimes(1);
    expect(second.find('.sv-msg-bubble').html()).toBe(firstHtml);
  });

  it('内容变化必须重算(不得命中陈旧条目)', () => {
    const first = mountItem(msg({ content: '第一版内容' }));
    expect(renderMarkdownSpy).toHaveBeenCalledTimes(1);
    first.unmount();

    // 同 id 但内容不同:键含文本 → 必 miss
    const second = mountItem(msg({ content: '第二版内容' }));
    expect(renderMarkdownSpy).toHaveBeenCalledTimes(2);
    expect(second.text()).toContain('第二版内容');
  });

  it('renderHtml 开关变化必须重算', () => {
    mountItem(msg({ content: '正文<Bar/>尾巴' }), { renderHtml: true, scripts: [HTML_SCRIPT] });
    const scopedCalls = renderScopedScriptsSpy.mock.calls.length;
    // 同内容关掉 HTML 渲染:走 markdown 分支,不得复用 scoped 结果
    const w2 = mountItem(msg({ content: '正文<Bar/>尾巴' }), { renderHtml: false, scripts: [HTML_SCRIPT] });
    expect(renderMarkdownSpy).toHaveBeenCalled();
    expect(w2.html()).not.toContain('class="bar"');
    expect(scopedCalls).toBeGreaterThan(0);
  });

  it('scriptHash 变化必须重算(脚本热更新即时生效)', () => {
    mountItem(msg({ content: '正文<Bar/>尾巴' }), {
      renderHtml: true, scripts: [HTML_SCRIPT], scriptHash: 'h1',
    });
    expect(renderScopedScriptsSpy).toHaveBeenCalledTimes(1);
    renderScopedScriptsSpy.mockClear();

    // 脚本内容变了(hash 变)但 scripts 引用与文本未变 → 必须重算
    mountItem(msg({ content: '正文<Bar/>尾巴' }), {
      renderHtml: true, scripts: [HTML_SCRIPT], scriptHash: 'h2',
    });
    expect(renderScopedScriptsSpy).toHaveBeenCalledTimes(1);
  });

  it('depth 变化必须重算(脚本适用性随楼层深度不同)', () => {
    mountItem(msg({ content: '正文<Bar/>尾巴' }), {
      renderHtml: true, scripts: [HTML_SCRIPT], depth: 0,
    });
    expect(renderScopedScriptsSpy).toHaveBeenCalledTimes(1);
    renderScopedScriptsSpy.mockClear();

    mountItem(msg({ content: '正文<Bar/>尾巴' }), {
      renderHtml: true, scripts: [HTML_SCRIPT], depth: 5,
    });
    expect(renderScopedScriptsSpy).toHaveBeenCalledTimes(1);
  });

  it('长列表模拟:50 条来回滚动一遍再滚一遍,第二条零重算', () => {
    const items = Array.from({ length: 50 }, (_, i) =>
      msg({ id: i + 1, content: `第 ${i + 1} 条内容` }),
    );
    // 第一遍:逐条挂载/卸载
    for (const m of items) {
      mountItem(m).unmount();
    }
    expect(renderMarkdownSpy).toHaveBeenCalledTimes(50);

    // 第二遍:同内容同输入 → 全部命中,零新增重算
    for (const m of items) {
      mountItem(m).unmount();
    }
    expect(renderMarkdownSpy).toHaveBeenCalledTimes(50);
  });
});
