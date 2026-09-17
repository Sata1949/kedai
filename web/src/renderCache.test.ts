// 渲染结果有界缓存单测(2026-09-17 P-9)。
//
// 背景:虚拟滚动离开缓冲区即卸载消息组件,滚回时组件重新挂载 → 组件内 computed 缓存随
// 实例销毁,同一条消息来回滚动会反复付 renderScopedScripts / markdown / sanitize 成本
// (参照:sanitize-html 处理 16.8k 字符 ≈ 3.057ms)。故把渲染结果缓存提到模块级。
// 本文件只测缓存容器语义(容量/FIFO/统计);「卸载重挂零重算」的端到端语义
// 由 ChatMessageItem.renderCache.test.ts 断言。
import { describe, expect, it } from 'vitest';
import { createBoundedCache, DEFAULT_RENDER_CACHE_LIMIT } from './renderCache';

describe('createBoundedCache', () => {
  it('命中与未命中分别计数,未命中返回 undefined', () => {
    const c = createBoundedCache<string>(10);
    expect(c.get('a')).toBeUndefined();
    expect(c.stats().misses).toBe(1);
    c.set('a', 'A');
    expect(c.get('a')).toBe('A');
    expect(c.stats().hits).toBe(1);
    expect(c.stats().misses).toBe(1);
  });

  it('可缓存 null(值为 null 不等于未命中)', () => {
    // scriptBlocksCache 存的是 renderScopedScripts 的返回值,未命中脚本时它就是 null;
    // 若把 null 当 miss,每次渲染都会重算「确认无脚本命中」这条贵路径。
    const c = createBoundedCache<string | null>(10);
    c.set('k', null);
    expect(c.get('k')).toBeNull();
    expect(c.stats().hits).toBe(1);
    expect(c.stats().misses).toBe(0);
  });

  it('超出容量按 FIFO 淘汰最早写入的条目', () => {
    const c = createBoundedCache<number>(3);
    c.set('a', 1);
    c.set('b', 2);
    c.set('c', 3);
    c.set('d', 4); // 淘汰 a
    expect(c.get('a')).toBeUndefined();
    expect(c.get('b')).toBe(2);
    expect(c.get('c')).toBe(3);
    expect(c.get('d')).toBe(4);
    expect(c.stats().size).toBe(3);
    expect(c.stats().evictions).toBe(1);
  });

  it('覆盖写入已有键不重复占位(不因改写触发淘汰)', () => {
    const c = createBoundedCache<number>(2);
    c.set('a', 1);
    c.set('b', 2);
    c.set('a', 11); // 改写 a:不应把容量顶爆
    expect(c.stats().size).toBe(2);
    expect(c.stats().evictions).toBe(0);
    expect(c.get('a')).toBe(11);
    expect(c.get('b')).toBe(2);
  });

  it('容量为 1 时只保留最后写入的条目', () => {
    const c = createBoundedCache<number>(1);
    c.set('a', 1);
    c.set('b', 2);
    expect(c.get('a')).toBeUndefined();
    expect(c.get('b')).toBe(2);
    expect(c.stats().size).toBe(1);
  });

  it('clear 清空条目并归零统计(测试隔离用)', () => {
    const c = createBoundedCache<string>(10);
    c.set('a', 'A');
    c.get('a');
    c.get('missing');
    c.clear();
    expect(c.stats()).toEqual({ hits: 0, misses: 0, evictions: 0, size: 0 });
    expect(c.get('a')).toBeUndefined();
  });

  it('默认容量为 500(有界,不得无上限)', () => {
    expect(DEFAULT_RENDER_CACHE_LIMIT).toBe(500);
    const c = createBoundedCache<number>();
    for (let i = 0; i < DEFAULT_RENDER_CACHE_LIMIT + 50; i++) c.set(`k${i}`, i);
    expect(c.stats().size).toBe(DEFAULT_RENDER_CACHE_LIMIT);
    expect(c.stats().evictions).toBe(50);
    // 最早的 50 条被淘汰,最新的仍在
    expect(c.get('k0')).toBeUndefined();
    expect(c.get(`k${DEFAULT_RENDER_CACHE_LIMIT + 49}`)).toBe(DEFAULT_RENDER_CACHE_LIMIT + 49);
  });
});
