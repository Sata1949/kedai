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

describe('FE-4 流式输出不冲掉渲染缓存', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    memStorage.clear();
    void useAppStore();
    renderMarkdownSpy.mockClear();
    renderScopedScriptsSpy.mockClear();
    messageHtmlCache.clear();
  });

  it('流式逐帧不写缓存:稳定历史条目零淘汰,终态帧才入缓存', async () => {
    // 预置一条稳定历史消息(模拟虚拟滚动外已渲染过的条目)
    mountItem(msg({ id: 1, content: '稳定历史消息' })).unmount();
    expect(messageHtmlCache.stats().size).toBe(1);

    vi.useFakeTimers();
    try {
      const w = mountItem(msg({ id: 2, content: '第一段', streaming: true }));
      const p = w.props('m') as UiMessage;
      // 连续 20 帧追加(每帧是一次不同的「历史前缀」)
      for (let i = 0; i < 20; i++) {
        p.content += `t${i}`;
        await w.vm.$nextTick(); // 先让 watcher 把 pendingText 推进到本帧
        vi.advanceTimersByTime(20); // 再推进节拍,触发本帧绘制
        await w.vm.$nextTick();
      }
      expect(w.text()).toContain('t19'); // 节流推进不丢字
      // 关键断言:流式帧一条都不入缓存(旧实现每帧入一条,8 秒回复约 480 帧
      // 会把 500 容量全换成该消息的历史前缀,稳定条目被挤光)
      expect(messageHtmlCache.stats().size, '流式帧不得占用缓存条目').toBe(1);
      expect(messageHtmlCache.stats().evictions, '稳定条目不得被挤出').toBe(0);
      // 终态:同文本转非流式 → 入缓存(size=2)
      p.streaming = false;
      await w.vm.$nextTick();
      expect(messageHtmlCache.stats().size, '终态帧才入缓存').toBe(2);
      const settledHtml = w.find('.sv-msg-bubble').html();
      w.unmount();

      // 重挂(非流式,同文本)→ 命中终态缓存:零重算,且 HTML 与终态逐字节全等
      // (流式光标等瞬态已随终态消失,此处比对的是缓存语义下的最终渲染)
      const before = renderMarkdownSpy.mock.calls.length;
      const fresh = mountItem(msg({ id: 2, content: p.content }));
      expect(renderMarkdownSpy.mock.calls.length, '终态帧入缓存后重挂应命中').toBe(before);
      expect(fresh.find('.sv-msg-bubble').html()).toBe(settledHtml);
      fresh.unmount();
    } finally {
      vi.useRealTimers();
    }
  });

  it('depth 缺省与 0 等价:缓存键与渲染入参同源(不得用缺省结果命中 depth=0 的键)', () => {
    // min_depth=1 的脚本在 0 层不适用;旧实现键按 0、渲染按 undefined(不过滤),
    // 同一键会命中「不过滤」的结果并复用给 depth=0 的渲染
    const script: RegexScript = { ...HTML_SCRIPT, id: 's2', min_depth: 1 };

    const lacking = mountItem(msg({ content: '正文<Bar/>尾巴' }), {
      renderHtml: true,
      scripts: [script],
      depth: undefined,
    });
    expect(lacking.html(), '缺省按 0 处理:min_depth=1 的脚本在 0 层不适用').not.toContain('class="bar"');
    lacking.unmount();

    const explicitZero = mountItem(msg({ content: '正文<Bar/>尾巴' }), {
      renderHtml: true,
      scripts: [script],
      depth: 0,
    });
    expect(explicitZero.html(), '显式 depth=0 与缺省同键同结果').not.toContain('class="bar"');
  });
});
