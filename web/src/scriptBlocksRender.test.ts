// 脚本块渲染缓存包装单测(USERPERSONA-1 下沉后首次覆盖)。
// 只测「键纪律 + null 语义 + 命中不重算」三件易错事:
// 键漏了渲染输入 → 显示旧文本;null 当未命中 → 每次渲染都重跑全量正则。
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createScriptBlocksRender } from './scriptBlocksRender';
import { scriptBlocksCache } from './renderCache';
import type { ScopedRenderResult } from './render';

const RESULT: ScopedRenderResult = { html: '<p>正文</p>', blocks: [] };

function makeRender() {
  return vi.fn((..._args: Parameters<typeof import('./render').renderScopedScripts>) => RESULT);
}

describe('createScriptBlocksRender', () => {
  beforeEach(() => {
    scriptBlocksCache.clear();
  });

  it('命中缓存不重复渲染,并把称呼传进宏上下文', () => {
    const render = makeRender();
    const run = createScriptBlocksRender(
      { scriptHash: () => 'h1', charName: () => '芽衣', userName: () => '林' },
      render,
    );

    expect(run('正文', [], 'msg-1', 0)).toBe(RESULT);
    expect(run('正文', [], 'msg-1', 0)).toBe(RESULT);
    expect(render).toHaveBeenCalledTimes(1);
    expect(render.mock.calls[0][4]).toEqual({ charName: '芽衣', userName: '林' });
  });

  it('未命中任何脚本(null)是有效结果,同样被缓存', () => {
    const render = vi.fn((..._args: Parameters<typeof import('./render').renderScopedScripts>) => null);
    const run = createScriptBlocksRender(
      { scriptHash: () => 'h1', charName: () => '', userName: () => '用户' },
      render,
    );

    expect(run('正文', [], 'msg-2', 0)).toBeNull();
    expect(run('正文', [], 'msg-2', 0)).toBeNull();
    expect(render).toHaveBeenCalledTimes(1);
  });

  it('称呼 / 脚本指纹 / 深度 / 文本任一变化都换键(不得复用陈旧渲染)', () => {
    const render = makeRender();
    const state = { scriptHash: 'h1', userName: '林' };
    const run = createScriptBlocksRender({
      scriptHash: () => state.scriptHash,
      charName: () => '芽衣',
      userName: () => state.userName,
    }, render);

    run('正文', [], 'msg-1', 0);
    expect(render).toHaveBeenCalledTimes(1);

    state.userName = '晚';
    run('正文', [], 'msg-1', 0);
    expect(render, '称呼变化必须换键({{user}} 展开结果不同)').toHaveBeenCalledTimes(2);

    state.scriptHash = 'h2';
    run('正文', [], 'msg-1', 0);
    expect(render, '脚本指纹变化必须换键').toHaveBeenCalledTimes(3);

    run('正文', [], 'msg-1', 2);
    expect(render, 'depth 变化必须换键(脚本适用性不同)').toHaveBeenCalledTimes(4);

    run('另一条正文', [], 'msg-1', 2);
    expect(render, '文本变化必须换键').toHaveBeenCalledTimes(5);
  });
});
