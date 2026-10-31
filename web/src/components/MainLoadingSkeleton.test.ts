// @vitest-environment jsdom
// UIVIS-3/UI-C2 主区加载骨架的结构契约(2026-10-31)。
// jsdom 只断言结构与 aria 语义;视觉表现(呼吸动画、间距)是样式层的事,由
// uivisVisual.test.ts 的源码契约兜住。
import { describe, expect, it } from 'vitest';
import { mount } from '@vue/test-utils';
import MainLoadingSkeleton from './MainLoadingSkeleton.vue';

describe('MainLoadingSkeleton(UIVIS-3/UI-C2)', () => {
  const w = mount(MainLoadingSkeleton);

  it('带 role=status 与中文 aria-label(加载中语义可达)', () => {
    expect(w.attributes('role')).toBe('status');
    expect(w.attributes('aria-label')).toBe('正在加载');
  });

  it('渲染标题行 + 正文行 + 正文块三段轮廓(铺满主区,不是字段级细条)', () => {
    expect(w.find('.sv-main-skeleton-head').exists()).toBe(true);
    expect(w.findAll('.sv-main-skeleton-line').length).toBeGreaterThan(0);
    expect(w.find('.sv-main-skeleton-block').exists()).toBe(true);
    // 装饰性子元素全部对读屏隐藏,只留容器一个状态节点
    const decorated = w.findAll('[aria-hidden="true"]');
    expect(decorated.length).toBeGreaterThan(0);
  });

  it('零 props:chunk 到位前挂载,不接受任何外部输入', () => {
    expect(Object.keys(MainLoadingSkeleton.props ?? {}).length).toBe(0);
  });
});
