// @vitest-environment jsdom
// UIP-13 加载骨架组件(2026-10-02)。jsdom 只做结构与 aria 断言;样式表现为源码契约
// (见 ../styles/motion.test.ts 的 sv-skeleton-pulse 条目)与探针量测。
import { describe, expect, it } from 'vitest';
import { mount } from '@vue/test-utils';
import SkeletonBlock from './SkeletonBlock.vue';

describe('UIP-13 SkeletonBlock', () => {
  it('默认 3 行:容器 role=status,块 aria-hidden,末行收窄', () => {
    const w = mount(SkeletonBlock);
    expect(w.attributes('role')).toBe('status');
    expect(w.attributes('aria-label')).toBe('加载中');
    const lines = w.findAll('.sv-skeleton-line');
    expect(lines).toHaveLength(3);
    for (const l of lines) expect(l.attributes('aria-hidden')).toBe('true');
    // 末行收窄,其余行不限宽(容器定宽即可)
    expect(lines[2].attributes('style')).toContain('62%');
    expect(lines[0].attributes('style') ?? '').not.toContain('62%');
  });

  it('lines/width 可定制;单行不收窄;容器宽度透传', () => {
    const w = mount(SkeletonBlock, { props: { lines: 5, width: '160px' } });
    const lines = w.findAll('.sv-skeleton-line');
    expect(lines).toHaveLength(5);
    expect(lines[4].attributes('style')).toContain('62%');

    const single = mount(SkeletonBlock, { props: { lines: 1, width: '96px' } });
    expect(single.findAll('.sv-skeleton-line')).toHaveLength(1);
    expect(single.find('.sv-skeleton-line').attributes('style') ?? '').not.toContain('62%');
    expect(single.attributes('style')).toContain('96px');
  });
});
