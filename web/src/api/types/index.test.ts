// 类型域拆分后的结构护栏（C-2A，2026-10-25）。
//
// 目的有三：
// 1) 每个域文件都被测试直接 import——零测试 ratchet 要求新源码文件有直接引用；
// 2) 断言 `api/types.ts`（旧路径）与 `api/types/index.ts`（新 barrel）都保持
//    **零运行期导出**：类型域拆分只搬声明，不得顺手带进常量/函数（否则 barrel
//    会从「纯类型门面」变成运行期模块，进而影响 tree-shaking 与首屏体积）；
// 3) 每个域的代表类型仍可解析——纯 type import 在 vue-tsc 阶段兜住拼写/路径错误。
import { describe, expect, expectTypeOf, it } from 'vitest';
import type { CharacterRecord, ChatMessage, RegexScript, ScriptTreeNode } from './character';
import type { SseEvent, TokenUsage } from './chat';
import type { ToolPermission, ToolRisk } from './tools';
import type { TaskDetail, TaskEvent, TaskRecord, TaskStep } from './task';
import type { AgentFlowConfig, AgentFlowStep } from './agentFlow';
import type { ConnectionProfile, RuntimeSettings, RuntimeSettingsPatch } from './settings';
import type { WorldBookEntry, WorldBookRecord } from './worldbook';
import type { AudioChannelState, AudioTrack } from './audio';
import type { TaskDeliverySummary, WorkspaceProfile } from './workspace';
import type { KedaiImportBundle, MacroExpandCtx } from './misc';

describe('api/types 域拆分', () => {
  it('各域代表类型仍可解析', () => {
    expectTypeOf<CharacterRecord>().toMatchTypeOf<{ id: string }>();
    expectTypeOf<ChatMessage>().toMatchTypeOf<{ session_id: string }>();
    expectTypeOf<RegexScript>().toMatchTypeOf<{ find_regex: string }>();
    expectTypeOf<ScriptTreeNode>().toMatchTypeOf<{ type: 'script' | 'folder' }>();
    expectTypeOf<TokenUsage>().toMatchTypeOf<{ total_tokens: number }>();
    expectTypeOf<SseEvent>().toMatchTypeOf<{ type: string }>();
    expectTypeOf<ToolRisk>().toEqualTypeOf<'safe' | 'sensitive' | 'dangerous'>();
    expectTypeOf<ToolPermission>().toMatchTypeOf<{ risk: ToolRisk }>();
    expectTypeOf<TaskRecord>().toMatchTypeOf<{ status: string }>();
    expectTypeOf<TaskStep>().toMatchTypeOf<{ name: string }>();
    expectTypeOf<TaskEvent>().toMatchTypeOf<{ task_id: string }>();
    expectTypeOf<TaskDetail>().toMatchTypeOf<{ task: TaskRecord }>();
    expectTypeOf<AgentFlowStep>().toMatchTypeOf<{ id: string }>();
    expectTypeOf<AgentFlowConfig>().toMatchTypeOf<{ enabled: boolean }>();
    expectTypeOf<ConnectionProfile>().toMatchTypeOf<{ base_url: string }>();
    expectTypeOf<RuntimeSettings>().toMatchTypeOf<{ model: string }>();
    expectTypeOf<RuntimeSettingsPatch>().toMatchTypeOf<{ model?: string }>();
    expectTypeOf<WorldBookRecord>().toMatchTypeOf<{ entry_count: number }>();
    expectTypeOf<WorldBookEntry>().toMatchTypeOf<{ keys: string[] }>();
    expectTypeOf<AudioTrack>().toMatchTypeOf<{ url: string }>();
    expectTypeOf<AudioChannelState>().toMatchTypeOf<{ playlist: AudioTrack[] }>();
    expectTypeOf<WorkspaceProfile>().toMatchTypeOf<{ root: string }>();
    expectTypeOf<TaskDeliverySummary>().toMatchTypeOf<{ checked: boolean }>();
    expectTypeOf<MacroExpandCtx>().toMatchTypeOf<{ character_name?: string }>();
    expectTypeOf<KedaiImportBundle>().toMatchTypeOf<{ sections: Record<string, unknown> }>();
  });

  it('新 barrel 与旧路径都没有运行期导出（纯类型模块）', async () => {
    const viaIndex = await import('./index');
    const viaLegacy = await import('../types');
    expect(Object.keys(viaIndex)).toEqual([]);
    expect(Object.keys(viaLegacy)).toEqual([]);
  });
});
