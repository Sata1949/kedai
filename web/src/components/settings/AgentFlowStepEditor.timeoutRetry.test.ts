// @vitest-environment jsdom
// 步骤编辑器的**调用预算**测试(A 批 A1/A2:单次调用超时 + 空产出重试)。
//
// 背景:两个字段都是后端新增的节点级配置,保存期有区间校验(超时 30..=3600 秒、
// 重试 1..=5 次)——编辑器若把区间写错或漏渲染,用户会「UI 看着能填、保存必 400」,
// 而保存失败只把后端中文错误整条展示(`useAgentFlow` 不定位字段)。
//
// 本文件锁定四件事:
//   ① 前端常量 == 后端校验区间(改一端不改另一端即 FAIL);
//   ② 输入框 min/max/placeholder/title 逐项按该区间与语义渲染;
//   ③ 写入落在草稿上(且不动其它字段)——夹取由 set 函数做,越界由后端兜底;
//   ④ 挂载子流程的节点上整块不渲染(执行参数旁路),草稿里的配置保留。
import { describe, expect, it } from 'vitest';
import { mount } from '@vue/test-utils';
import AgentFlowStepEditor from './AgentFlowStepEditor.vue';
import {
  STEP_CALL_TIMEOUT_MAX,
  STEP_CALL_TIMEOUT_MIN,
  STEP_MAX_RETRIES_MAX,
  STEP_MAX_RETRIES_MIN,
} from '../../utils/agentFlowStepLimits';
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

/** 按 title 里的判据取一个 number 输入框(避免依赖行序) */
function numInput(wrapper: ReturnType<typeof mount>, marker: string) {
  const hit = wrapper
    .findAll('input[type="number"]')
    .find((i) => (i.attributes('title') ?? '').includes(marker));
  expect(hit, `未找到 title 含「${marker}」的输入框`).toBeTruthy();
  return hit!;
}

const timeoutInput = (w: ReturnType<typeof mount>) => numInput(w, '超过该秒数即失败');
const retryInput = (w: ReturnType<typeof mount>) => numInput(w, '只在产出为空时重试');

describe('AgentFlowStepEditor 单次调用超时(A 批 A1)', () => {
  it('常量与后端校验区间一致(30..=3600 秒)', () => {
    // 后端单一出处:server-rs/src/services/agent_flow_service/ 的
    // MIN_STEP_CALL_TIMEOUT_SECS / MAX_STEP_CALL_TIMEOUT_SECS
    expect(STEP_CALL_TIMEOUT_MIN).toBe(30);
    expect(STEP_CALL_TIMEOUT_MAX).toBe(3600);
  });

  it('输入框 min/max/placeholder/title 按区间与语义渲染', () => {
    const step = makeStep();
    const wrapper = mount(AgentFlowStepEditor, { props: { step, steps: [step] } });
    const input = timeoutInput(wrapper);

    expect(input.attributes('min')).toBe('30');
    expect(input.attributes('max')).toBe('3600');
    expect(input.attributes('placeholder')).toBe('沿用缺省 300 秒');
    const title = input.attributes('title') ?? '';
    expect(title).toContain('30-3600');
    expect(title).toContain('留空沿用缺省 300 秒');
    expect(title).toContain('挂载子流程的节点上不生效');
    // 未设值时输入框为空 —— 「留空 = 沿用宿主缺省」是缺省口径
    expect((input.element as HTMLInputElement).value).toBe('');
    wrapper.unmount();
  });

  it('写入超时就落在草稿上(不改其它字段)', () => {
    const step = makeStep();
    const wrapper = mount(AgentFlowStepEditor, { props: { step, steps: [step] } });
    return timeoutInput(wrapper)
      .setValue('900')
      .then(() => {
        expect(step.call_timeout_secs).toBe(900);
        expect(step.max_retries).toBeUndefined();
        expect(step.max_tokens).toBeNull();
        expect(step.max_context).toBeUndefined();
        wrapper.unmount();
      });
  });

  it('越界值由 set 函数夹取(60 以下夹到 30,守住后端区间)', () => {
    const step = makeStep();
    const wrapper = mount(AgentFlowStepEditor, { props: { step, steps: [step] } });
    return timeoutInput(wrapper)
      .setValue('5')
      .then(() => {
        expect(step.call_timeout_secs).toBe(STEP_CALL_TIMEOUT_MIN);
        wrapper.unmount();
      });
  });

  it('清空输入回到「沿用缺省」(字段写 null,整键省略)', () => {
    const step = makeStep({ call_timeout_secs: 120 });
    const wrapper = mount(AgentFlowStepEditor, { props: { step, steps: [step] } });
    return timeoutInput(wrapper)
      .setValue('')
      .then(() => {
        expect(step.call_timeout_secs).toBeNull();
        wrapper.unmount();
      });
  });
});

describe('AgentFlowStepEditor 空产出重试(A 批 A2)', () => {
  it('常量与后端校验区间一致(1..=5 次)', () => {
    // 后端单一出处:server-rs/src/services/agent_flow_service/ 的
    // MIN_STEP_MAX_RETRIES / MAX_STEP_MAX_RETRIES
    expect(STEP_MAX_RETRIES_MIN).toBe(1);
    expect(STEP_MAX_RETRIES_MAX).toBe(5);
  });

  it('输入框 min/max/placeholder/title 按区间与语义渲染', () => {
    const step = makeStep();
    const wrapper = mount(AgentFlowStepEditor, { props: { step, steps: [step] } });
    const input = retryInput(wrapper);

    expect(input.attributes('min')).toBe('1');
    expect(input.attributes('max')).toBe('5');
    expect(input.attributes('placeholder')).toBe('不重试');
    const title = input.attributes('title') ?? '';
    expect(title).toContain('只在产出为空时重试');
    expect(title).toContain('1-5');
    // 「硬错误不重试」与「翻倍输出预算」两条语义必须写在用户看得到的地方
    expect(title).toContain('不重试');
    expect(title).toContain('翻倍输出预算');
    expect((input.element as HTMLInputElement).value).toBe('');
    wrapper.unmount();
  });

  it('写入重试次数就落在草稿上(不改其它字段)', () => {
    const step = makeStep();
    const wrapper = mount(AgentFlowStepEditor, { props: { step, steps: [step] } });
    return retryInput(wrapper)
      .setValue('3')
      .then(() => {
        expect(step.max_retries).toBe(3);
        expect(step.call_timeout_secs).toBeUndefined();
        expect(step.max_tokens).toBeNull();
        wrapper.unmount();
      });
  });
});

describe('AgentFlowStepEditor 调用预算:挂载子流程的节点上旁路(A 批 A1/A2)', () => {
  it('两个输入都不渲染,草稿里的配置保留(清空挂载即恢复生效)', () => {
    const step = makeStep({ sub_flow_id: 'flow-x', call_timeout_secs: 60, max_retries: 2 });
    const wrapper = mount(AgentFlowStepEditor, { props: { step, steps: [step] } });
    const titles = wrapper
      .findAll('input[type="number"]')
      .map((i) => i.attributes('title') ?? '');
    expect(
      titles.some((t) => t.includes('超过该秒数即失败')),
      '挂载子流程后不该显示单次调用超时输入框',
    ).toBe(false);
    expect(
      titles.some((t) => t.includes('只在产出为空时重试')),
      '挂载子流程后不该显示空产出重试输入框',
    ).toBe(false);
    // 草稿里的配置仍在(清空挂载即恢复生效)
    expect(step.call_timeout_secs).toBe(60);
    expect(step.max_retries).toBe(2);
    wrapper.unmount();
  });

  it('反思步骤(不生成正文)同样能配置两项:它们与档位/动作无关,只关「这次调用」', () => {
    const step = makeStep({ action: 'reflect', generates: undefined });
    const wrapper = mount(AgentFlowStepEditor, { props: { step, steps: [step] } });
    expect((timeoutInput(wrapper).element as HTMLInputElement).value).toBe('');
    expect((retryInput(wrapper).element as HTMLInputElement).value).toBe('');
    wrapper.unmount();
  });
});
