// 世界书保存差集(WBCONC 提交 2):删除/新增必须由基线差集显式给出。
import { describe, expect, it } from 'vitest';
import { diffEntryIds, isRenderableEntry } from './worldBookEntryDiff';

describe('diffEntryIds', () => {
  it('基线有、当前没有 → removed;当前有、基线没有 → added', () => {
    expect(diffEntryIds([0, 1, 2], [0, 2, 3])).toEqual({
      removed: [1],
      added: [3],
    });
  });

  it('纯叠加(当前是基线的子集)不产生 added;纯删除不产生 removed 之外的项', () => {
    expect(diffEntryIds([0, 1], [0, 1, 2])).toEqual({ removed: [], added: [2] });
    expect(diffEntryIds([0, 1, 2], [0, 2])).toEqual({ removed: [1], added: [] });
  });

  it('空基线(新建后首次打开)→ 当前所有 id 都是 added', () => {
    expect(diffEntryIds([], [0, 1])).toEqual({ removed: [], added: [0, 1] });
  });
});

describe('isRenderableEntry', () => {
  it('content / keys / regex 全空 → 不可入库(GET 会过滤)', () => {
    expect(isRenderableEntry({ content: '', keys: [], regex: null })).toBe(false);
    expect(isRenderableEntry({ content: '', keys: [''], regex: '' })).toBe(false);
  });

  it('任一字段非空 → 可入库', () => {
    expect(isRenderableEntry({ content: '正文', keys: [], regex: null })).toBe(true);
    expect(isRenderableEntry({ content: '', keys: ['关键词'], regex: null })).toBe(true);
    expect(isRenderableEntry({ content: '', keys: [], regex: '\\d+' })).toBe(true);
  });
});
