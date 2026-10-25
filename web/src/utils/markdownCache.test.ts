import { beforeEach, describe, expect, it } from 'vitest';
import {
  clearMarkdownRenderCache,
  markdownRenderCacheSize,
  renderMarkdownCached,
} from './markdownCache';

describe('markdownCache（C-2A 任务面板共享预渲染缓存）', () => {
  beforeEach(() => {
    clearMarkdownRenderCache();
  });

  it('同一原文命中缓存:输出一致且条目数不增长', () => {
    const first = renderMarkdownCached('**加粗**');
    const second = renderMarkdownCached('**加粗**');
    expect(first).toContain('<strong>加粗</strong>');
    expect(second).toBe(first);
    expect(markdownRenderCacheSize()).toBe(1);
  });

  it('不同原文各占一条;空串不占缓存位', () => {
    renderMarkdownCached('# 一');
    renderMarkdownCached('# 二');
    expect(renderMarkdownCached('')).toBe('');
    expect(markdownRenderCacheSize()).toBe(2);
  });

  it('clear 后重新计数', () => {
    renderMarkdownCached('内容');
    clearMarkdownRenderCache();
    expect(markdownRenderCacheSize()).toBe(0);
  });
});
