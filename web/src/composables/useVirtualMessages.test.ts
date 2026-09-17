import { describe, expect, it } from 'vitest';
import {
  createVirtualListState,
  ESTIMATED_HEIGHTS,
  ESTIMATED_HEIGHT_FALLBACK,
} from './useVirtualMessages';

// 虚拟滚动核心状态单测(纯逻辑,无 DOM;IO 胶水在 useVirtualMessages 内,浏览器侧行为)
// 覆盖:启用阈值、尾部常驻、占位高度(实测缓存/角色估算/兜底)、进出缓冲区切换、pin 与重置。

describe('createVirtualListState', () => {
  it('消息数不超过阈值时全部真实挂载(短会话零开销)', () => {
    const s = createVirtualListState({ threshold: 80, tailKeep: 30 });
    expect(s.isEnabled(80)).toBe(false);
    expect(s.isEnabled(81)).toBe(true);
    // 阈值边界(=80)不启用:任意下标均激活
    for (let i = 0; i < 80; i++) {
      expect(s.isActive(i + 1, i, 80)).toBe(true);
    }
  });

  it('超阈值后仅尾部 tailKeep 条默认激活,头部未上报可见的不挂载', () => {
    const s = createVirtualListState({ threshold: 80, tailKeep: 30 });
    const count = 200;
    // 尾部 30 条(下标 170..199)常驻真实挂载
    expect(s.isActive(200, 199, count)).toBe(true);
    expect(s.isActive(171, 170, count)).toBe(true);
    // 尾部窗口之外(下标 169 及以前)未进入过视口 → 占位
    expect(s.isActive(170, 169, count)).toBe(false);
    expect(s.isActive(1, 0, count)).toBe(false);
  });

  it('markVisible 进入缓冲区后挂载;markHidden 记录实测高度后降级占位', () => {
    const s = createVirtualListState({ threshold: 10, tailKeep: 5 });
    const count = 100;
    expect(s.isActive(42, 41, count)).toBe(false);
    s.markVisible(42);
    expect(s.isActive(42, 41, count)).toBe(true);
    // 离开缓冲区:实测高度入缓存,行降级为占位
    s.markHidden(42, 233);
    expect(s.isActive(42, 41, count)).toBe(false);
    expect(s.placeholderHeight(42, 'assistant')).toBe(233);
  });

  it('占位高度:未渲染过的按角色估算,未知角色走兜底', () => {
    const s = createVirtualListState();
    expect(s.placeholderHeight(1, 'user')).toBe(ESTIMATED_HEIGHTS.user);
    expect(s.placeholderHeight(2, 'assistant')).toBe(ESTIMATED_HEIGHTS.assistant);
    expect(s.placeholderHeight(3, 'system')).toBe(ESTIMATED_HEIGHTS.system);
    expect(s.placeholderHeight(4, 'unknown-role')).toBe(ESTIMATED_HEIGHT_FALLBACK);
  });

  it('实测高度优先于角色估算;markHidden 忽略非正高度', () => {
    const s = createVirtualListState({ threshold: 10, tailKeep: 5 });
    s.markVisible(7);
    s.markHidden(7, 0); // 元素尚未排版完成:不写缓存,但仍降级占位
    expect(s.isActive(7, 6, 100)).toBe(false);
    expect(s.placeholderHeight(7, 'user')).toBe(ESTIMATED_HEIGHTS.user);
    s.markVisible(7);
    s.markHidden(7, 120);
    expect(s.placeholderHeight(7, 'user')).toBe(120);
  });

  it('pinnedId(编辑中的消息)强制真实挂载,与可见集无关', () => {
    const s = createVirtualListState({ threshold: 10, tailKeep: 5 });
    const count = 100;
    expect(s.isActive(9, 8, count, 9)).toBe(true);
    // 其他行不受 pin 影响
    expect(s.isActive(8, 7, count, 9)).toBe(false);
    // pin 解除后按常规规则判定
    expect(s.isActive(9, 8, count, null)).toBe(false);
  });

  it('列表长度变化时尾部窗口跟随(新消息进入尾部即真实挂载)', () => {
    const s = createVirtualListState({ threshold: 80, tailKeep: 30 });
    // 200 条时窗口为下标 170..199;追加一条(201 条)后窗口滑动为 171..200:
    // 原下标 170 退出常驻窗口(未上报可见 → 占位),新消息(下标 200)常驻真实挂载
    expect(s.isActive(171, 170, 200)).toBe(true);
    expect(s.isActive(171, 170, 201)).toBe(false);
    expect(s.isActive(202, 200, 201)).toBe(true);
  });

  it('reset 清空可见集与高度缓存(会话切换)', () => {
    const s = createVirtualListState({ threshold: 10, tailKeep: 5 });
    s.markVisible(42);
    s.markHidden(42, 233);
    s.markVisible(43);
    s.reset();
    expect(s.visible.size).toBe(0);
    expect(s.heights.size).toBe(0);
    expect(s.isActive(43, 42, 100)).toBe(false);
    expect(s.placeholderHeight(42, 'assistant')).toBe(ESTIMATED_HEIGHTS.assistant);
  });
});

// 条数窗口(2026-09-17 P-9):固定像素窗口对超高卡片覆盖不足——单条高度就可能吃掉整个
// overscanPx(默认 1200),导致滚动时相邻条目反复挂载/卸载。条数窗口与像素窗口取较大者。
describe('createVirtualListState 条数窗口(rowWindow)', () => {
  /** 造一份连续 id 的列表顺序(下标 = id - 1) */
  function syncRange(s: ReturnType<typeof createVirtualListState>, ids: number[]): void {
    s.syncOrder(ids);
  }

  it('默认 rowWindow 为 8', () => {
    const s = createVirtualListState();
    expect(s.rowWindow).toBe(8);
  });

  it('缓冲区内的行其近邻也挂载(与像素窗口取较大者)', () => {
    const ids = Array.from({ length: 200 }, (_, i) => i + 1);
    const s = createVirtualListState({ threshold: 80, tailKeep: 30, rowWindow: 8 });
    syncRange(s, ids);
    // 仅下标 50 被 IO 上报可见
    s.markVisible(51);
    // 上下各 8 条内 → 挂载
    expect(s.isActive(44, 43, 200)).toBe(true); // 距 50 有 7
    expect(s.isActive(59, 58, 200)).toBe(true); // 距 50 有 8
    // 窗口外 → 仍占位
    expect(s.isActive(42, 41, 200)).toBe(false); // 距 50 有 9
    expect(s.isActive(60, 59, 200)).toBe(false); // 距 50 有 9
  });

  it('rowWindow=0 时关闭条数窗口,退化为纯像素窗口行为', () => {
    const ids = Array.from({ length: 200 }, (_, i) => i + 1);
    const s = createVirtualListState({ threshold: 80, tailKeep: 30, rowWindow: 0 });
    syncRange(s, ids);
    s.markVisible(51);
    expect(s.isActive(51, 50, 200)).toBe(true); // 自身仍挂载
    expect(s.isActive(52, 51, 200)).toBe(false); // 近邻不再搭车
  });

  it('未同步顺序时条数窗口安全降级(不误挂载)', () => {
    const s = createVirtualListState({ threshold: 80, tailKeep: 30, rowWindow: 8 });
    s.markVisible(51); // 未 syncOrder → 无下标映射
    expect(s.isActive(51, 50, 200)).toBe(true);
    expect(s.isActive(52, 51, 200)).toBe(false);
  });

  it('窗口随列表顺序更新(消息插入后下标变化仍正确)', () => {
    const s = createVirtualListState({ threshold: 80, tailKeep: 30, rowWindow: 2 });
    const base = Array.from({ length: 200 }, (_, i) => i + 1);
    syncRange(s, base);
    s.markVisible(5); // 下标 4
    expect(s.isActive(7, 6, 200)).toBe(true); // 距 2 → 窗口内
    expect(s.isActive(8, 7, 200)).toBe(false); // 距 3 → 窗口外

    // 头部插入两条(id 900/901)后,所有原 id 下标 +2:id=5 变为下标 6
    syncRange(s, [900, 901, ...base]);
    // id=8 现在下标 9,距 6 有 3 → 超出窗口 2(若沿用旧下标会误判为窗口内)
    expect(s.isActive(8, 9, 202)).toBe(false);
    // id=7 现在下标 8,距 6 有 2 → 仍在窗口内
    expect(s.isActive(7, 8, 202)).toBe(true);
  });

  it('reset 同时清空顺序映射', () => {
    const s = createVirtualListState({ threshold: 80, tailKeep: 30, rowWindow: 8 });
    syncRange(s, [1, 2, 3]);
    s.markVisible(1);
    s.reset();
    s.markVisible(2);
    // 顺序已清空 → 窗口不生效,仅 2 自身挂载
    expect(s.isActive(2, 1, 200)).toBe(true);
    expect(s.isActive(1, 0, 200)).toBe(false);
  });
});
