// 二维依赖图工具函数测试(二维批次 1/2 前端)。
// 与后端 agent_flow_service 的图原语同口径:线性兼容、上游合法性、层级、成果选拔。
// 只断言数据层(与「画布不做像素断言」同一原则),不涉及 DOM。
import { describe, expect, it } from 'vitest';
import type { AgentFlowConfig, AgentFlowStep } from '../api/types';
import {
  MAX_SUB_FLOW_DEPTH,
  computeLevels,
  effectiveInputIds,
  effectiveLevels,
  graphHint,
  isLinearCompat,
  levelOf,
  outputStepName,
  removeStepReferences,
  setStepKind,
  setSubFlowId,
  stepInputs,
  stepKind,
  stepKindWarnings,
  stepSubFlowWarnings,
  subFlowCandidates,
  subFlowId,
  subFlowMountNote,
  subFlowSummary,
  toggleStepInput,
  upstreamCandidates,
  upstreamSummary,
  upstreamWarnings,
  flowAncestorDepth,
  flowNestingDepth,
  wouldCreateCycle,
  wouldCreateFlowCycle,
} from './agentFlowGraph';

/** 步骤工厂:id/上游/是否生成可定制,其余取一维默认值 */
function step(
  id: string,
  inputs: string[] = [],
  overrides: Partial<AgentFlowStep> = {},
): AgentFlowStep {
  return {
    id,
    name: id,
    enabled: true,
    goal: `${id} 目标`,
    action: 'direct',
    generates: true,
    inputs,
    ...overrides,
  };
}

/** 菱形:a、b 源节点 → c 合并 → d 收口 */
function diamond(): AgentFlowStep[] {
  return [step('a'), step('b'), step('c', ['a', 'b']), step('d', ['c'])];
}

describe('agentFlowGraph 依赖解析', () => {
  it('全部步骤不设上游 = 线性兼容(一维流程)', () => {
    expect(isLinearCompat([step('a'), step('b'), step('c')])).toBe(true);
    expect(isLinearCompat(diamond())).toBe(false);
  });

  it('stepInputs 归一化缺省与脏数据', () => {
    expect(stepInputs({ inputs: undefined })).toEqual([]);
    expect(stepInputs({ inputs: ['a', ''] })).toEqual(['a']);
  });

  it('层级按依赖深度计算,源节点为第 1 层', () => {
    const steps = diamond();
    expect(computeLevels(steps)).toEqual([0, 0, 1, 2]);
    expect(levelOf(steps, 'a')).toBe(1);
    expect(levelOf(steps, 'c')).toBe(2);
    expect(levelOf(steps, 'd')).toBe(3);
  });

  it('成环时层级为 null(编辑器提示,后端亦拒绝保存)', () => {
    const steps = [step('a', ['b']), step('b', ['a'])];
    expect(computeLevels(steps)).toBeNull();
    expect(levelOf(steps, 'a')).toBeNull();
    expect(graphHint(steps)).toContain('环');
  });
});

describe('agentFlowGraph 有效图(执行器视角,画布连线与布局共用)', () => {
  it('线性兼容流程的隐式上游 = 前一步(第 i 步挂第 i-1 步)', () => {
    const steps = [step('a'), step('b'), step('c')];
    expect(effectiveInputIds(steps)).toEqual([[], ['a'], ['b']]);
    // 显式 computeLevels 会把它们全放在第 1 层(线性提示用),有效层级则是链
    expect(computeLevels(steps)).toEqual([0, 0, 0]);
    expect(effectiveLevels(steps)).toEqual([0, 1, 2]);
  });

  it('二维流程的有效上游就是显式上游,不再追加隐式串联边', () => {
    const steps = diamond();
    expect(effectiveInputIds(steps)).toEqual([[], [], ['a', 'b'], ['c']]);
    expect(effectiveLevels(steps)).toEqual([0, 0, 1, 2]);
  });

  it('只要任一步设了上游,整个流程就不再按线性串联补边', () => {
    // 用户把 c 的上游显式设为 a:此时 a、b 都是源节点,b 不再隐式依赖 a
    const steps = [step('a'), step('b'), step('c', ['a'])];
    expect(effectiveInputIds(steps)).toEqual([[], [], ['a']]);
    expect(effectiveLevels(steps)).toEqual([0, 0, 1]);
  });

  it('有效层级在成环时为 null', () => {
    expect(effectiveLevels([step('a', ['b']), step('b', ['a'])])).toBeNull();
  });
});

describe('agentFlowGraph 上游候选与成环拦截', () => {
  it('候选排除自身与下游节点(选中即会成环)', () => {
    const steps = diamond();
    // 给 a 选上游:自身与 a 的全部下游(c、d)都会成环 → 只剩 b 可选
    const ids = upstreamCandidates(steps, 'a').map((s) => s.id);
    expect(ids).toEqual(['b']);
  });

  it('wouldCreateCycle 自环与间接环都判真', () => {
    const steps = diamond();
    expect(wouldCreateCycle(steps, 'a', 'a')).toBe(true);
    expect(wouldCreateCycle(steps, 'a', 'c')).toBe(true); // c 依赖 a → 再加 a→c 成环
    expect(wouldCreateCycle(steps, 'c', 'b')).toBe(false); // b 与 c 无路径
  });

  it('勾选上游就地写入 inputs,可反复切换', () => {
    const steps = [step('a'), step('b'), step('c', ['a'])];
    const c = steps[2];
    toggleStepInput(steps, c, 'a');
    expect(stepInputs(c)).toEqual([]);
    toggleStepInput(steps, c, 'b');
    expect(stepInputs(c)).toEqual(['b']);
  });

  it('上游顺序归一化为流程数组下标序(与后端多父合并顺序同口径,IFW-6)', () => {
    // 数组顺序 a、b、c;先勾 c 再勾 a —— 结果必须是 ['a','c'],不能是勾选顺序 ['c','a']
    const steps = [step('a'), step('b'), step('c', ['b'])];
    toggleStepInput(steps, steps[2], 'a');
    expect(stepInputs(steps[2])).toEqual(['a', 'b']);
    // 摘要显示顺序据此与执行时的拼接顺序一致(此前是勾选顺序,属误导)
    expect(upstreamSummary(steps, steps[2])).toContain('上游:a、b');
  });

  it('删除步骤后清理其它步骤对它的悬空引用', () => {
    const steps = [step('a'), step('b'), step('c', ['a', 'b']), step('d', ['a'])];
    removeStepReferences(
      steps.filter((s) => s.id !== 'a'),
      'a',
    );
    const c = steps.find((s) => s.id === 'c') as AgentFlowStep;
    const d = steps.find((s) => s.id === 'd') as AgentFlowStep;
    expect(stepInputs(c)).toEqual(['b']);
    expect(stepInputs(d)).toEqual([]);
  });
});

describe('agentFlowGraph 提示与成果节点', () => {
  it('上游摘要区分线性串联 / 源节点 / 多上游', () => {
    const linear = [step('a'), step('b')];
    expect(upstreamSummary(linear, linear[1])).toContain('线性串联');

    const steps = diamond();
    expect(upstreamSummary(steps, steps[0])).toContain('源节点');
    expect(upstreamSummary(steps, steps[2])).toContain('上游:a、b');
  });

  it('上游缺失或停用时给出警示(后端会拒绝保存)', () => {
    const missing = [step('a', ['ghost'])];
    expect(upstreamWarnings(missing, missing[0])[0]).toContain('上游节点已不存在');

    const disabled = [step('a', [], { enabled: false }), step('b', ['a'])];
    expect(upstreamWarnings(disabled, disabled[1])[0]).toContain('已停用');
    expect(upstreamWarnings(disabled, disabled[1])[0]).toContain('保存会被拒绝');
  });

  it('成果节点:显式标注优先,未标注取无后继汇点,汇点不生成时回退末个生成步', () => {
    // 未标注:汇点 = d(生成步)
    expect(outputStepName(diamond())).toBe('d');
    // 显式标注优先于汇点
    const marked = diamond();
    marked[1].is_output = true;
    expect(outputStepName(marked)).toBe('b');
    // 汇点是反思步(不生成)→ 回退末个生成步(与后端 [生成, 反思] 的旧语义一致)
    const reflectTail = [step('gen'), step('check', ['gen'], { action: 'reflect', generates: undefined })];
    expect(outputStepName(reflectTail)).toBe('gen');
  });

  it('一维流程不给二维提示(不增加噪音)', () => {
    expect(graphHint([step('a'), step('b')])).toBeNull();
  });

  it('二维提示带上并行上限与成果节点', () => {
    const text = graphHint(diamond(), 3);
    expect(text).toContain('最多 3 个');
    expect(text).toContain('成果取「d」');
    // 未配置上限时按默认 2 提示
    expect(graphHint(diamond())).toContain('最多 2 个');
  });
});

describe('agentFlowGraph 节点档位(二维批次 6a)', () => {
  it('档位读写:缺省/脏数据按宽松,严格写回 strict,切回宽松省略字段', () => {
    const s = step('a');
    expect(stepKind(s)).toBe('loose');

    setStepKind(s, 'strict');
    expect(s.kind).toBe('strict');
    expect(stepKind(s)).toBe('strict');

    // 切回宽松写 null(序列化时省略该字段,与后端「缺省即宽松」一致)
    setStepKind(s, 'loose');
    expect(s.kind).toBeNull();
    expect(stepKind(s)).toBe('loose');

    // 缺省 / 显式 null 都按宽松(只有 'strict' 是严格;未知取值由后端保存期 400 拒绝,
    // 此处只做读侧兜底,不给非法取值留测试入口——类型逃逸会撞 lint ratchet)
    expect(stepKind({ kind: undefined })).toBe('loose');
    expect(stepKind({ kind: null })).toBe('loose');
  });

  it('严格档对已配置工具给出「不会下发」警示,且不暗示配置已丢失', () => {
    // 宽松档:无警示
    expect(stepKindWarnings(step('a'))).toEqual([]);

    // 严格 + 全部工具
    const all = step('a', [], { kind: 'strict', tools: [] });
    expect(stepKindWarnings(all)[0]).toContain('全部工具');
    expect(stepKindWarnings(all)[0]).toContain('切回宽松档即生效');

    // 严格 + 白名单:报出工具个数
    const list = step('a', [], { kind: 'strict', tools: ['read', 'search'] });
    expect(stepKindWarnings(list)[0]).toContain('2 个工具');

    // 严格 + 不使用工具:无矛盾,不提示(这是严格档的常态配置)
    expect(stepKindWarnings(step('a', [], { kind: 'strict', tools: null }))).toEqual([]);

    // 严格 + 非 auto 工具策略:策略不会生效
    const choice = step('a', [], { kind: 'strict', tools: null, tool_choice: 'required' });
    expect(stepKindWarnings(choice)[0]).toContain('工具策略');
  });
});

describe('agentFlowGraph 静态子图(二维批次 6b)', () => {
  // 流程库工厂:主流程把挂载点写成 sub_flow_id
  function flow(id: string, name: string, steps: Partial<AgentFlowStep>[] = []): AgentFlowConfig {
    return { id, name, enabled: true, steps: steps.map((s, i) => step(`s${i}`, [], s)) };
  }

  it('子流程读写:空串/脏数据按未挂载,清空写回 null', () => {
    const s = step('a');
    expect(subFlowId(s)).toBeNull();

    setSubFlowId(s, 'flow-x');
    expect(s.sub_flow_id).toBe('flow-x');
    expect(subFlowId(s)).toBe('flow-x');

    // 前后空白 trim 后使用(后端 sub_flow_ref 同口径)
    setSubFlowId(s, '  flow-y  ');
    expect(s.sub_flow_id).toBe('flow-y');

    // 清空 / 纯空白 → null(序列化时省略,不留脏引用)
    setSubFlowId(s, '   ');
    expect(s.sub_flow_id).toBeNull();
    setSubFlowId(s, null);
    expect(subFlowId({ sub_flow_id: undefined })).toBeNull();
  });

  it('成环判定:自引用与「引用链上已有本流程」都判为成环', () => {
    const lib = [flow('A', '主'), flow('B', '中间', []), flow('C', '末端')];
    // B → C 的链
    lib[1].steps = [{ ...step('b1', [], { sub_flow_id: 'C' }) }];
    // 把 C 挂到 A 上不成环
    expect(wouldCreateFlowCycle(lib, 'A', 'C')).toBe(false);
    // 把 A 自己挂到 A 上:自引用
    expect(wouldCreateFlowCycle(lib, 'A', 'A')).toBe(true);
    // C 引用链上没有 A,但让 C 挂 A(即 C→A 而 A 又要挂 C)会成环
    lib[2].steps = [{ ...step('c1', [], { sub_flow_id: 'A' }) }];
    expect(wouldCreateFlowCycle(lib, 'A', 'C')).toBe(true);
  });

  it('子流程候选:排除自身与会成环的流程,保留合法者', () => {
    const lib = [flow('A', '主'), flow('B', '直接引用我', []), flow('C', '干净')];
    lib[1].steps = [{ ...step('b1', [], { sub_flow_id: 'A' }) }];
    const ids = subFlowCandidates(lib, 'A').map((f) => f.id);
    expect(ids).toEqual(['C']);
  });

  it('子流程候选保留「深度过深」者(只滤必然错,不静默丢弃用户的选择)', () => {
    // B→C→D→E 已 3 层,挂到 A 上会超限——候选里仍要有,由警示告知
    const lib = [flow('A', '主'), flow('B', '深链', []), flow('C', '二'), flow('D', '三'), flow('E', '四')];
    lib[1].steps = [step('b1', [], { sub_flow_id: 'C' })];
    lib[2].steps = [step('c1', [], { sub_flow_id: 'D' })];
    lib[3].steps = [step('d1', [], { sub_flow_id: 'E' })];
    expect(subFlowCandidates(lib, 'A').map((f) => f.id)).toContain('B');
    expect(stepSubFlowWarnings(lib, 'A', step('a1', [], { sub_flow_id: 'B' }))[0]).toContain(
      '嵌套超过',
    );
  });

  it('嵌套层数:逐层累加,成环/悬空返回 null', () => {
    const lib = [flow('A', '主'), flow('B', '二'), flow('C', '三'), flow('D', '四')];
    expect(flowNestingDepth(lib, 'A')).toBe(0);
    lib[0].steps = [step('a1', [], { sub_flow_id: 'B' })];
    lib[1].steps = [step('b1', [], { sub_flow_id: 'C' })];
    lib[2].steps = [step('c1', [], { sub_flow_id: 'D' })];
    expect(flowNestingDepth(lib, 'A')).toBe(3);
    expect(flowNestingDepth(lib, 'D')).toBe(0);

    // 悬空引用 → 层数无法确定
    lib[3].steps = [step('d1', [], { sub_flow_id: 'missing' })];
    expect(flowNestingDepth(lib, 'A')).toBeNull();

    // 成环 → 层数无法确定(由成环警示兜底)
    const cyclic = [flow('A', '主', [{ sub_flow_id: 'B' }]), flow('B', '环', [{ sub_flow_id: 'A' }])];
    expect(flowNestingDepth(cyclic, 'A')).toBeNull();
  });

  it('祖先层数:沿反向边向上累加,无人挂载为 0,成环为 null', () => {
    // H → G → F:F 的祖先跳数 2,H 为 0
    const lib = [
      flow('F', '中'),
      flow('G', '上', [{ sub_flow_id: 'F' }]),
      flow('H', '顶', [{ sub_flow_id: 'G' }]),
    ];
    expect(flowAncestorDepth(lib, 'F')).toBe(2);
    expect(flowAncestorDepth(lib, 'G')).toBe(1);
    expect(flowAncestorDepth(lib, 'H')).toBe(0);
    // 库外 id / 成环 → null
    expect(flowAncestorDepth(lib, 'nope')).toBeNull();
    const cyclic = [flow('A', '主', [{ sub_flow_id: 'B' }]), flow('B', '环', [{ sub_flow_id: 'A' }])];
    expect(flowAncestorDepth(cyclic, 'A')).toBeNull();
  });

  it('引用警示:悬空/成环/超深/反思四类都点名「保存会被拒绝」', () => {
    const lib = [flow('A', '主'), flow('B', '子'), flow('C', '二'), flow('D', '三'), flow('E', '四')];
    // 悬空
    const dangling = step('a1', [], { sub_flow_id: 'nope' });
    expect(stepSubFlowWarnings(lib, 'A', dangling)[0]).toContain('不存在');
    expect(stepSubFlowWarnings(lib, 'A', dangling)[0]).toContain('保存会被拒绝');
    // 合法引用:无警示
    lib[1].steps = [step('b1', [], {})];
    expect(stepSubFlowWarnings(lib, 'A', step('a1', [], { sub_flow_id: 'B' }))).toEqual([]);
    // 成环
    lib[1].steps = [step('b1', [], { sub_flow_id: 'A' })];
    expect(stepSubFlowWarnings(lib, 'A', step('a1', [], { sub_flow_id: 'B' }))[0]).toContain('成环');
    // 超深:B→C→D→E 已有 3 跳,再挂到 A 上共 4 跳(上限 3)→ 5 个流程
    lib[1].steps = [step('b1', [], { sub_flow_id: 'C' })];
    lib[2].steps = [step('c1', [], { sub_flow_id: 'D' })];
    lib[3].steps = [step('d1', [], { sub_flow_id: 'E' })];
    const deep = stepSubFlowWarnings(lib, 'A', step('a1', [], { sub_flow_id: 'B' }));
    expect(deep[0]).toContain(`嵌套超过 ${MAX_SUB_FLOW_DEPTH} 层`);
    expect(deep[0]).toContain('共 5 个流程');
    // 反思步骤挂子流程(后端 validate_flow 直接拒)
    const reflect = step('a1', [], { sub_flow_id: 'B', action: 'reflect' });
    expect(stepSubFlowWarnings(lib, 'A', reflect)[0]).toContain('反思步骤');
  });

  it('深度警示把**祖先层数**算进去(本流程自身被挂载时也要报)', () => {
    // H → G → F(本流程,F 的祖先跳数 2);候选 X 自身还有 1 跳 → 2+1+1 = 4 > 3
    const lib = [
      flow('F', '本流程'),
      flow('G', '上', [{ sub_flow_id: 'F' }]),
      flow('H', '顶', [{ sub_flow_id: 'G' }]),
      flow('X', '候选', [{ sub_flow_id: 'W' }]),
      flow('W', '末端'),
    ];
    // 只看「向下的层数」时这条引用看起来完全合法(1 + 1 = 2 ≤ 3)——正是修正前的假阴性
    expect(flowNestingDepth(lib, 'X')).toBe(1);
    const warns = stepSubFlowWarnings(lib, 'F', step('f1', [], { sub_flow_id: 'X' }));
    expect(warns[0]).toContain('嵌套超过');
    expect(warns[0]).toContain('共 5 个流程');
  });

  it('停用步骤不参与校验 → 不给任何警示(启用后才会被保存期拒绝)', () => {
    const lib = [flow('A', '主'), flow('B', '子')];
    const disabled = step('a1', [], { sub_flow_id: 'nope', enabled: false });
    expect(stepSubFlowWarnings(lib, 'A', disabled)).toEqual([]);
    // 同一个步骤启用后照报
    expect(
      stepSubFlowWarnings(lib, 'A', { ...disabled, enabled: true })[0],
    ).toContain('不存在');
  });

  it('流程停用:只有「反思+挂载」那条不报,其余引用链警示仍然成立', () => {
    // 各流程先各带一个合法生成步(否则会先撞上「被引用流程没有启用步骤」那条)
    const lib = [
      flow('A', '主'),
      flow('B', '子', [{}]),
      flow('C', '二', [{}]),
      flow('D', '三', [{}]),
      flow('E', '四', [{}]),
    ];
    // 反思那条由 validate_flow 判,而未启用流程在 validate_flow 里直接放行 → 不报
    const reflect = step('a1', [], { sub_flow_id: 'B', action: 'reflect' });
    expect(stepSubFlowWarnings(lib, 'A', reflect, false)).toEqual([]);
    // 悬空 / 成环 / 超深由全库校验判(它遍历全库、不跳过未启用流程)→ 照报
    expect(
      stepSubFlowWarnings(lib, 'A', step('a1', [], { sub_flow_id: 'nope' }), false)[0],
    ).toContain('不存在');
    lib[1].steps = [step('b1', [], { sub_flow_id: 'A' })];
    expect(
      stepSubFlowWarnings(lib, 'A', step('a1', [], { sub_flow_id: 'B' }), false)[0],
    ).toContain('成环');
    lib[1].steps = [step('b1', [], { sub_flow_id: 'C' })];
    lib[2].steps = [step('c1', [], { sub_flow_id: 'D' })];
    lib[3].steps = [step('d1', [], { sub_flow_id: 'E' })];
    expect(
      stepSubFlowWarnings(lib, 'A', step('a1', [], { sub_flow_id: 'B' }), false)[0],
    ).toContain('嵌套超过');
  });

  it('被引用流程结构不合法也提示(后端按「启用态」校验它)', () => {
    // 没有启用的步骤
    const noActive = flow('空子', '空子', [{ enabled: false }]);
    const lib1 = [flow('A', '主'), noActive];
    const warn1 = stepSubFlowWarnings(lib1, 'A', step('a1', [], { sub_flow_id: '空子' }));
    expect(warn1[0]).toContain('没有启用的步骤');
    expect(warn1[0]).toContain('保存会被拒绝');
    // 缺少生成正文的步骤(只有反思步)
    const noGenerating = flow('反思子', '反思子', [{ action: 'reflect', generates: undefined }]);
    const lib2 = [flow('A', '主'), noGenerating];
    const warn2 = stepSubFlowWarnings(lib2, 'A', step('a1', [], { sub_flow_id: '反思子' }));
    expect(warn2[0]).toContain('缺少生成正文的步骤');
  });

  it('旁路说明点名子流程名且说明配置保留;未挂载返回空串', () => {
    const lib = [flow('A', '主'), flow('B', '摘要流程')];
    const note = subFlowMountNote(lib, 'B');
    expect(note).toContain('摘要流程');
    expect(note).toContain('清空子流程即恢复生效');
    expect(subFlowMountNote(lib, null)).toBe('');
  });

  it('列表行摘要:挂载显示「子流程:名字」,失效点名「引用已失效」,未挂载返回空串', () => {
    const lib = [flow('A', '主'), flow('B', '摘要流程')];
    expect(subFlowSummary(lib, step('a1', [], { sub_flow_id: 'B' }))).toBe('子流程:摘要流程');
    expect(subFlowSummary(lib, step('a1'))).toBe('');
    // 引用已失效(库里没有)时回退显示 id,并点明失效(与画布徽标/下拉同文案)
    expect(subFlowSummary(lib, step('a1', [], { sub_flow_id: 'gone' }))).toBe(
      '子流程:gone(引用已失效)',
    );
  });
});
