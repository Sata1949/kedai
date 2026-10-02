// @vitest-environment jsdom
// UIP-12 输入模态标记契约(2026-10-02)。jsdom 派发真实 DOM 事件验证「标记写入」;
// CSS 消费点(pointer 模态下收窄 select 焦点)的源码断言在 styles/motion.test.ts
// ——jsdom 环境下 vitest 会把相对 URL 解析成 http://localhost,读不了本地 .css。
import { describe, expect, it, afterEach } from 'vitest';
import { installInputModality } from './inputModality';

let uninstall: (() => void) | null = null;
afterEach(() => {
  uninstall?.();
  uninstall = null;
  delete document.documentElement.dataset.inputMode;
});

describe('UIP-12 输入模态标记', () => {
  it('未安装时不产生任何标记(默认 = 现状行为)', () => {
    document.dispatchEvent(new Event('pointerdown'));
    document.dispatchEvent(new Event('keydown'));
    expect(document.documentElement.dataset.inputMode).toBeUndefined();
  });

  it('指针事件写入 pointer,键盘事件写入 keyboard(双向可切)', () => {
    uninstall = installInputModality();
    document.dispatchEvent(new Event('pointerdown'));
    expect(document.documentElement.dataset.inputMode).toBe('pointer');
    document.dispatchEvent(new Event('keydown'));
    expect(document.documentElement.dataset.inputMode).toBe('keyboard');
    document.dispatchEvent(new Event('pointerdown'));
    expect(document.documentElement.dataset.inputMode).toBe('pointer');
  });

  it('卸载后不再响应(返回的清理函数生效)', () => {
    const off = installInputModality();
    off();
    document.dispatchEvent(new Event('pointerdown'));
    expect(document.documentElement.dataset.inputMode).toBeUndefined();
  });
});
