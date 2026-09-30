// 工作区画像展示辅助的单元测试(CODE-4)。
import { describe, expect, it } from 'vitest';
import { projectHintTitle, projectKindLabel } from './workspaceProfile';

describe('projectKindLabel', () => {
  it('已登记类型给展示名;未登记类型原样回显(不假装认识)', () => {
    expect(projectKindLabel('rust')).toBe('Rust');
    expect(projectKindLabel('dotnet')).toBe('.NET');
    expect(projectKindLabel('elixir')).toBe('elixir');
  });
});

describe('projectHintTitle', () => {
  it('tooltip 同时给出证据文件与建议命令', () => {
    expect(projectHintTitle('rust', 'server-rs/Cargo.toml', 'cargo test')).toBe(
      'Rust:server-rs/Cargo.toml → 建议验证:cargo test',
    );
  });
});
