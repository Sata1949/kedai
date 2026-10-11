// 显示层正则合并口径回归(IMPHUB ⑥):全局在前、卡级在后;空全局不引入抖动。
import { describe, expect, it } from 'vitest';
import type { RegexScript } from '../api';
import { mergedRegexScripts } from './regexMerge';

function script(id: string, name: string): RegexScript {
  return {
    id,
    script_name: name,
    find_regex: id,
    replace_string: '',
    markdown_only: false,
    enabled: true,
  };
}

describe('mergedRegexScripts(全局 + 卡级)', () => {
  it('顺序:全局在前、卡级在后(卡级更具体,后应用覆盖)', () => {
    const out = mergedRegexScripts([script('g', '全局')], [script('c', '卡级')]);
    expect(out.map((s) => s.id)).toEqual(['g', 'c']);
  });

  it('无全局库:返回卡级数组原引用(与历史行为逐字节一致,不引入缓存键抖动)', () => {
    const card = [script('c', '卡级')];
    expect(mergedRegexScripts([], card)).toBe(card);
    expect(mergedRegexScripts(null, card)).toBe(card);
    expect(mergedRegexScripts(undefined, card)).toBe(card);
  });

  it('无卡级库:返回全局新数组;两处皆空:空数组', () => {
    const g = [script('g', '全局')];
    expect(mergedRegexScripts(g, [])).toEqual(g);
    expect(mergedRegexScripts(g, [])).not.toBe(g); // 拷贝:调用方改写不污染 store
    expect(mergedRegexScripts([], [])).toEqual([]);
    expect(mergedRegexScripts(null, null)).toEqual([]);
  });
});
