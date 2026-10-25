// @vitest-environment jsdom
// C-2A：WorldBookEntryEditor 从 WorldBooksModal 两段重复模板中抽出后的契约测试。
import { describe, expect, it } from 'vitest';
import { mount } from '@vue/test-utils';
import WorldBookEntryEditor from './WorldBookEntryEditor.vue';
import type { WorldBookEntry } from '../api';

function makeEntry(overrides: Partial<WorldBookEntry> = {}): WorldBookEntry {
  return {
    id: 1,
    comment: '天气',
    keys: ['雨', '晴'],
    keys_secondary: ['风'],
    regex: null,
    use_regex: false,
    constant: false,
    enabled: true,
    content: '今天有雨',
    position: 1,
    depth: 4,
    scan_depth: null,
    order: 100,
    case_sensitive: false,
    sticky: 0,
    cooldown: 0,
    probability: 100,
    use_probability: false,
    role: null,
    ...overrides,
  };
}

function mountEditor(entry: WorldBookEntry, index = 0, total = 1) {
  return mount(WorldBookEntryEditor, {
    props: {
      entry,
      index,
      total,
      keysPlaceholder: '关键词占位',
      regexPlaceholder: '正则占位',
    },
  });
}

describe('WorldBookEntryEditor', () => {
  it('渲染条目全部字段与两套占位文案', () => {
    const wrapper = mountEditor(makeEntry());
    expect((wrapper.find('.sv-wb-comment').element as HTMLInputElement).value).toBe('天气');
    const textareas = wrapper.findAll('textarea');
    expect((textareas[0].element as HTMLTextAreaElement).value).toBe('今天有雨');
    const placeholders = wrapper.findAll('input').map((i) => i.attributes('placeholder'));
    expect(placeholders).toContain('关键词占位');
    expect(placeholders).toContain('正则占位');
  });

  it('文本 / 关键词 / 数字字段都只上抛 patch,不改 prop 对象', async () => {
    const entry = makeEntry();
    const wrapper = mountEditor(entry);
    await wrapper.find('.sv-wb-comment').setValue('新名称');
    const keysInput = wrapper.findAll('input').find((i) => i.attributes('placeholder') === '关键词占位');
    if (!keysInput) throw new Error('关键词输入框应存在');
    await keysInput.setValue('甲, 乙');
    await wrapper.find('input[type="number"]').setValue('8');

    const patches = wrapper.emitted('patch');
    expect(patches).toBeTruthy();
    expect(patches).toContainEqual(['comment', '新名称']);
    expect(patches).toContainEqual(['keys', ['甲', '乙']]);
    expect(patches).toContainEqual(['depth', 8]);
    // 组件只发事件,父级草稿对象保持原值
    expect(entry.comment).toBe('天气');
    expect(entry.depth).toBe(4);
  });

  it('常驻/激活/大小写/正则/概率按钮分别上抛事件或 patch', async () => {
    const wrapper = mountEditor(makeEntry({ regex: 'x' }));
    const buttons = wrapper.findAll('button');
    const byText = (t: string) => buttons.find((b) => b.text() === t);
    const constant = byText('触发');
    const enabled = byText('激活');
    const caseBtn = byText('Aa不敏');
    const regexBtn = byText('正则关');
    const probBtn = byText('概率关');
    if (!constant || !enabled || !caseBtn || !regexBtn || !probBtn) throw new Error('字段按钮应齐全');
    await constant.trigger('click');
    await enabled.trigger('click');
    await caseBtn.trigger('click');
    await regexBtn.trigger('click');
    await probBtn.trigger('click');
    expect(wrapper.emitted('toggleConstant')).toHaveLength(1);
    expect(wrapper.emitted('patch')).toContainEqual(['enabled', false]);
    expect(wrapper.emitted('patch')).toContainEqual(['case_sensitive', true]);
    expect(wrapper.emitted('patch')).toContainEqual(['use_regex', true]);
    expect(wrapper.emitted('patch')).toContainEqual(['use_probability', true]);
  });

  it('上移/下移在边界禁用,点击分别上抛 -1 / 1;删除上抛 remove', async () => {
    const first = mountEditor(makeEntry(), 0, 2);
    const up = first.findAll('button').find((b) => b.text() === '↑');
    const down = first.findAll('button').find((b) => b.text() === '↓');
    const remove = first.findAll('button').find((b) => b.text() === '✕');
    if (!up || !down || !remove) throw new Error('排序按钮应存在');
    expect((up.element as HTMLButtonElement).disabled).toBe(true);
    expect((down.element as HTMLButtonElement).disabled).toBe(false);
    await down.trigger('click');
    await remove.trigger('click');
    expect(first.emitted('move')).toEqual([[1]]);
    expect(first.emitted('remove')).toHaveLength(1);

    const last = mountEditor(makeEntry(), 1, 2);
    const lastDown = last.findAll('button').find((b) => b.text() === '↓');
    if (!lastDown) throw new Error('下移按钮应存在');
    expect((lastDown.element as HTMLButtonElement).disabled).toBe(true);
  });

  it('位置/角色下拉用 change 上抛数值与 null', async () => {
    const wrapper = mountEditor(makeEntry());
    const selects = wrapper.findAll('select');
    await selects[0].setValue('3');
    await selects[1].setValue('');
    const patches = wrapper.emitted('patch');
    expect(patches).toContainEqual(['position', 3]);
    expect(patches).toContainEqual(['role', null]);
  });
});
