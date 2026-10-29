// 综合导入嗅探与归一纯函数回归网(IMPHUB 步骤 C,2026-10-09)。
//
// 覆盖执行稿 §4-C 的形状清单:bundle v1(段序/版本拒绝/未知段/模式过滤)、酒馆预设
// (含内嵌正则提取)、kedai 注入(含禁词迁移)、流程四形态、skills 三形态、插件、
// .md/.txt 文本歧义、段级容错(单段不合法不阻断其余段)+ 追加合并工具。
import { describe, expect, it } from 'vitest';
import type { AgentFlowConfig, PromptInjectConfig } from '../api';
import {
  IMPORT_BUNDLE_KEY,
  appendFloors,
  detectInjectFileShape,
  importReportMessage,
  mergeRegexEntries,
  migrateBannedWordsPrompt,
  normalizeFlowFile,
  parseImportBundle,
  regexDedupeKey,
} from './importBundle';

function flowOf(id: string, name: string): AgentFlowConfig {
  return { id, name, enabled: true, steps: [{ id: `${id}-s`, name: '步', enabled: true, goal: '', action: 'direct' }] };
}

function injectCfg(mode: 'simple' | 'complex' = 'complex'): PromptInjectConfig {
  return {
    mode,
    simple: { banned_words_enabled: false } as PromptInjectConfig['simple'],
    floors: [
      { id: 'f0', name: '楼层0', content: '内容0', role: 'system', position: 'system', depth: 0, enabled: true, order: 0 },
    ],
  };
}

function bundleOf(sections: Record<string, unknown>): Record<string, unknown> {
  return {
    [IMPORT_BUNDLE_KEY]: 1,
    exported_at: '2026-10-09T00:00:00Z',
    generator: { app: 'kedai', version: '0.4.1' },
    app_mode: 'roleplay',
    sections,
  };
}

describe('bundle v1:段序 / 模式过滤 / 版本', () => {
  const full = bundleOf({
    preset_entries: { format: 'kedai_inject', ...injectCfg() },
    flows: { kedai_flow_bundle: 1, root_id: null, flows: [flowOf('f1', '流程一')] },
    prompts: { system: { roleplay: 'RP 词', task: '任务词' }, runtime: '运行时词' },
    skills: [{ name: '技能A', folder: '写作' }],
    regex_scripts: [{ script_name: '状态栏', find_regex: '/<P\\/>/g', replace_string: 'x', enabled: true }],
    plugins: [{ file: 'demo.json', folder: '工具', name: 'demo', description: 'd', script: 'return 1;' }],
  });

  it('角色扮演模式:6 段全出,段序固定 ①→⑦(与后端字段声明序一致)', () => {
    const out = parseImportBundle(full, 'roleplay');
    expect(out.sections.map((s) => s.key)).toEqual([
      'preset_entries', 'flows', 'prompts', 'skills', 'regex_scripts', 'plugins',
    ]);
    expect(out.skipped).toEqual([]);
  });

  it('任务模式:①⑥ 不出现且登记 skipped(原因写明「角色扮演专属」)', () => {
    const out = parseImportBundle(full, 'task');
    expect(out.sections.map((s) => s.key)).toEqual(['flows', 'prompts', 'skills', 'plugins']);
    const skippedKeys = out.skipped.map((s) => s.key);
    expect(skippedKeys).toContain('preset_entries');
    expect(skippedKeys).toContain('regex_scripts');
    expect(out.skipped.every((s) => s.reason.includes('角色扮演专属'))).toBe(true);
  });

  it('版本 >1 拒绝(提示升级);缺 sections 拒绝', () => {
    expect(() => parseImportBundle({ [IMPORT_BUNDLE_KEY]: 2, sections: {} }, 'roleplay')).toThrow(/更新版本/);
    expect(() => parseImportBundle({ [IMPORT_BUNDLE_KEY]: 1 }, 'roleplay')).toThrow(/sections/);
  });

  it('段内未知键忽略并计入 skipped(前向兼容);空段不导入', () => {
    const out = parseImportBundle(
      bundleOf({ flows: { kedai_flow_bundle: 1, flows: [flowOf('f1', '甲')] }, future_thing: { a: 1 }, skills: [] }),
      'roleplay',
    );
    expect(out.sections.map((s) => s.key)).toEqual(['flows']);
    const unknown = out.skipped.find((s) => s.key === 'future_thing');
    expect(unknown?.reason).toContain('未知段');
    // skills: [] 空段 → 不导入也不阻断 flows
    expect(out.skipped.some((s) => s.key === 'skills')).toBe(true);
  });

  it('段内容不合法(非对象)→ 该段 skipped,其余段照常', () => {
    const out = parseImportBundle(
      bundleOf({ flows: { flows: 'not-an-object' }, prompts: { runtime: '词' } }),
      'roleplay',
    );
    expect(out.sections.map((s) => s.key)).toEqual(['prompts']);
    expect(out.skipped.some((s) => s.key === 'flows')).toBe(true);
  });

  it('④ prompts 段:只出现非空部分进 summary;全空 → 段 skipped', () => {
    const section = parseImportBundle(bundleOf({ prompts: { system: { roleplay: 'A', task: '' }, runtime: '' } }), 'roleplay').sections[0];
    expect(section.summary).toContain('角色扮演');
    expect(section.summary).not.toContain('任务)');
    const empty = parseImportBundle(bundleOf({ prompts: { runtime: '' } }), 'roleplay');
    expect(empty.sections).toEqual([]);
    expect(empty.skipped.some((s) => s.key === 'prompts')).toBe(true);
  });
});

describe('酒馆预设(顶层 prompts)', () => {
  const st = {
    name: '可待',
    prompts: [
      { identifier: 'a', name: 'A', content: 'x', role: 'system' },
      { identifier: 'b', name: 'B', content: 'y', role: 'user' },
    ],
    prompt_order: [],
    extensions: {
      regex_scripts: [
        { scriptName: '状态栏', findRegex: '/<P\\/>/g', replaceString: '<i>x</i>' },
        { scriptName: '去标记', findRegex: '<X/>', replaceString: '' },
      ],
    },
  };

  it('角色扮演:① 段(整文件交后端)+ ⑥ 段(内嵌正则进全局库)同时产出', () => {
    const out = parseImportBundle(st, 'roleplay', { fileName: 'preset.json' });
    expect(out.sections.map((s) => s.key)).toEqual(['preset_entries', 'regex_scripts']);
    const preset = out.sections[0];
    if (preset.key !== 'preset_entries' || preset.payload.format !== 'st_preset') throw new Error('段形状错误');
    expect(preset.payload.text).toContain('prompts');
    expect(preset.count).toBe(2);
    expect(out.sections[1].count).toBe(2);
  });

  it('无内嵌正则时不产出 ⑥ 段;任务模式两段都 skipped', () => {
    const noRegex = { ...st, extensions: {} };
    expect(parseImportBundle(noRegex, 'roleplay').sections.map((s) => s.key)).toEqual(['preset_entries']);
    const task = parseImportBundle(st, 'task');
    expect(task.sections).toEqual([]);
    expect(task.skipped.map((s) => s.key)).toEqual(['preset_entries', 'regex_scripts']);
  });

  it('detectInjectFileShape 与推导一致(prompts 数组 → st)', () => {
    expect(detectInjectFileShape(st)).toBe('st');
    expect(detectInjectFileShape(injectCfg())).toBe('kedai');
  });
});

describe('kedai 注入配置与禁词迁移', () => {
  it('mode+simple → ① 段 kedai_inject;禁词迁移只补空 banned_prompt', () => {
    const payload = {
      mode: 'simple',
      simple: {
        banned_words_enabled: true,
        banned_words: [{ word: '笨蛋' }, { word: '  ' }, { word: '脏话' }],
      },
      floors: [],
    };
    const out = parseImportBundle(payload, 'roleplay');
    const section = out.sections[0];
    if (section.key !== 'preset_entries' || section.payload.format !== 'kedai_inject') throw new Error('段形状错误');
    expect(section.payload.config.simple.banned_prompt).toContain('笨蛋');
    expect(section.payload.config.simple.banned_prompt).toContain('脏话');
    expect(section.payload.config.simple.banned_prompt).not.toContain('、\u3000');

    // 已有 banned_prompt 不覆盖
    const kept = migrateBannedWordsPrompt({
      ...injectCfg(),
      simple: { banned_words: [{ word: 'x' }], banned_prompt: '原提示' } as PromptInjectConfig['simple'],
    });
    expect(kept.simple.banned_prompt).toBe('原提示');
  });

  it('任务模式:① 不出现,skipped 记「角色扮演专属」', () => {
    const out = parseImportBundle(injectCfg(), 'task');
    expect(out.sections).toEqual([]);
    expect(out.skipped[0].key).toBe('preset_entries');
  });
});

describe('流程四形态 / skills 三形态 / 插件 / 文本', () => {
  it('流程:搬运包带 root_id;库格式整库;{config} 包装;单流程', () => {
    const packed = parseImportBundle(
      { kedai_flow_bundle: 1, root_id: 'f1', flows: [flowOf('f1', '甲')] },
      'roleplay',
    );
    const p = packed.sections[0];
    if (p.key !== 'flows') throw new Error('应为 flows 段');
    expect(p.payload.rootId).toBe('f1');

    const lib = parseImportBundle({ flows: [flowOf('a', 'A'), flowOf('b', 'B')], current_flow_id: 'b' }, 'roleplay');
    const l = lib.sections[0];
    if (l.key !== 'flows') throw new Error('应为 flows 段');
    expect(l.count).toBe(2);
    expect(l.payload.rootId).toBe('b');

    expect(parseImportBundle({ config: flowOf('', '包装') }, 'roleplay').sections[0].key).toBe('flows');
    expect(parseImportBundle(flowOf('', '裸'), 'roleplay').sections[0].key).toBe('flows');
  });

  it('流程版本不是 1 → 明确拒绝(不静默丢字段)', () => {
    expect(() => parseImportBundle({ kedai_flow_bundle: 2, flows: [flowOf('f', 'x')] }, 'roleplay')).toThrow(/版本不支持/);
    expect(() => normalizeFlowFile({ flows: [{ name: '没有 steps' }] })).toThrow(/steps 数组/);
  });

  it('skills:数组 / {skills:[]} 包壳 / 单对象(无 script)', () => {
    const arr = parseImportBundle([{ name: 'A', content: 'a' }, { name: 'B', folder: 'x' }], 'roleplay');
    expect(arr.sections[0].key).toBe('skills');
    expect(arr.sections[0].count).toBe(2);

    const wrapped = parseImportBundle({ skills: [{ name: 'A' }] }, 'roleplay');
    expect(wrapped.sections[0].key).toBe('skills');

    const single = parseImportBundle({ name: '单个技能', description: 'd' }, 'roleplay');
    expect(single.sections[0].key).toBe('skills');
  });

  it('插件:name+script 非空 → ⑦ 段,file 取所选文件名;缺 script 不算插件', () => {
    const out = parseImportBundle(
      { name: 'demo', description: 'd', script: 'return 1;' },
      'roleplay',
      { fileName: 'my-plugin.json' },
    );
    const s = out.sections[0];
    if (s.key !== 'plugins') throw new Error('应为 plugins 段');
    expect(s.payload[0].file).toBe('my-plugin.json');
    expect(s.payload[0].name).toBe('demo');
  });

  it('文本(.md/.txt)→ ③④ prompts(默认 runtime;不做猜测)', () => {
    const out = parseImportBundle('# 提示词\n内容', 'task', { fileName: 'p.md' });
    const s = out.sections[0];
    if (s.key !== 'prompts') throw new Error('应为 prompts 段');
    expect(s.payload.runtime).toContain('# 提示词');
    expect(s.payload.system.roleplay).toBe('');
    expect(() => parseImportBundle('   ', 'roleplay')).toThrow(/为空/);
  });

  // IMPHUB-L3:摘要曾写「(预览内可切换为系统提示词)」,但该段 option 恒为 null、
  // 预览把 summary 当纯文本渲染、执行侧只走 runtime——界面承诺了不存在的能力。
  // 收窄取「删文案」一侧(零行为风险),用例锁死「不得再出现切换承诺」。
  it('文本段摘要不得承诺不存在的注入位置切换(IMPHUB-L3)', () => {
    const out = parseImportBundle('# 提示词\n内容', 'roleplay', { fileName: 'p.md' });
    const s = out.sections[0];
    if (s.key !== 'prompts') throw new Error('应为 prompts 段');
    expect(s.summary).not.toContain('可切换');
    expect(s.summary).toContain('运行时主提示词');
    expect(s.option).toBeNull();
  });

  it('全部不命中 → 抛出,文案列出支持范围', () => {
    expect(() => parseImportBundle({ foo: 1 }, 'roleplay')).toThrow(/无法识别/);
    expect(() => parseImportBundle(null, 'roleplay')).toThrow(/无法识别/);
  });

  it('importReportMessage 覆盖段文案(覆盖模式注明不可逆)', () => {
    expect(importReportMessage({ imported: 2, skipped: 0, renamed: [], replaced: [] })).toBe('已导入 2 个流程');
    expect(
      importReportMessage({ imported: 1, skipped: 1, renamed: [], replaced: [{ id: 'a', name: '甲' }] }),
    ).toContain('覆盖 1 个同 id 流程');
  });
});

describe('追加合并工具(纯函数)', () => {
  it('appendFloors:接在末尾重编 order;id 冲突给导入条目新 id 不串台', () => {
    const current = injectCfg();
    const incoming: PromptInjectConfig = {
      mode: 'complex',
      simple: {} as PromptInjectConfig['simple'],
      floors: [
        { id: 'f0', name: '撞 id', content: 'c', role: 'user', position: 'system', depth: 0, enabled: true, order: 9 },
        { id: 'g1', name: '新楼层', content: 'd', role: 'user', position: 'system', depth: 0, enabled: true, order: 8 },
      ],
    };
    const merged = appendFloors(current, incoming);
    expect(merged.floors.map((f) => f.name)).toEqual(['楼层0', '撞 id', '新楼层']);
    expect(merged.floors.map((f) => f.order)).toEqual([0, 1, 2]);
    const ids = merged.floors.map((f) => f.id);
    expect(new Set(ids).size).toBe(ids.length);
    // 本机那份不被就地改写(纯函数)
    expect(current.floors).toHaveLength(1);
  });

  it('mergeRegexEntries:name+find_regex 两套键名去重;空键不去重直接追加', () => {
    const key = regexDedupeKey({ script_name: 'A', find_regex: 'x' });
    expect(key).toBe(regexDedupeKey({ scriptName: 'A', findRegex: 'x' }));
    const { merged, added } = mergeRegexEntries(
      [{ script_name: 'A', find_regex: 'x' }],
      [
        { scriptName: 'A', findRegex: 'x' }, // 同键 → 跳过
        { script_name: 'B', find_regex: 'y' },
        { replace_string: '只有替换体' }, // 空键 → 追加(后端再按宽进归一)
      ],
    );
    expect(added).toBe(2);
    expect(merged).toHaveLength(3);
  });
});
