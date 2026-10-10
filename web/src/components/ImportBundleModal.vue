<script setup lang="ts">
// 综合导入 / 综合导出弹窗(IMPHUB 步骤 C,2026-10-09)。
//
// 导入:选文件 → 嗅探/解析(utils/importBundle 纯函数)→ 预览(每段勾选 + 摘要 +
// 段级策略 + 模式过滤说明 + 替换类确认)→ 逐段执行 → 统一回执(§2.1.4)。
// 导出:选段(默认全选当前模式可用段)→ GET /api/export/bundle → 下载 bundle v1。
//
// 逐段独立执行、失败隔离:一段 failed 不阻断其余段(结果逐条列出,不合并成一句)。
// 「预先知道要重渲染」的状态刷新(流程库/技能/注入配置/全局正则)在各段成功后立即做。
import { computed, ref } from 'vue';
import { storeToRefs } from 'pinia';
import { useAppStore } from '../store';
import * as api from '../api';
import { downloadBlob } from '../exportFile';
import type { ImportSection, ParsedBundle } from '../utils/importBundle';
import type { ImportSectionResult } from '../api';
import {
  IMPORT_FILE_HINT,
  ROLEPLAY_ONLY_SECTIONS,
  SECTION_KEYS,
  SECTION_LABELS,
  appendFloors,
  importReportMessage,
  mergeRegexEntries,
  parseImportBundle,
} from '../utils/importBundle';

const store = useAppStore();
const { importBundleOpen, appMode } = storeToRefs(store);

const close = (): void => {
  store.importBundleOpen = false;
};

// ===== 页签 =====
const tab = ref<'import' | 'export'>('import');

// ===== 导入页签状态 =====
const fileInput = ref<HTMLInputElement | null>(null);
const pickedName = ref('');
const parsed = ref<ParsedBundle | null>(null);
const parseError = ref('');
/** 每段勾选(缺省全选;键为段键) */
const selected = ref<Record<string, boolean>>({});
/** ① 段策略:替换(默认,带确认)/ 追加 */
const presetStrategy = ref<'replace' | 'append'>('replace');
/** ② 段冲突:新增副本(默认)/ 覆盖同 id(不可逆,执行前二次确认) */
const flowConflict = ref<'rename' | 'replace'>('rename');
/** ⑥ 段策略:替换(默认,带确认)/ 追加(按 name+find_regex 去重) */
const regexStrategy = ref<'replace' | 'append'>('replace');
const running = ref(false);
const results = ref<ImportSectionResult[]>([]);
const msg = ref<{ kind: 'ok' | 'err'; text: string } | null>(null);

/** 已勾选且当前模式可用的段(段序固定) */
const chosenSections = computed(() => (parsed.value?.sections ?? []).filter((s) => selected.value[s.key]));

async function onFilePicked(e: Event): Promise<void> {
  const input = e.target as HTMLInputElement;
  const file = input.files?.[0];
  input.value = '';
  if (!file) return;
  parsed.value = null;
  parseError.value = '';
  results.value = [];
  msg.value = null;
  try {
    const text = await file.text();
    // .md/.txt 原样按文本导入(歧义显式化:作为运行时提示词,不做猜测);其余必须能 JSON 解析
    const isText = /\.(md|txt)$/i.test(file.name);
    let data: unknown = text;
    if (!isText) {
      try {
        data = JSON.parse(text);
      } catch {
        throw new Error('文件不是有效的 JSON');
      }
    }
    const out = parseImportBundle(data, appMode.value, { fileName: file.name });
    parsed.value = out;
    pickedName.value = file.name;
    selected.value = Object.fromEntries(out.sections.map((s) => [s.key, true]));
    if (out.sections.length === 0) {
      msg.value = { kind: 'err', text: '该文件在当前模式下没有可导入的段(见下方说明)' };
    }
  } catch (err) {
    parseError.value = (err as Error).message;
  }
}

/** 替换类动作的一次性确认文案(说清哪些段会被替换;取消 = 什么都不做) */
function replaceConfirmText(sections: ImportSection[]): string | null {
  const parts: string[] = [];
  for (const s of sections) {
    if (s.key === 'preset_entries' && presetStrategy.value === 'replace') parts.push('①预设条目(替换全部楼层)');
    if (s.key === 'flows' && flowConflict.value === 'replace') parts.push('②执行流程(覆盖同 id 流程)');
    if (s.key === 'regex_scripts' && regexStrategy.value === 'replace') parts.push('⑥预设正则(替换全局正则库)');
  }
  if (parts.length === 0) return null;
  return `本次导入包含替换操作:${parts.join('、')}。替换不可撤销,确定继续?`;
}

/** 单段执行(失败隔离:异常在此收成 failed 回执,不向外抛) */
async function runSection(section: ImportSection): Promise<ImportSectionResult> {
  const base = { section: section.key as string, errors: [] as string[] };
  try {
    switch (section.key) {
      case 'preset_entries': {
        const p = section.payload;
        if (p.format === 'st_preset') {
          const file = new File([p.text], p.fileName || 'preset.json', { type: 'application/json' });
          const res = await api.importPromptPreset(file, presetStrategy.value);
          await store.loadPromptInject();
          return {
            ...base,
            status: 'ok',
            summary: `已导入 ${res.imported} 条楼层(${presetStrategy.value === 'append' ? '追加' : '替换'})`,
          };
        }
        if (presetStrategy.value === 'append') {
          const current = await api.getPromptInject();
          const merged = appendFloors(current, p.config);
          const saved = await api.savePromptInject(merged);
          await store.loadPromptInject();
          return { ...base, status: 'ok', summary: `已追加楼层(库内共 ${saved.floors.length} 条)` };
        }
        const saved = await api.savePromptInject(p.config);
        await store.loadPromptInject();
        return {
          ...base,
          status: 'ok',
          summary: `已替换注入配置(${saved.floors.length} 条楼层,${saved.mode === 'simple' ? '简单' : '复杂'}模式)`,
        };
      }
      case 'flows': {
        const onConflict = flowConflict.value === 'replace' ? ('replace' as const) : undefined;
        const report = await store.importAgentFlows(
          section.payload.flows,
          section.payload.rootId,
          onConflict,
        );
        return { ...base, status: 'ok', summary: importReportMessage(report) };
      }
      case 'prompts': {
        const p = section.payload;
        const mode = appMode.value === 'task' ? '任务' : '角色扮演';
        const variant = appMode.value === 'task' ? p.system.task : p.system.roleplay;
        const wrote: string[] = [];
        // 空串语义 = 未自定义(不覆盖本机;契约与后端导出件注释同源)
        if (variant) {
          await store.saveSettings({ agent_system_prompt: variant });
          wrote.push(`系统提示词(${mode})`);
        }
        if (p.runtime) {
          await api.saveAgentPromptMd(p.runtime);
          wrote.push('运行时主提示词');
        }
        if (wrote.length === 0) {
          return { ...base, status: 'skipped', summary: '段内为空(空串 = 未自定义),未覆盖本机' };
        }
        return { ...base, status: 'ok', summary: `已写入 ${wrote.join(' + ')}` };
      }
      case 'skills': {
        const res = await api.importSkills(section.payload);
        await store.loadSkills();
        return { ...base, status: 'ok', summary: `已导入 ${res.imported} 个技能(同名覆盖)` };
      }
      case 'regex_scripts': {
        if (regexStrategy.value === 'append') {
          const current = await api.getRegexScripts();
          const { merged, added } = mergeRegexEntries(current, section.payload);
          const res = await store.saveRegexScripts(merged);
          return {
            ...base,
            status: 'ok',
            summary: `已追加 ${added} 条正则(库内共 ${res.count} 条${res.skipped ? `,跳过 ${res.skipped} 条无效` : ''})`,
          };
        }
        const res = await store.saveRegexScripts(section.payload);
        return {
          ...base,
          status: 'ok',
          summary: `已替换全局正则库(${res.count} 条${res.skipped ? `,跳过 ${res.skipped} 条无效` : ''})`,
        };
      }
      case 'plugins': {
        const res = await api.importPluginTools(section.payload);
        return {
          ...base,
          status: 'ok',
          summary: `已导入 ${res.imported} 个插件(已注册 ${res.loaded} 个)`,
        };
      }
    }
  } catch (e) {
    const text = (e as Error).message || String(e);
    return { ...base, status: 'failed', summary: `失败:${text}`, errors: [text] };
  }
}

async function runImport(): Promise<void> {
  const bundle = parsed.value;
  if (!bundle || running.value) return;
  const chosen = chosenSections.value;
  if (chosen.length === 0) {
    msg.value = { kind: 'err', text: '未勾选任何段' };
    return;
  }
  const confirmText = replaceConfirmText(chosen);
  if (confirmText && !window.confirm(confirmText)) return;
  running.value = true;
  msg.value = null;
  results.value = [];
  // 模式不适用 / 未知段先行登记 skipped(不发送),再逐段执行
  const skipped: ImportSectionResult[] = bundle.skipped.map((s) => ({
    section: s.key,
    status: 'skipped' as const,
    summary: s.reason,
    errors: [],
  }));
  const executed: ImportSectionResult[] = [];
  for (const section of chosen) {
    executed.push(await runSection(section));
  }
  results.value = [...skipped, ...executed];
  const failed = executed.filter((r) => r.status === 'failed').length;
  const okCount = executed.filter((r) => r.status === 'ok').length;
  msg.value = failed > 0
    ? { kind: 'err', text: `导入完成:${okCount} 段成功,${failed} 段失败(见下方回执)` }
    : { kind: 'ok', text: `导入完成:${okCount} 段成功` };
  running.value = false;
}

// ===== 导出页签状态 =====
/** 导出勾选(默认全选;任务模式下 ①⑥ 不渲染也不导出) */
const exportSelected = ref<Record<string, boolean>>(
  Object.fromEntries(SECTION_KEYS.map((k) => [k, true])),
);
const exporting = ref(false);

/** 导出页签可见段(任务模式隐藏角色扮演专属段) */
const exportableSections = computed(() =>
  SECTION_KEYS.filter((k) => appMode.value !== 'task' || !ROLEPLAY_ONLY_SECTIONS.has(k)),
);

async function runExport(): Promise<void> {
  if (exporting.value) return;
  const keys = exportableSections.value.filter((k) => exportSelected.value[k]);
  if (keys.length === 0) {
    msg.value = { kind: 'err', text: '未勾选任何段' };
    return;
  }
  exporting.value = true;
  msg.value = null;
  try {
    const { bundle, warnings } = await api.exportBundle(appMode.value);
    // 选段在客户端做(后端恒回当前模式全段;段序按 SECTION_KEYS 重建以保契约)
    const sections: Record<string, unknown> = {};
    for (const k of keys) {
      if (bundle.sections[k] !== undefined) sections[k] = bundle.sections[k];
    }
    const out = { ...bundle, sections };
    downloadBlob(
      `kedai-bundle-${new Date().toISOString().slice(0, 10)}.json`,
      new Blob([JSON.stringify(out, null, 2)], { type: 'application/json' }),
    );
    msg.value = warnings.length > 0
      ? { kind: 'ok', text: `已导出(跳过 ${warnings.length} 个解析失败的插件文件)` }
      : { kind: 'ok', text: '已导出' };
  } catch (e) {
    msg.value = { kind: 'err', text: `导出失败:${(e as Error).message}` };
  } finally {
    exporting.value = false;
  }
}
</script>

<template>
  <div class="sv-modal-mask" @click.self="close">
    <div class="sv-modal lg">
      <div class="sv-modal-head">
        <h2 class="flex items-center gap-2">
          <span class="sv-supreme pink" /> 综合导入 / 导出
        </h2>
        <button class="sv-btn ghost sv-btn-square" @click="close">✕</button>
      </div>

      <div class="sv-modal-body">
        <!-- 页签 -->
        <div class="sv-mode" style="margin-bottom: var(--space-3)">
          <button :class="{ active: tab === 'import' }" @click="tab = 'import'">导入</button>
          <button :class="{ active: tab === 'export' }" @click="tab = 'export'">导出</button>
        </div>

        <!-- ===== 导入页签 ===== -->
        <div v-show="tab === 'import'" class="sv-stack">
          <div class="sv-data-row">
            <div class="info">
              <b>选择文件</b>
              <span>一个文件可包含多段(预设条目/执行流程/提示词/技能/预设正则/插件),导入前可逐段预览勾选</span>
            </div>
            <button class="sv-btn primary" @click="fileInput?.click()">选择文件</button>
          </div>
          <input
            ref="fileInput"
            type="file"
            accept=".json,.md,.txt,application/json,text/plain,text/markdown"
            class="hidden"
            @change="onFilePicked"
          />
          <p class="sv-note">{{ IMPORT_FILE_HINT }}</p>
          <div v-if="parseError" class="sv-feedback err">解析失败:{{ parseError }}</div>

          <template v-if="parsed">
            <div class="sv-note">
              来源:{{ parsed.source }}{{ pickedName ? `(文件 ${pickedName})` : '' }}
            </div>

            <!-- 段预览:勾选 + 摘要 + 段级策略 -->
            <div class="impb-section" v-for="s in parsed.sections" :key="s.key">
              <label class="impb-head" :for="`impb-sel-${s.key}`">
                <input :id="`impb-sel-${s.key}`" v-model="selected[s.key]" type="checkbox" />
                <b>{{ s.label }}</b>
                <span class="impb-count">共 {{ s.count }} 项</span>
              </label>
              <p class="sv-note impb-summary">{{ s.summary }}</p>
              <div v-if="s.option === 'preset'" class="sv-inp-row">
                <label class="sv-inp-tag">导入方式</label>
                <select v-model="presetStrategy" class="sv-select">
                  <option value="replace">替换现有全部楼层(默认)</option>
                  <option value="append">追加到现有楼层之后</option>
                </select>
              </div>
              <div v-else-if="s.option === 'flows'" class="sv-inp-row">
                <label class="sv-inp-tag">冲突处理</label>
                <select v-model="flowConflict" class="sv-select">
                  <option value="rename">新增副本(同 id 分配新 id;默认)</option>
                  <option value="replace">覆盖同 id 流程(不可逆)</option>
                </select>
              </div>
              <div v-else-if="s.option === 'regex'" class="sv-inp-row">
                <label class="sv-inp-tag">导入方式</label>
                <select v-model="regexStrategy" class="sv-select">
                  <option value="replace">替换全局正则库(默认)</option>
                  <option value="append">追加(按名称+查找式去重)</option>
                </select>
              </div>
            </div>

            <!-- 模式不适用 / 未知段说明(不发送,回执记 skipped) -->
            <div v-if="parsed.skipped.length" class="impb-skipped">
              <p
                v-for="(s, i) in parsed.skipped"
                :key="`${s.key}-${i}`"
                class="sv-note"
              >
                「{{ s.label }}」不导入:{{ s.reason }}
              </p>
            </div>

            <div class="sv-btn-row">
              <button
                class="sv-btn primary sv-btn-fill"
                :disabled="running || chosenSections.length === 0"
                @click="runImport"
              >
                {{ running ? '导入中...' : `开始导入(${chosenSections.length} 段)` }}
              </button>
            </div>
          </template>

          <div v-if="msg" class="sv-feedback" :class="msg.kind === 'err' ? 'err' : 'ok'">{{ msg.text }}</div>

          <!-- 统一回执(§2.1.4:逐段 status/summary;失败隔离可见) -->
          <div v-if="results.length" class="impb-results">
            <div class="sv-field-label"><span class="sv-supreme yellow" /> 导入回执</div>
            <div v-for="r in results" :key="r.section + r.summary" class="impb-result">
              <span class="impb-status" :class="r.status">{{ r.status === 'ok' ? '成功' : r.status === 'skipped' ? '跳过' : '失败' }}</span>
              <span class="impb-result-name">{{ SECTION_LABELS[r.section as keyof typeof SECTION_LABELS] ?? r.section }}</span>
              <span class="impb-result-text">{{ r.summary }}</span>
            </div>
          </div>
        </div>

        <!-- ===== 导出页签 ===== -->
        <div v-show="tab === 'export'" class="sv-stack">
          <p class="sv-note">
            导出为综合导出件(bundle v1):勾选要包含的段。当前模式:{{ appMode === 'task' ? '任务(①预设条目 / ⑥预设正则 不适用)' : '角色扮演' }}。
          </p>
          <label
            v-for="k in exportableSections"
            :key="k"
            class="impb-head"
            :for="`impb-exp-${k}`"
          >
            <input :id="`impb-exp-${k}`" v-model="exportSelected[k]" type="checkbox" />
            <b>{{ SECTION_LABELS[k] }}</b>
          </label>
          <div class="sv-btn-row">
            <button class="sv-btn primary sv-btn-fill" :disabled="exporting" @click="runExport">
              {{ exporting ? '导出中...' : '导出综合导出件(JSON)' }}
            </button>
          </div>
          <p class="sv-note">
            导出件含 ②执行流程 / ⑤技能 / ⑦插件 / ③④提示词等段;不含 API 连接与密钥,节点上的连接引用跨机后需重选。
          </p>
          <div v-if="msg" class="sv-feedback" :class="msg.kind === 'err' ? 'err' : 'ok'">{{ msg.text }}</div>
        </div>
      </div>

      <div class="sv-modal-foot">
        <button class="sv-btn primary" @click="close">完成</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* 段预览行(MAINTENANCE D-5:新增组件样式不进 style.css,写 scoped) */
.impb-section {
  border: var(--bw-thin) solid var(--sv-line);
  padding: var(--space-2-5) var(--space-3);
  display: flex;
  flex-direction: column;
  gap: var(--space-1-5);
  background: var(--sv-white);
}
.impb-head {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  cursor: pointer;
  font-size: var(--text-sm);
}
.impb-count {
  margin-left: auto;
  font-size: var(--text-2xs);
  color: var(--sv-ink-faint);
}
.impb-summary {
  margin: 0;
}
.impb-skipped {
  border-left: var(--bw-thin) solid var(--sv-line);
  padding-left: var(--space-3);
}
.impb-skipped .sv-note {
  margin: 0;
}
.impb-results {
  display: flex;
  flex-direction: column;
  gap: var(--space-1-5);
}
.impb-result {
  display: flex;
  align-items: baseline;
  gap: var(--space-2);
  font-size: var(--text-sm);
}
.impb-status {
  flex: none;
  font-size: var(--text-2xs);
  padding: 1px var(--space-1-5);
  border: var(--bw-thin) solid var(--sv-line);
}
.impb-status.ok {
  background: var(--sv-ink);
  color: var(--sv-white);
  border-color: var(--sv-ink);
}
.impb-status.failed {
  color: var(--sv-red);
  border-color: var(--sv-red);
}
.impb-result-name {
  flex: none;
  color: var(--sv-ink-dim);
}
.impb-result-text {
  min-width: 0;
}
</style>
