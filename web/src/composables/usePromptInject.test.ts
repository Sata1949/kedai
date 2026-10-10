// 形状判定回归网(STIMP-2 原用例迁入说明见下;IMPHUB 步骤 C 起从 utils/importBundle 取实现)。
//
// 缺陷背景:修复前 ST 判据查 data.floors(kedai 自有楼层形状),与后端 parsing/preset.rs
// 「以 prompts 为唯一依据」相反 —— 真酒馆预设必被前端拒绝(「无法识别的 JSON 格式」),
// kedai 自家导出因 floors 条目带 role/position 反被误判为 ST 送后端 400。见 docs/计划.md STIMP 章。
//
// 本文件保留两件事:① composable 的 re-export 仍然有效(调用方零改动);
// ② 仓库自带示例预设的判定锚点。综合导入的完整形状矩阵(含 bundle v1 / 流程 / 技能 /
// 插件 / 文本歧义 / 追加合并)在 utils/importBundle.test.ts;
// 原「onImportPreset 三路分派」用例的职责随之迁到综合导入窗口(组件层,见 ImportBundleModal.test.ts)。
import { describe, expect, it } from 'vitest';
import { detectInjectFileShape } from './usePromptInject';
// 仓库自带示例预设(静态 JSON 导入;本文件不再需要 jsdom,node 环境即可)
import examplePreset from '../../../examples/presets/可待精华增强楼层.json';

describe('detectInjectFileShape 形状判定(re-export 自 utils/importBundle)', () => {
  it('顶层 prompts 数组 → st(酒馆预设)', () => {
    expect(detectInjectFileShape({ name: 'x', prompts: [{ identifier: 'a' }], prompt_order: [] })).toBe('st');
  });

  it('prompts 为空数组仍判 st(是否可导入交后端给精确错误)', () => {
    expect(detectInjectFileShape({ prompts: [] })).toBe('st');
  });

  it('mode 字符串 + simple 字段 → kedai(本工具导出配置)', () => {
    expect(detectInjectFileShape({ mode: 'complex', simple: { banned_words_enabled: false }, floors: [] })).toBe('kedai');
  });

  it('floors-only 形状(缺 mode/simple)→ unknown', () => {
    expect(detectInjectFileShape({ floors: [{ role: 'user', position: 'system' }] })).toBe('unknown');
  });

  it('空对象 → unknown', () => {
    expect(detectInjectFileShape({})).toBe('unknown');
  });

  it('null 与字符串等非对象 → unknown', () => {
    expect(detectInjectFileShape(null)).toBe('unknown');
    expect(detectInjectFileShape('not-an-object')).toBe('unknown');
  });
});

describe('仓库自带示例回归', () => {
  it('examples/presets/可待精华增强楼层.json 判定为 st(自 v0.2.0 起不可导入的缺陷锚点)', () => {
    expect(Array.isArray(examplePreset.prompts)).toBe(true);
    expect(detectInjectFileShape(examplePreset)).toBe('st');
  });
});
