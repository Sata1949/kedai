// @vitest-environment jsdom
// 步骤编辑器的**数值区间**测试(自定义流程收口批 2026-09-24)。
//
// 背景:后端 `validate_flow` 只收 `max_tokens` ∈ 1..=32768
// (`server-rs/src/services/agent_flow_service.rs` 的「输出上限需在 1-32768 之间」),
// 而编辑器此前把 `max` 写死 131072——用户在 32769~131072 之间填任何值都会
// 「UI 看着能填、保存必 400」,且保存失败的提示只把后端中文错误整条展示
// (`useAgentFlow` 不定位字段),用户无从判断是哪个字段越界。
//
// 本文件锁定「输入框区间 == 后端校验区间」这条契约:区间改回大数值即 FAIL。
import { describe, expect, it } from 'vitest';
import { mount } from '@vue/test-utils';
import AgentFlowStepEditor from './AgentFlowStepEditor.vue';
import { STEP_OUTPUT_TOKENS_MAX, STEP_OUTPUT_TOKENS_MIN } from '../../utils/agentFlowTools';
import type { AgentFlowStep } from '../../api/types';

function makeStep(over: Partial<AgentFlowStep> = {}): AgentFlowStep {
  return {
    id: 'a',
    name: '起草',
    enabled: true,
    goal: '写一段开头',
    action: 'direct',
    generates: true,
    system_prompt: null,
    temperature: null,
    max_tokens: null,
    tools: null,
    tool_choice: 'auto',
    tool_choice_function: null,
    parallel_tool_calls: null,
    inputs: [],
    is_output: null,
    sub_flow_id: null,
    ...over,
  };
}

/** 取「输出上限」输入框(按 title 定位,避免依赖行序) */
function outputLimitInput(wrapper: ReturnType<typeof mount>) {
  const inputs = wrapper.findAll('input[type="number"]');
  const hit = inputs.find((i) => (i.attributes('title') ?? '').includes('输出上限'));
  expect(hit, '未找到「输出上限」输入框').toBeTruthy();
  return hit!;
}

describe('AgentFlowStepEditor 数值区间(输出上限)', () => {
  it('常量与后端校验区间一致(1..=32768)', () => {
    // 后端单一出处:server-rs/src/services/agent_flow_service.rs 的 max_tokens 校验
    expect(STEP_OUTPUT_TOKENS_MIN).toBe(1);
    expect(STEP_OUTPUT_TOKENS_MAX).toBe(32768);
  });

  it('输入框 min/max 与 title 都按该区间渲染(不再允许 131072)', () => {
    const step = makeStep();
    const wrapper = mount(AgentFlowStepEditor, { props: { step, steps: [step] } });
    const input = outputLimitInput(wrapper);

    expect(input.attributes('min')).toBe('1');
    expect(input.attributes('max')).toBe('32768');
    expect(input.attributes('title')).toContain('1~32768');
    expect(input.attributes('title')).toContain('留空沿用全局');
    wrapper.unmount();
  });

  it('区间外的值仍可读回(既有流程里的历史值不被 UI 改写)', () => {
    // 只收窄**输入约束**,不静默改写草稿值:越界值由后端拒绝并给出中文错误
    const step = makeStep({ max_tokens: 65536 });
    const wrapper = mount(AgentFlowStepEditor, { props: { step, steps: [step] } });
    expect((outputLimitInput(wrapper).element as HTMLInputElement).value).toBe('65536');
    expect(step.max_tokens).toBe(65536);
    wrapper.unmount();
  });
});
