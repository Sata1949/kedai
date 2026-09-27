// @vitest-environment jsdom
// 步骤编辑器的**数值区间**测试(自定义流程收口批 2026-09-24)。
//
// 背景:后端 `validate_flow` 只收 `max_tokens` ∈ 1..=32768
// (`server-rs/src/services/agent_flow_service/` 的「输出上限需在 1-32768 之间」),
// 而编辑器此前把 `max` 写死 131072——用户在 32769~131072 之间填任何值都会
// 「UI 看着能填、保存必 400」,且保存失败的提示只把后端中文错误整条展示
// (`useAgentFlow` 不定位字段),用户无从判断是哪个字段越界。
//
// 本文件锁定「输入框区间 == 后端校验区间」这条契约:区间改回大数值即 FAIL。
import { describe, expect, it } from 'vitest';
import { mount } from '@vue/test-utils';
import AgentFlowStepEditor from './AgentFlowStepEditor.vue';
import {
  STEP_MAX_CONTEXT_MAX,
  STEP_MAX_CONTEXT_MIN,
  STEP_OUTPUT_TOKENS_MAX,
  STEP_OUTPUT_TOKENS_MIN,
} from '../../utils/agentFlowTools';
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

/** 取「上下文上限」输入框(二维批次 8;同样按 title 定位) */
function contextLimitInput(wrapper: ReturnType<typeof mount>) {
  const inputs = wrapper.findAll('input[type="number"]');
  const hit = inputs.find((i) => (i.attributes('title') ?? '').includes('token 上限'));
  expect(hit, '未找到「上下文上限」输入框').toBeTruthy();
  return hit!;
}

describe('AgentFlowStepEditor 数值区间(输出上限)', () => {
  it('常量与后端校验区间一致(1..=32768)', () => {
    // 后端单一出处:server-rs/src/services/agent_flow_service/ 的 max_tokens 校验
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

describe('AgentFlowStepEditor 数值区间(上下文上限,二维批次 8)', () => {
  it('常量与后端校验区间一致(256..=1048576)', () => {
    // 后端单一出处:server-rs/src/services/agent_flow_service/ 的
    // MIN_STEP_MAX_CONTEXT / MAX_STEP_MAX_CONTEXT
    expect(STEP_MAX_CONTEXT_MIN).toBe(256);
    expect(STEP_MAX_CONTEXT_MAX).toBe(1048576);
  });

  it('输入框 min/max 与 title 按该区间渲染,占位文案表明「留空 = 不限制」', () => {
    const step = makeStep();
    const wrapper = mount(AgentFlowStepEditor, { props: { step, steps: [step] } });
    const input = contextLimitInput(wrapper);

    expect(input.attributes('min')).toBe('256');
    expect(input.attributes('max')).toBe('1048576');
    expect(input.attributes('placeholder')).toBe('不限制');
    expect(input.attributes('title')).toContain('留空 = 不限制');
    // 未设值时输入框为空 —— 「留空 = 不裁剪」是缺省口径
    expect((input.element as HTMLInputElement).value).toBe('');
    wrapper.unmount();
  });

  it('写入上限就落在草稿上(不改其他字段)', () => {
    const step = makeStep();
    const wrapper = mount(AgentFlowStepEditor, { props: { step, steps: [step] } });
    return contextLimitInput(wrapper)
      .setValue('1024')
      .then(() => {
        expect(step.max_context).toBe(1024);
        expect(step.max_tokens).toBeNull();
        wrapper.unmount();
      });
  });

  it('挂载子流程的节点上该输入不渲染(执行参数整块旁路,配置保留)', () => {
    const step = makeStep({ sub_flow_id: 'flow-x', max_context: 1024 });
    const wrapper = mount(AgentFlowStepEditor, { props: { step, steps: [step] } });
    expect(
      wrapper.findAll('input[type="number"]').some((i) => (i.attributes('title') ?? '').includes('token 上限')),
      '挂载子流程后不该显示上下文上限输入框',
    ).toBe(false);
    // 草稿里的配置仍在(清空挂载即恢复生效)
    expect(step.max_context).toBe(1024);
    wrapper.unmount();
  });
});
