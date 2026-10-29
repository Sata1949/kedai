// 综合导入(bundle v1)嗅探与归一纯函数(IMPHUB 步骤 C,2026-10-09)。
//
// 职责:把「用户选中的一个文件」按执行稿 §2.1.5 的嗅探表判定为段集合,并把文件形状
// 归一到逐段载荷(后端端点可直接消费)。本模块**纯函数、零 IO**——读文件/发请求在
// 组件层(ImportBundleModal),故可在 node 环境全量单测。
//
// 为什么纯函数集中在这里而不是留在 composable:同一份判定被两处消费(组件执行路径 +
// 单测),复制实现必然漂移;`detectInjectFileShape` 与 `normalizeFlowFile` 原住
// usePromptInject/useAgentFlow,迁入本模块后由原 composable **re-export**(既有
// 测试与调用方零改动)。
import type {
  AgentFlowConfig,
  FlowImportReport,
  PromptInjectConfig,
  SkillImportItem,
} from '../api';

/** bundle 版本键(`kedai_import_bundle` 的值;后端 IMPORT_BUNDLE_VERSION 同一契约) */
export const IMPORT_BUNDLE_KEY = 'kedai_import_bundle';
export const IMPORT_BUNDLE_VERSION = 1;

/** 搬运包版本键与版本号(与后端 `agent_flow_service::FLOW_BUNDLE_*` 对齐) */
export const FLOW_BUNDLE_KEY = 'kedai_flow_bundle';
export const FLOW_BUNDLE_VERSION = 1;

/** 顶层运行模式(与 onboarding.ts 的 AppMode 同域;避免 L1 内互相 import 只取类型) */
export type BundleMode = 'roleplay' | 'task';

// ==================== 形状判定(原 usePromptInject / useAgentFlow 迁入) ====================

export type InjectFileShape = 'st' | 'kedai' | 'unknown';

/** 判定注入配置文件的形状:顶层 prompts 数组 → 酒馆预设(与后端 parsing/preset.rs 的唯一依据对齐);
 *  mode 字符串 + simple 字段 → 本工具导出配置(顶层形状见 exportInjectConfig);其余 → unknown。
 *  注意 floors 不是酒馆预设判据(那是本工具自有楼层形状,导出配置里也有,故置于 kedai 之后) */
export function detectInjectFileShape(data: unknown): InjectFileShape {
  if (!data || typeof data !== 'object') return 'unknown';
  const d = data as Record<string, unknown>;
  if (Array.isArray(d.prompts)) return 'st';
  if (typeof d.mode === 'string' && d.simple !== undefined) return 'kedai';
  return 'unknown';
}

/**
 * 归一流程文件 → 待导入的 flows[](+ 可选入口 root_id)。
 *
 * 支持四种形状(旧文件必须继续能导,新增的搬运包才有版本键):
 *  - 搬运包 `{kedai_flow_bundle:1, root_id, flows:[…]}`:整包导入,root_id 一并带上;
 *  - 库格式 `{flows:[…], current_flow_id}`:**整库**导入(此前只取当前/第一个,会静默丢流程);
 *  - `{config:{…}}` 包装:取其中的单流程;
 *  - 单流程 `{name,enabled,steps:[…]}`:一份。
 *
 * 版本键存在但不是 1 → 明确拒绝(新版字段旧端读不懂,静默丢字段比报错更糟)。
 */
export function normalizeFlowFile(raw: unknown): {
  flows: AgentFlowConfig[];
  rootId?: string;
} {
  if (!raw || typeof raw !== 'object') {
    throw new Error('无法识别的流程文件:内容不是 JSON 对象');
  }
  const obj = raw as Record<string, unknown>;
  if (obj[FLOW_BUNDLE_KEY] !== undefined && obj[FLOW_BUNDLE_KEY] !== FLOW_BUNDLE_VERSION) {
    throw new Error(
      `流程包版本不支持(${String(obj[FLOW_BUNDLE_KEY])}),请升级 Kedai 后再导入`,
    );
  }
  const asFlow = (v: unknown): AgentFlowConfig | null =>
    v && typeof v === 'object' && Array.isArray((v as AgentFlowConfig).steps)
      ? (v as AgentFlowConfig)
      : null;
  if (Array.isArray(obj.flows)) {
    const flows = (obj.flows as unknown[]).map(asFlow);
    if (flows.length === 0 || flows.some((f) => f === null)) {
      throw new Error('流程文件里的 flows 不含合法流程(每项都需要 steps 数组)');
    }
    // 入口:搬运包用 root_id;库格式用 current_flow_id(否则导入后只选中「第一个」)
    const root =
      typeof obj.root_id === 'string'
        ? obj.root_id
        : typeof obj.current_flow_id === 'string'
          ? obj.current_flow_id
          : undefined;
    return { flows: flows as AgentFlowConfig[], rootId: root };
  }
  const single = asFlow(obj.config) ?? asFlow(obj);
  if (!single) {
    throw new Error('无法识别的流程文件:需包含 steps 数组(单流程 / {config} 包装 / 流程库 / 搬运包)');
  }
  return { flows: [single] };
}

/** 导入报告 → 用户文案(导入 N / 跳过 M / 其中 K 个分配了新 id / 覆盖 J 个) */
export function importReportMessage(report: FlowImportReport): string {
  const parts = [`已导入 ${report.imported} 个流程`];
  if (report.skipped > 0) parts.push(`跳过 ${report.skipped} 个(内容已存在)`);
  if (report.renamed.length > 0) {
    const names = report.renamed.map((r) => `「${r.name || r.old_id}」`).join('、');
    parts.push(`${report.renamed.length} 个因 id 冲突分配了新 id:${names}`);
  }
  // 覆盖段(B 批 B4):仅 `on_conflict=replace` 才可能非空。容缺读取(replaced 缺失 =
  // 旧服务端的报告,不给用户报「导入失败」),空数组则一字不出现(缺省的 rename 路径不变)
  const replaced = report.replaced ?? [];
  if (replaced.length > 0) {
    const names = replaced.map((r) => `「${r.name || r.id}」`).join('、');
    parts.push(`覆盖 ${replaced.length} 个同 id 流程(本机原有那份已被替换):${names}`);
  }
  return parts.join(';');
}

/**
 * **覆盖模式**导入的二次确认文案(B 批 B4;覆盖不可逆,故每次导入前都问一遍)。
 * 说清三件事:只动同 id 的那几份、id 不变引用不破、无法撤销——「本机其它流程不动」
 * 一并写明,否则用户会以为整库被替换。
 */
export function importReplaceHint(count: number): string {
  return (
    `本次将导入 ${count} 个流程,并开启「覆盖同名流程」。\n` +
    '与本机同 id 且内容不同的流程会被替换(id 不变,引用不会断),该操作无法撤销;\n' +
    '本机其它流程不受影响。确定继续吗?'
  );
}

// ==================== 段模型 ====================

/** bundle v1 的 6 个固定段键(①~⑦;③④ 合为一段) */
export type ImportSectionKey =
  | 'preset_entries'
  | 'flows'
  | 'prompts'
  | 'skills'
  | 'regex_scripts'
  | 'plugins';

/** 段键 → 中文名(预览与回执共用,单点定义) */
export const SECTION_LABELS: Record<ImportSectionKey, string> = {
  preset_entries: '①预设条目',
  flows: '②自定义执行流程',
  prompts: '③④提示词(系统 / 运行时)',
  skills: '⑤技能',
  regex_scripts: '⑥预设正则',
  plugins: '⑦插件',
};

/** 角色扮演专属段(①⑥):任务模式下不出现、不发送,回执记 skipped */
export const ROLEPLAY_ONLY_SECTIONS: ReadonlySet<ImportSectionKey> = new Set([
  'preset_entries',
  'regex_scripts',
]);

/** 段内的可选项组(UI 按此渲染段级策略下拉;null = 无选项) */
export type SectionOptionKind = 'preset' | 'flows' | 'regex';

/** ① 预设条目载荷:kedai 注入形态(直接 PUT)或酒馆预设原文件(交后端 multipart 解析) */
export type PresetEntriesPayload =
  | { format: 'kedai_inject'; config: PromptInjectConfig }
  | { format: 'st_preset'; fileName: string; text: string };

/** ② 流程段载荷(归一后的 flows[] + 可选入口) */
export interface FlowsPayload {
  flows: AgentFlowConfig[];
  rootId?: string;
}

/** ③④ 提示词段载荷(双变体 + 运行时;**空串 = 未自定义,导入侧不写**) */
export interface PromptsPayload {
  system: { roleplay: string; task: string };
  runtime: string;
}

/** ⑦ 插件段条目(与后端 import/export 端点同形) */
export interface PluginEntryPayload {
  file: string;
  folder?: string;
  name: string;
  description?: string;
  parameters?: unknown;
  script: string;
}

/** 一个解析完成的段(判别联合:按 key 收窄载荷类型) */
export type ImportSection =
  | { key: 'preset_entries'; label: string; summary: string; count: number; option: SectionOptionKind; payload: PresetEntriesPayload }
  | { key: 'flows'; label: string; summary: string; count: number; option: SectionOptionKind; payload: FlowsPayload }
  | { key: 'prompts'; label: string; summary: string; count: number; option: null; payload: PromptsPayload }
  | { key: 'skills'; label: string; summary: string; count: number; option: null; payload: SkillImportItem[] }
  // 正则条目**宽进**:kedai snake_case 与酒馆 camelCase 都由后端 normalize_script 归一,
  // 故前端不替后端挑形状(只数条目),避免两套归一实现漂移。
  | { key: 'regex_scripts'; label: string; summary: string; count: number; option: SectionOptionKind; payload: Record<string, unknown>[] }
  | { key: 'plugins'; label: string; summary: string; count: number; option: null; payload: PluginEntryPayload[] };

/** 模式不适用 / 未知段(bundle 内未知键)→ 预览与回执如实登记 */
export interface SkippedSection {
  key: string;
  label: string;
  reason: string;
}

export interface ParsedBundle {
  /** 来源描述(预览头部;如「综合导出件 v1」「酒馆预设」) */
  source: string;
  /** 当前模式下可用、已归一的段(段序固定:preset_entries → flows → prompts → skills → regex_scripts → plugins) */
  sections: ImportSection[];
  /** 不导入的段(模式不适用 / 未知段键;原因写 reason) */
  skipped: SkippedSection[];
}

/** 段序契约(与后端 BundleSections 字段声明序一致;前端预览与执行都按此序) */
export const SECTION_KEYS: readonly ImportSectionKey[] = [
  'preset_entries',
  'flows',
  'prompts',
  'skills',
  'regex_scripts',
  'plugins',
];

// ==================== 段内归一工具(纯函数,单测锁定) ====================

/**
 * kedai 注入配置的禁词库迁移(原 usePromptInject.onImportPreset 内联逻辑):
 * 旧格式只带 `banned_words`(词条表)、`banned_prompt` 为空时,把词条拼成自省提示词。
 * 迁移只补空值,不覆盖已有 banned_prompt。
 */
/**
 * 把「已判定为对象」的注入配置载荷收敛成 `PromptInjectConfig`(缺 `floors` 视为空数组)。
 * 字段值本身不在此校验——导入落点 `PUT /api/prompt-inject` 的后端侧才是权威校验方,
 * 前端只做形状收敛(避免整对象的双重断言逃逸)。
 */
function toInjectConfig(v: Record<string, unknown>): PromptInjectConfig {
  return {
    mode: v.mode as PromptInjectConfig['mode'],
    simple: v.simple as PromptInjectConfig['simple'],
    floors: (v.floors ?? []) as PromptInjectConfig['floors'],
  };
}

export function migrateBannedWordsPrompt(config: PromptInjectConfig): PromptInjectConfig {
  const cfg = JSON.parse(JSON.stringify(config)) as PromptInjectConfig;
  if (cfg.simple?.banned_words?.length && !cfg.simple?.banned_prompt) {
    const words = cfg.simple.banned_words
      .map((b) => b.word?.trim())
      .filter((w): w is string => !!w);
    if (words.length > 0) {
      cfg.simple.banned_prompt = `输出中禁止出现以下词语,若涉及请用含义相近、更得体的表达替换:${words.join('、')}`;
    }
  }
  return cfg;
}

/**
 * 楼层**追加**合并(kedai 注入形态的 append 策略;后端 multipart 的 append 只覆盖 ST 路径):
 * 保留本机全部楼层,导入楼层接在末尾并重编 order;id 冲突时给导入条目分配新 id
 * (楼层 id 是渲染/编辑的键,覆盖会串台)。
 */
export function appendFloors(
  current: PromptInjectConfig,
  incoming: PromptInjectConfig,
): PromptInjectConfig {
  const merged = JSON.parse(JSON.stringify(current)) as PromptInjectConfig;
  const used = new Set(merged.floors.map((f) => f.id));
  for (const floor of incoming.floors) {
    const copy = JSON.parse(JSON.stringify(floor)) as PromptInjectConfig['floors'][number];
    if (used.has(copy.id)) {
      copy.id = `${copy.id}-new${merged.floors.length}`;
    }
    used.add(copy.id);
    merged.floors.push(copy);
  }
  merged.floors.forEach((f, i) => (f.order = i));
  return merged;
}

/** 正则条目的去重键:兼容 kedai snake_case 与酒馆 camelCase 两套键名(与后端宽进口径对称) */
export function regexDedupeKey(entry: unknown): string {
  const obj = entry && typeof entry === 'object' ? (entry as Record<string, unknown>) : {};
  const str = (v: unknown): string => (typeof v === 'string' ? v.trim() : '');
  const name = str(obj.script_name) || str(obj.scriptName);
  const find = str(obj.find_regex) || str(obj.findRegex);
  return `${name}\u0000${find}`;
}

/**
 * 正则**追加**合并:按 name+find_regex 去重(本机已有同键条目时不重复导入);
 * 返回 (合并后全量, 实际新增数)。替换策略走整体 PUT,不经本函数。
 * 入参取 `unknown`(本机库的 `RegexScript` 与文件里的任意键名形态都要能进来)。
 */
export function mergeRegexEntries(
  current: readonly unknown[],
  incoming: readonly unknown[],
): { merged: unknown[]; added: number } {
  const seen = new Set(current.map(regexDedupeKey));
  const merged = [...current];
  let added = 0;
  for (const entry of incoming) {
    const key = regexDedupeKey(entry);
    if (key !== '\u0000' && seen.has(key)) continue;
    seen.add(key);
    merged.push(entry);
    added += 1;
  }
  return { merged, added };
}

// ==================== 主入口:parseImportBundle ====================

function isObject(v: unknown): v is Record<string, unknown> {
  return !!v && typeof v === 'object' && !Array.isArray(v);
}

/** bundle v1 的 sections 对象 → 段集合(未知键登记进 skipped;模式不适用段亦登记) */
function parseBundleSections(
  sectionsObj: Record<string, unknown>,
  mode: BundleMode,
): { sections: ImportSection[]; skipped: SkippedSection[] } {
  const sections: ImportSection[] = [];
  const skipped: SkippedSection[] = [];
  for (const key of SECTION_KEYS) {
    const raw = sectionsObj[key];
    if (raw === undefined) continue; // 缺省 = 该段不导入(不回退任何默认值)
    if (mode === 'task' && ROLEPLAY_ONLY_SECTIONS.has(key)) {
      skipped.push({
        key,
        label: SECTION_LABELS[key],
        reason: '角色扮演专属段,任务模式下不导入',
      });
      continue;
    }
    const parsed = parseBundleSection(key, raw);
    if (!parsed) {
      skipped.push({ key, label: SECTION_LABELS[key], reason: '段内容为空或形状不合法' });
      continue;
    }
    if ('error' in parsed) {
      skipped.push({ key, label: SECTION_LABELS[key], reason: parsed.error });
      continue;
    }
    if (parsed.entry.key !== key) {
      // 理论上不可达(各分支自洽);留断言防回归时静默错段
      throw new Error(`段 ${key} 解析结果错位:${parsed.entry.key}`);
    }
    sections.push(parsed.entry);
    skipped.push(...parsed.skipped);
  }
  // sections 内的未知段键:忽略并计入回执(前向兼容)
  for (const key of Object.keys(sectionsObj)) {
    if (!(SECTION_KEYS as string[]).includes(key)) {
      skipped.push({ key, label: key, reason: '未知段(文件由更新版本导出?),已忽略' });
    }
  }
  return { sections, skipped };
}

/** 单段解析结果:成功给 entry;失败给 error(登记 skipped,不阻断其它段) */
type SectionParseResult =
  | { entry: ImportSection; skipped: SkippedSection[] }
  | { error: string };

/** 单段解析:null/error = 段整体视为空/不合法(登记 skipped,不阻断其它段) */
function parseBundleSection(key: ImportSectionKey, raw: unknown): SectionParseResult | null {
  const skipped: SkippedSection[] = [];
  switch (key) {
    case 'preset_entries': {
      if (!isObject(raw)) return null;
      const format = typeof raw.format === 'string' ? raw.format : 'kedai_inject';
      if (format === 'st_preset') {
        const text = typeof raw.text === 'string' ? raw.text : '';
        if (!text) return null;
        return {
          entry: {
            key,
            label: SECTION_LABELS[key],
            count: 1,
            summary: '酒馆预设原文件(导入时由后端解析楼层;替换 / 追加可选)',
            option: 'preset',
            payload: { format: 'st_preset', fileName: 'preset.json', text },
          },
          skipped,
        };
      }
      // kedai_inject:本体是 PromptInjectConfig(mode/simple/floors 与顶层导出同形)
      const cfg = migrateBannedWordsPrompt(toInjectConfig(raw));
      if (typeof cfg.mode !== 'string' || !cfg.simple) return null;
      return {
        entry: {
          key,
          label: SECTION_LABELS[key],
          count: cfg.floors?.length ?? 0,
          summary: `注入配置(${cfg.mode === 'simple' ? '简单' : '复杂'}模式,${cfg.floors?.length ?? 0} 条楼层)`,
          option: 'preset',
          payload: { format: 'kedai_inject', config: cfg },
        },
        skipped,
      };
    }
    case 'flows': {
      if (!isObject(raw)) return null;
      // 段内不合法形状(如 flows 不是数组):隔离为本段失败,不阻断其余段
      let normalized: { flows: AgentFlowConfig[]; rootId?: string };
      try {
        normalized = normalizeFlowFile(raw as unknown);
      } catch (e) {
        return { error: `流程段解析失败:${(e as Error).message}` };
      }
      const { flows, rootId } = normalized;
      if (flows.length === 0) return null;
      return {
        entry: {
          key,
          label: SECTION_LABELS[key],
          count: flows.length,
          summary: `${flows.length} 个流程(含子流程;默认新增副本,可选覆盖同 id)`,
          option: 'flows',
          payload: { flows, rootId },
        },
        skipped,
      };
    }
    case 'prompts': {
      if (!isObject(raw)) return null;
      const systemRaw = isObject(raw.system) ? raw.system : {};
      const str = (v: unknown): string => (typeof v === 'string' ? v : '');
      const system = { roleplay: str(systemRaw.roleplay), task: str(systemRaw.task) };
      const runtime = str(raw.runtime);
      if (!system.roleplay && !system.task && !runtime) return null;
      const parts: string[] = [];
      if (system.roleplay) parts.push('系统提示词(角色扮演)');
      if (system.task) parts.push('系统提示词(任务)');
      if (runtime) parts.push('运行时主提示词');
      return {
        entry: {
          key,
          label: SECTION_LABELS[key],
          count: parts.length,
          summary: `${parts.join(' + ')};只写当前模式变体,空值不覆盖`,
          option: null,
          payload: { system, runtime },
        },
        skipped,
      };
    }
    case 'skills': {
      const items = Array.isArray(raw) ? raw : null;
      if (!items || items.length === 0) return null;
      const skills = items.filter((v): boolean => isObject(v)) as SkillImportItem[];
      if (skills.length === 0) return null;
      if (skills.length !== items.length) {
        skipped.push({
          key,
          label: SECTION_LABELS[key],
          reason: `跳过 ${items.length - skills.length} 条非对象技能条目`,
        });
      }
      return {
        entry: {
          key,
          label: SECTION_LABELS[key],
          count: skills.length,
          summary: `${skills.length} 个技能(同名覆盖,folder 以导入值为准)`,
          option: null,
          payload: skills,
        },
        skipped,
      };
    }
    case 'regex_scripts': {
      const items = Array.isArray(raw) ? raw : null;
      if (!items || items.length === 0) return null;
      const scripts = items.filter(isObject) as Record<string, unknown>[];
      return {
        entry: {
          key,
          label: SECTION_LABELS[key],
          count: scripts.length,
          summary: `${scripts.length} 条正则(显示层正则;替换 / 追加可选)`,
          option: 'regex',
          payload: scripts,
        },
        skipped,
      };
    }
    case 'plugins': {
      const items = Array.isArray(raw) ? raw : null;
      if (!items || items.length === 0) return null;
      const plugins: PluginEntryPayload[] = [];
      let invalid = 0;
      for (const it of items) {
        if (!isObject(it)) {
          invalid += 1;
          continue;
        }
        const file = typeof it.file === 'string' ? it.file : '';
        const name = typeof it.name === 'string' ? it.name : '';
        const script = typeof it.script === 'string' ? it.script : '';
        if (!file || !name || !script) {
          invalid += 1;
          continue;
        }
        plugins.push({
          file,
          folder: typeof it.folder === 'string' ? it.folder : '',
          name,
          description: typeof it.description === 'string' ? it.description : '',
          parameters: it.parameters,
          script,
        });
      }
      if (plugins.length === 0) return null;
      if (invalid > 0) {
        skipped.push({
          key,
          label: SECTION_LABELS[key],
          reason: `跳过 ${invalid} 条缺少 file/name/script 的插件条目`,
        });
      }
      return {
        entry: {
          key,
          label: SECTION_LABELS[key],
          count: plugins.length,
          summary: `${plugins.length} 个插件(同名文件覆盖,导入后自动重新注册)`,
          option: null,
          payload: plugins,
        },
        skipped,
      };
    }
  }
}

/**
 * 解析一个**文件内容**为段集合(综合导入的预览与执行共用入口)。
 *
 * 判定顺序(§2.1.5 嗅探表,先特异后一般):
 *   1. 文本(.md/.txt 由调用方原样传入)→ ③④ 提示词(默认 runtime;歧义显式化,不做猜测)
 *   2. bundle v1(`kedai_import_bundle` 为整数;>1 拒绝)
 *   3. 酒馆预设(顶层 prompts 数组)→ ①(+ extensions.regex_scripts 非空时 ⑥)
 *   4. kedai 注入配置(mode 字符串 + simple 对象)→ ①
 *   5. 流程四形态 → ②
 *   6. skills 三形态(数组 / 单对象)→ ⑤
 *   7. 插件 JSON(name + script 非空)→ ⑦
 * 全部不命中 → 抛出(错误文案面向用户,列出支持范围)。
 *
 * `opts.fileName` 仅用于 ⑦ 段(文件名是资产标识;缺省 `plugin.json`)。
 */
export function parseImportBundle(
  data: unknown,
  mode: BundleMode,
  opts: { fileName?: string } = {},
): ParsedBundle {
  // 1. 文本文件(.md/.txt):顶层一定是字符串(调用方按扩展名分流)
  if (typeof data === 'string') {
    const text = data.trim();
    if (!text) throw new Error('文件内容为空');
    return {
      source: `文本提示词${opts.fileName ? `(${opts.fileName})` : ''}`,
      sections: [
        {
          key: 'prompts',
          label: SECTION_LABELS.prompts,
          count: 1,
          // IMPHUB-L3:此处曾写「(预览内可切换为系统提示词)」,但本段 option 恒为 null、
          // 预览把 summary 当纯文本渲染、执行侧只走 runtime —— 承诺了不存在的能力。
          // 按计划 IMPHUB-F1 的收窄一侧(②)删去该句,零行为变化。
          summary: '一段文本,作为运行时主提示词导入',
          option: null,
          payload: { system: { roleplay: '', task: '' }, runtime: text },
        },
      ],
      skipped: [],
    };
  }

  if (!isObject(data)) {
    // 顶层数组 = skills 三形态之一(数组);其余非对象内容拒绝
    if (Array.isArray(data)) {
      const parsed = parseBundleSection('skills', data);
      if (parsed && !('error' in parsed) && parsed.entry.key === 'skills') {
        return { source: '技能文件', sections: [parsed.entry], skipped: parsed.skipped };
      }
    }
    throw new Error('无法识别的文件内容:不是 JSON 对象或文本');
  }
  const obj = data as Record<string, unknown>;

  // 2. bundle v1
  if (obj[IMPORT_BUNDLE_KEY] !== undefined) {
    const v = obj[IMPORT_BUNDLE_KEY];
    if (typeof v !== 'number' || !Number.isInteger(v)) {
      throw new Error(`不是有效的综合导出件:${IMPORT_BUNDLE_KEY} 应为整数`);
    }
    if (v > IMPORT_BUNDLE_VERSION) {
      throw new Error(`文件由更新版本的 Kedai 导出(bundle v${v}),请升级后再导入`);
    }
    const sectionsObj = obj.sections;
    if (!isObject(sectionsObj)) {
      throw new Error('综合导出件缺少 sections 对象');
    }
    const { sections, skipped } = parseBundleSections(sectionsObj, mode);
    const srcMode = typeof obj.app_mode === 'string' ? obj.app_mode : '?';
    return { source: `综合导出件 v${v}(导出方模式:${srcMode})`, sections, skipped };
  }

  // 3. 酒馆预设:楼层(整文件交后端)+ 内嵌正则(进全局库,收口 PAD-2)
  if (Array.isArray(obj.prompts)) {
    const prompts = obj.prompts;
    const sections: ImportSection[] = [];
    const skipped: SkippedSection[] = [];
    if (mode === 'task') {
      skipped.push({
        key: 'preset_entries',
        label: SECTION_LABELS.preset_entries,
        reason: '角色扮演专属段,任务模式下不导入',
      });
    } else {
      sections.push({
        key: 'preset_entries',
        label: SECTION_LABELS.preset_entries,
        count: prompts.length,
        summary: `酒馆预设(${prompts.length} 个条目;楼层替换 / 追加可选,导入后切复杂模式)`,
        option: 'preset',
        payload: {
          format: 'st_preset',
          fileName: opts.fileName ?? 'preset.json',
          text: JSON.stringify(data),
        },
      });
    }
    // ST 预设内嵌正则(extensions.regex_scripts;非空才成段)
    const ext = isObject((data as Record<string, unknown>).extensions)
      ? ((data as Record<string, unknown>).extensions as Record<string, unknown>)
      : {};
    const stRegex = Array.isArray(ext.regex_scripts) ? (ext.regex_scripts as Record<string, unknown>[]) : [];
    if (stRegex.length > 0) {
      if (mode === 'task') {
        skipped.push({
          key: 'regex_scripts',
          label: SECTION_LABELS.regex_scripts,
          reason: '角色扮演专属段,任务模式下不导入',
        });
      } else {
        sections.push({
          key: 'regex_scripts',
          label: SECTION_LABELS.regex_scripts,
          count: stRegex.length,
          summary: `预设内嵌正则 ${stRegex.length} 条(进全局正则库;替换 / 追加可选)`,
          option: 'regex',
          payload: stRegex,
        });
      }
    }
    return { source: '酒馆预设(原文件)', sections, skipped };
  }

  // 4. kedai 注入配置
  if (typeof obj.mode === 'string' && obj.simple !== undefined) {
    const cfg = migrateBannedWordsPrompt(toInjectConfig(obj));
    if (mode === 'task') {
      return {
        source: 'kedai 注入配置',
        sections: [],
        skipped: [
          {
            key: 'preset_entries',
            label: SECTION_LABELS.preset_entries,
            reason: '角色扮演专属段,任务模式下不导入',
          },
        ],
      };
    }
    return {
      source: 'kedai 注入配置',
      sections: [
        {
          key: 'preset_entries',
          label: SECTION_LABELS.preset_entries,
          count: cfg.floors?.length ?? 0,
          summary: `注入配置(${cfg.mode === 'simple' ? '简单' : '复杂'}模式,${cfg.floors?.length ?? 0} 条楼层)`,
          option: 'preset',
          payload: { format: 'kedai_inject', config: cfg },
        },
      ],
      skipped: [],
    };
  }

  // 5. 流程四形态(搬运包 / 库格式 / {config} 包装 / 单流程)
  if (
    Array.isArray(obj.flows) ||
    isObject(obj.config) ||
    Array.isArray(obj.steps)
  ) {
    const { flows, rootId } = normalizeFlowFile(data);
    return {
      source: '流程文件',
      sections: [
        {
          key: 'flows',
          label: SECTION_LABELS.flows,
          count: flows.length,
          summary: `${flows.length} 个流程(默认新增副本,可选覆盖同 id)`,
          option: 'flows',
          payload: { flows, rootId },
        },
      ],
      skipped: [],
    };
  }

  // 6. skills 三形态之一(数组已在顶层处理):{skills:[…]} 包壳 / 单对象(单对象在 7 之后判定,与插件消歧)
  if (Array.isArray(obj.skills)) {
    const parsed = parseBundleSection('skills', obj.skills);
    if (parsed && !('error' in parsed)) {
      return { source: '技能包', sections: [parsed.entry], skipped: parsed.skipped };
    }
  }

  // 7. 插件 JSON(name + script 均非空;file 取原文件名)
  {
    const name = typeof obj.name === 'string' ? obj.name.trim() : '';
    const script = typeof obj.script === 'string' ? obj.script : '';
    if (name && script.trim()) {
      return {
        source: '插件文件',
        sections: [
          {
            key: 'plugins',
            label: SECTION_LABELS.plugins,
            count: 1,
            summary: `插件「${name}」(文件名取自所选文件;同名文件覆盖)`,
            option: null,
            payload: [
              {
                file: opts.fileName ?? 'plugin.json',
                folder: typeof obj.folder === 'string' ? obj.folder : '',
                name,
                description: typeof obj.description === 'string' ? obj.description : '',
                parameters: obj.parameters,
                script,
              },
            ],
          },
        ],
        skipped: [],
      };
    }
    // 单对象技能(有 name,无 script)
    if (name) {
      const parsed = parseBundleSection('skills', [data]);
      if (parsed && !('error' in parsed)) {
        return { source: '技能文件', sections: [parsed.entry], skipped: parsed.skipped };
      }
    }
  }

  throw new Error(
    '无法识别的 JSON 格式(支持:综合导出件 / 酒馆预设 / kedai 注入配置 / 流程 / 技能 / 插件)',
  );
}

/** 帮助文案:文件选择区下方的支持范围说明(组件与测试共用一处文案) */
export const IMPORT_FILE_HINT =
  '支持:综合导出件(bundle v1)、酒馆预设、kedai 注入配置、流程 JSON、技能 JSON、插件 JSON、.md/.txt 文本提示词。';
