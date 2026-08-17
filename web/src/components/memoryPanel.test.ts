import { describe, expect, it } from 'vitest';
import {
  distillErrorText,
  distillSuccessText,
  formatMemoryTime,
  kindClass,
  kindLabel,
  memoryRows,
  nextPendingDelete,
} from './memoryPanel';
import type { MemoryEntry } from '../api';

// 记忆库面板纯函数测试:kind 标签映射 / 时间格式化 / 列表行映射排序 /
// 蒸馏结果反馈文案 / 删除两段式确认状态机。

function entry(overrides: Partial<MemoryEntry> = {}): MemoryEntry {
  return {
    id: 1,
    character_id: 'charA',
    source_session_id: null,
    kind: 'manual',
    content: '记忆',
    usage_count: 0,
    last_usage: null,
    selected: true,
    created_at: '2026-08-14T08:00:00Z',
    updated_at: '2026-08-14T08:00:00Z',
    ...overrides,
  };
}

describe('kindLabel / kindClass(kind 标签映射)', () => {
  it('三种已知 kind 映射为中文标签与配色 class', () => {
    expect(kindLabel('distilled')).toBe('蒸馏');
    expect(kindLabel('tool')).toBe('工具');
    expect(kindLabel('manual')).toBe('手动');
    expect(kindClass('distilled')).toBe('kind-distilled');
    expect(kindClass('tool')).toBe('kind-tool');
    expect(kindClass('manual')).toBe('kind-manual');
  });

  it('未知 kind 不吞异常数据:标签原样、配色中性灰', () => {
    expect(kindLabel('bogus')).toBe('bogus');
    expect(kindClass('bogus')).toBe('kind-unknown');
  });
});

describe('formatMemoryTime(last_usage 格式化)', () => {
  it('ISO 时间统一截断到分(T → 空格)', () => {
    expect(formatMemoryTime('2026-08-15T10:30:00Z')).toBe('2026-08-15 10:30');
  });

  it('null(从未注入)显示「未使用」而非空串', () => {
    expect(formatMemoryTime(null)).toBe('未使用');
  });
});

describe('memoryRows(列表行映射与排序)', () => {
  it('映射展示行并按 id 降序(最新在前,与后端 ORDER BY id DESC 一致,幂等)', () => {
    const rows = memoryRows([
      entry({ id: 2, kind: 'distilled', content: '旧', usage_count: 0, last_usage: null }),
      entry({ id: 5, kind: 'tool', content: '新', usage_count: 4, last_usage: '2026-08-15T09:00:00Z', selected: false }),
    ]);
    expect(rows.map((r) => r.id)).toEqual([5, 2]);
    expect(rows[0]).toMatchObject({
      content: '新',
      kindLabel: '工具',
      kindClass: 'kind-tool',
      usage: 4,
      lastUsage: '2026-08-15 09:00',
      selected: false,
    });
    expect(rows[1].lastUsage).toBe('未使用');
    // 幂等:同输入两次调用顺序一致
    expect(memoryRows([
      entry({ id: 2 }),
      entry({ id: 5 }),
    ]).map((r) => r.id)).toEqual([5, 2]);
  });

  it('空列表返回空数组(空态判定)', () => {
    expect(memoryRows([])).toEqual([]);
  });
});

describe('蒸馏反馈文案', () => {
  it('成功:inserted>0 → 新增 N 条;0(空历史)→ 无可提取提示', () => {
    expect(distillSuccessText(3)).toBe('蒸馏完成:新增 3 条记忆');
    expect(distillSuccessText(0)).toBe('蒸馏完成:当前会话没有可提取的记忆');
  });

  it('失败:未开启(400)→ 追加去设置开启的指引;其余错误原样带前缀', () => {
    expect(distillErrorText('跨会话记忆蒸馏未开启,请先在设置中打开 memory_distill_enabled')).toBe(
      '蒸馏失败:跨会话记忆蒸馏未开启,请先在设置中打开 memory_distill_enabled。可到 设置 → 生成参数 打开「记忆蒸馏」后重试',
    );
    expect(distillErrorText('会话不存在: s9')).toBe('蒸馏失败:会话不存在: s9');
  });
});

describe('nextPendingDelete(删除两段式确认状态机)', () => {
  it('request 进入确认态;confirm / cancel 退出', () => {
    expect(nextPendingDelete(null, 'request', 7)).toBe(7);
    expect(nextPendingDelete(7, 'request', 7)).toBe(7);
    expect(nextPendingDelete(7, 'cancel', 7)).toBeNull();
    expect(nextPendingDelete(7, 'confirm', 7)).toBeNull();
  });

  it('切换目标行时确认态跟随新行(只允许一行处于确认)', () => {
    expect(nextPendingDelete(7, 'request', 9)).toBe(9);
  });
});
