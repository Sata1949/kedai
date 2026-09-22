// @vitest-environment jsdom
// 步骤编辑器的 **props 响应式**测试(遗留.md IFW-7① 的回归)。
//
// 背景:草稿会被整体替换(`useAgentFlow.loadFlowConfig` 用深拷贝重建草稿,保存 / 切换流程 /
// 新建 / 复制 / 导入后都走它),调用方随后仍把 `editingStepId` 指回同 id 的步骤。若编辑器
// 把 props 解构成**值**(`const { step } = props`),它拿到的就是替换前的那个对象:
// 列表行读新对象、编辑区写旧对象,界面看着能编辑,继续编辑的改动下次保存时静默丢失。
//
// 本文件只测「props 换了对象之后,表单读的、写的都是新对象」——这是修复的判别性断言
// (换成值解构即 FAIL)。表单字段的语义覆盖留在 AgentFlowSection.test.ts。
import { describe, expect, it } from 'vitest';
import { mount } from '@vue/test-utils';
import AgentFlowStepEditor from './AgentFlowStepEditor.vue';
import type { AgentFlowStep } from '../../api/types';

function makeStep(over: Partial<AgentFlowStep> = {}): AgentFlowStep {
  return {
    id: 'a',
    name: '起草',
    enabled: true,
    goal: '旧目标',
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

/** 取「目标」输入框(表单第一行) */
function goalInput(wrapper: ReturnType<typeof mount>) {
  return wrapper.find('input.sv-input');
}

describe('AgentFlowStepEditor props 响应式(IFW-7①)', () => {
  it('step prop 换成新对象后:输入框显示新值,编辑写入新对象(旧对象不被改)', async () => {
    const oldStep = makeStep({ goal: '旧目标' });
    const wrapper = mount(AgentFlowStepEditor, {
      props: { step: oldStep, steps: [oldStep] },
    });
    expect((goalInput(wrapper).element as HTMLInputElement).value).toBe('旧目标');

    // 模拟保存后的草稿重建:同 id 的**新对象**替换旧对象(loadFlowConfig 的深拷贝语义)
    const newStep = makeStep({ goal: '新目标' });
    expect(newStep).not.toBe(oldStep);
    await wrapper.setProps({ step: newStep, steps: [newStep] });

    // ① 读:输入框必须跟着换成新对象的值(值解构时此处仍是「旧目标」→ FAIL)
    expect((goalInput(wrapper).element as HTMLInputElement).value).toBe('新目标');

    // ② 写:编辑落在新对象上
    await goalInput(wrapper).setValue('改过的目标');
    expect(newStep.goal).toBe('改过的目标');
    expect(oldStep.goal).toBe('旧目标');
    wrapper.unmount();
  });

  it('steps prop 换成新数组后:上游候选跟着更新(不读快照)', async () => {
    const a = makeStep({ id: 'a', name: '起草' });
    const wrapper = mount(AgentFlowStepEditor, {
      props: { step: a, steps: [a] },
    });
    // 只有一个步骤时没有可用上游
    expect(wrapper.text()).toContain('暂无可用上游');

    // 换成含两个步骤的新数组(同 id 的新对象)
    const a2 = makeStep({ id: 'a', name: '起草' });
    const b2 = makeStep({ id: 'b', name: '修订' });
    await wrapper.setProps({ step: a2, steps: [b2, a2] });

    expect(wrapper.text()).not.toContain('暂无可用上游');
    expect(wrapper.text()).toContain('修订');
    wrapper.unmount();
  });
});
