// 提示词预览告警码 → 中文提示(单一出处;后端只回结构化码,文案不在此之外的组件里分叉)
import { describe, expect, it } from 'vitest';
import { promptWarningText } from './promptWarnings';

describe('promptWarningText(预览告警文案单一出处)', () => {
  it('已知码给出可操作的中文提示', () => {
    const text = promptWarningText('template_missing_world_info');
    expect(text).toContain('{{world_info}}');
    expect(text).toContain('常驻');
    // 契约口径:完整替换不追加,行为本身不是错误 —— 不得写成「保存失败」类措辞
    expect(text).not.toContain('失败');
  });

  it('未登记码原样透出(后端新增码不静默丢信息)', () => {
    expect(promptWarningText('some_future_code')).toBe('some_future_code');
  });

  it('原型链键名不得被当成已登记码', () => {
    expect(promptWarningText('constructor')).toBe('constructor');
    expect(promptWarningText('toString')).toBe('toString');
  });
});
