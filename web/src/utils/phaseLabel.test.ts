// 阶段标签与流式缓冲键解析的纯函数测试。
//
// 这份契约原先只活在 CallTracePanel 组件内部,任务看台的多缓冲兜底便把**内部键**直接
// 插值给了用户(`【subflow.1:0】`,遗留.md IFW-7③)。抽成模块后契约在这里锁死:
// 阶段词的中文映射、子图路径展开、脏数据回退、缓冲 key 的切分口径、步骤名补全。
import { describe, expect, it } from 'vitest';
import { bufferLabel, phaseText } from './phaseLabel';

describe('phaseText 阶段中文映射', () => {
  it('固定词映射为中文;未命中分支原样回退(脏数据不静默变空白)', () => {
    expect(phaseText('planner')).toBe('规划');
    expect(phaseText('summarize')).toBe('汇总');
    expect(phaseText('summary')).toBe('汇总');
    expect(phaseText('subagent')).toBe('子Agent');
    expect(phaseText('audit')).toBe('审计');
    expect(phaseText('final_audit')).toBe('终审');
    expect(phaseText('unknown_phase')).toBe('unknown_phase');
  });

  it('agent 阶段(team 主 Agent 子目标)带步骤下标:多缓冲前缀才可区分', () => {
    // team 落库时 step_index = 该子目标的全局步骤下标(team.rs),恒有值
    expect(phaseText('agent', 0)).toBe('主Agent #1');
    expect(phaseText('agent', 3)).toBe('主Agent #4');
    // 缺下标时只说词(旧数据/异常路径)
    expect(phaseText('agent')).toBe('主Agent');
    expect(phaseText('agent', null)).toBe('主Agent');
    // 该阶段也吃步骤名解析器(下标落在 plan 上)
    expect(phaseText('agent', 1, (i) => (i === 1 ? '写开场' : null))).toBe('主Agent #2 · 写开场');
  });

  it('step 阶段:step_index 0 起、展示 +1;缺序号时只说「步骤」', () => {
    expect(phaseText('step', 0)).toBe('步骤 #1');
    expect(phaseText('step', 2)).toBe('步骤 #3');
    expect(phaseText('step')).toBe('步骤');
    expect(phaseText('step', null)).toBe('步骤');
  });

  it('step 阶段可补步骤名(名称为空/取不到时不加后缀,不留悬空分隔符)', () => {
    const names = ['起草', '  ', '修订'];
    const stepName = (i: number) => names[i] ?? null;
    expect(phaseText('step', 0, stepName)).toBe('步骤 #1 · 起草');
    expect(phaseText('step', 1, stepName)).toBe('步骤 #2');
    // 越界(plan 比下标短)时不补名
    expect(phaseText('step', 9, stepName)).toBe('步骤 #10');
  });

  it('子图:路径段展开为「#父」序,叶子是子图内的步号', () => {
    expect(phaseText('subflow', 0)).toBe('子流程 #1');
    expect(phaseText('subflow.1', 2)).toBe('子流程(#2 内) #3');
    expect(phaseText('subflow.1.0', 0)).toBe('子流程(#2›#1 内) #1');
    expect(phaseText('subflow.2')).toBe('子流程(#3 内)');
  });

  it('子图脏数据兜底:非数字路径段原样回退,不渲染 NaN;空路径段被跳过', () => {
    expect(phaseText('subflow.x', 2)).toBe('子流程(x 内) #3');
    expect(phaseText('subflow.', 1)).toBe('子流程 #2');
    expect(phaseText('subflow.1.x', 0)).toBe('子流程(#2›x 内) #1');
    expect(phaseText('subflow.x')).not.toContain('NaN');
  });
});

describe('bufferLabel 缓冲 key → 标签', () => {
  it('按首个冒号切分:phase 与 step_index 各自取到(phase 本身含点不含冒号)', () => {
    expect(bufferLabel('planner:')).toBe('规划');
    expect(bufferLabel('step:1')).toBe('步骤 #2');
    expect(bufferLabel('subflow.1:0')).toBe('子流程(#2 内) #1');
  });

  it('步骤名只在 step/agent 阶段生效,并按 key 里的下标取值', () => {
    const stepName = (i: number) => (i === 1 ? '起草' : null);
    expect(bufferLabel('step:1', stepName)).toBe('步骤 #2 · 起草');
    expect(bufferLabel('step:0', stepName)).toBe('步骤 #1');
    expect(bufferLabel('agent:1', stepName)).toBe('主Agent #2 · 起草');
    // 子图内部节点不进 plan:即便传了解析器也不补名(避免指向错误的步骤)
    expect(bufferLabel('subflow.1:0', stepName)).toBe('子流程(#2 内) #1');
  });

  it('非法 key 按「只有 phase」处理:无冒号 / 步骤号非数字都不渲染 NaN', () => {
    expect(bufferLabel('planner')).toBe('规划');
    expect(bufferLabel('step:abc')).toBe('步骤');
    expect(bufferLabel('')).toBe('');
  });
});
