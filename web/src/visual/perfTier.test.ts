// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';
import { applyPerfTier, resolvePerfTier } from './perfTier';

describe('perfTier 性能档位', () => {
  it('桌面高配走 high', () => {
    expect(resolvePerfTier({ cores: 8, deviceMemory: 8, mobile: false })).toBe('high');
  });

  it('低核数 / 低内存 / 移动端走 low', () => {
    expect(resolvePerfTier({ cores: 4, deviceMemory: 8, mobile: false })).toBe('low');
    expect(resolvePerfTier({ cores: 8, deviceMemory: 4, mobile: false })).toBe('low');
    expect(resolvePerfTier({ cores: 16, deviceMemory: 16, mobile: true })).toBe('low');
  });

  it('缺失能力信息时按高配处理,不误伤桌面', () => {
    expect(resolvePerfTier({})).toBe('high');
    expect(resolvePerfTier({ cores: 8 })).toBe('high');
  });

  it('applyPerfTier 写 html[data-perf-tier]', () => {
    const root = document.createElement('html');
    applyPerfTier(root, 'low');
    expect(root.dataset.perfTier).toBe('low');
    applyPerfTier(null, 'high');
    expect(root.dataset.perfTier).toBe('low');
  });
});
