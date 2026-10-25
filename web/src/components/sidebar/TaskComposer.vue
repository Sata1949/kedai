<script setup lang="ts">
// 任务模式「下达目标」+ 执行设置（C-2A，2026-10-25；原 Sidebar 553-657）。
// 表单草稿留在组件内；创建与执行仍走 store action。工作区画像 debounce 探测随组件
// 生命周期清理，避免侧栏重建后留下悬空请求。
import { computed, onBeforeUnmount, ref, watch } from 'vue';
import { storeToRefs } from 'pinia';
import { useAppStore } from '../../store';
import { inTauri, isAndroidTauri } from '../../platform';
import { MODE_LABELS } from '../../api/labels';
import { probeWorkspace } from '../../api/workspace';
import { projectHintTitle, projectKindLabel } from '../../utils/workspaceProfile';
import type { WorkspaceProfile } from '../../api';
import TaskModeSelect from '../TaskModeSelect.vue';
import TaskFlowSelect from '../TaskFlowSelect.vue';
import TaskConnectionSelect from '../TaskConnectionSelect.vue';

const store = useAppStore();
const { appMode, executors } = storeToRefs(store);

const taskTitle = ref('');
/** 所选执行者库 id(空 = 通用执行者)。执行者与角色扮演角色卡解耦,不再列 characters。 */
const executorId = ref('');
const creatingTask = ref(false);

/** 任务工作区(CODE-1;空 = 未绑定,任务在草稿目录工作)。绑定即冻结:创建期后端
 *  canonical 化并校验(不存在 / 指向数据目录 → 400 中文原因原样上屏)。 */
const workspacePath = ref('');
/** 原生目录选择仅桌面 Tauri 提供:Android 的 SAF 返回 content:// URI,与后端路径语义不符;
 *  浏览器形态退化为手填(输入框始终可编辑,这只是一个便利按钮)。 */
const canPickDir = inTauri && !isAndroidTauri;
/** 编码能力包开启且未选工作区 → 显式提示(把「退化为草稿目录」说在前面,不静默) */
const wsHintVisible = computed(
  () => store.taskCodingBundleEnabled && !workspacePath.value.trim(),
);

/** 「执行设置」折叠行摘要:当前模式 + 执行者,让高级项收起时仍可见关键决策。 */
const taskSettingsSummary = computed(() => {
  const mode = MODE_LABELS[store.taskRunMode] ?? store.taskRunMode;
  const executor = executorId.value
    ? executors.value.find((e) => e.id === executorId.value)?.name ?? '自定义执行者'
    : '通用执行者';
  return `${mode} · ${executor}`;
});

/** 最近工作区(CODE-1;localStorage,去重 + 上限 8 + 最多展示 3 条)。
 *  失效路径不静默删——点了由后端 400 说明原因,与「手填」同一条错误路径。 */
const WS_RECENT_KEY = 'kedai.taskWorkspace.recent.v1';
const WS_RECENT_MAX = 8;
const WS_RECENT_SHOW = 3;
function loadRecentWorkspaces(): string[] {
  try {
    const raw = localStorage.getItem(WS_RECENT_KEY);
    if (!raw) return [];
    const arr: unknown = JSON.parse(raw);
    if (!Array.isArray(arr)) return [];
    return arr
      .filter((x): x is string => typeof x === 'string' && x.trim() !== '')
      .slice(0, WS_RECENT_MAX);
  } catch {
    // 解析失败(手改 / 旧格式)= 视为无历史;不影响创建流程
    return [];
  }
}
const recentWorkspaces = ref<string[]>(loadRecentWorkspaces());
const recentWorkspacesShown = computed(() => recentWorkspaces.value.slice(0, WS_RECENT_SHOW));

function rememberWorkspace(p: string): void {
  const next = [p, ...recentWorkspaces.value.filter((x) => x !== p)].slice(0, WS_RECENT_MAX);
  recentWorkspaces.value = next;
  try {
    localStorage.setItem(WS_RECENT_KEY, JSON.stringify(next));
  } catch {
    // 隐私模式等写入失败:不影响任务创建
  }
}

/** 原生目录选择(仅桌面 Tauri 渲染该按钮;取消返回 null = 不改动输入) */
async function pickWorkspace(): Promise<void> {
  try {
    const { open } = await import('@tauri-apps/plugin-dialog');
    const picked = await open({ directory: true, multiple: false, title: '选择任务工作区' });
    if (typeof picked === 'string') workspacePath.value = picked;
  } catch (err) {
    alert(`选择目录失败:${(err as Error).message}`);
  }
}

// ==================== 工作区画像(CODE-4) ====================
//
// 选定/手填后 debounce 探测,把「这是什么项目 + 建议怎么验证」显示在表单里——用户在**建任务之前**
// 就知道这个目录是什么项目,而不是等任务跑起来才发现选错了目录。
// 探测失败(路径不存在/非目录/数据目录)**不拦创建**:后端创建期还会再拦一次,此处只是提示;
// 文案原样用后端的中文原因,不在前端另造一套。

/** 探测结果(null = 无结果,含未填/探测失败) */
const wsProfile = ref<WorkspaceProfile | null>(null);
/** 探测失败的后端中文原因(空 = 无错误) */
const wsProbeError = ref('');
/** 请求序号:丢弃过期响应(用户连改路径时,先发的慢响应可能后到) */
let wsProbeSeq = 0;
let wsProbeTimer: ReturnType<typeof setTimeout> | null = null;

function clearWsProbe(): void {
  if (wsProbeTimer) {
    clearTimeout(wsProbeTimer);
    wsProbeTimer = null;
  }
}

async function runWsProbe(p: string): Promise<void> {
  const seq = ++wsProbeSeq;
  try {
    const profile = await probeWorkspace(p);
    if (seq !== wsProbeSeq) return;
    wsProfile.value = profile;
    wsProbeError.value = '';
  } catch (err) {
    if (seq !== wsProbeSeq) return;
    wsProfile.value = null;
    wsProbeError.value = (err as Error).message;
  }
}

/** 输入变化 → 清空旧结果并 debounce 重探(空输入不探测:没有可探的目录) */
watch(workspacePath, () => {
  clearWsProbe();
  wsProfile.value = null;
  wsProbeError.value = '';
  const p = workspacePath.value.trim();
  if (!p) return;
  wsProbeTimer = setTimeout(() => void runWsProbe(p), 300);
});

// 组件卸载时清掉待发定时器(侧栏在模式切换时会重建,不清会留下一个悬空请求)
onBeforeUnmount(clearWsProbe);

/** 新建并立即执行 */
async function createAndRun(): Promise<void> {
  const t = taskTitle.value.trim();
  if (!t || creatingTask.value) return;
  creatingTask.value = true;
  const ws = workspacePath.value.trim();
  try {
    const task = await store.createTask(t, executorId.value || undefined, ws || undefined);
    if (ws) rememberWorkspace(ws);
    taskTitle.value = '';
    await store.runTask(task.id);
  } catch (err) {
    alert(`创建任务失败:${(err as Error).message}`);
  } finally {
    creatingTask.value = false;
  }
}

// 任务模式进入时拉执行者库(下拉数据源);失败仅记错误,不影响创建任务(可选通用执行者)
watch(
  () => appMode.value,
  (m) => {
    if (m === 'task' && executors.value.length === 0) void store.loadExecutors();
  },
  { immediate: true },
);
</script>

<template>
  <!-- 下达目标:首屏主件;执行设置收进 <details>,收起时仍显示当前模式/执行者 -->
  <div class="sv-task-new">
    <div class="sv-task-new-title">下达目标</div>
    <p class="sv-task-new-lede">写清楚要交付什么；执行设置可稍后改。</p>
    <textarea
      v-model="taskTitle"
      class="sv-input sv-task-new-input"
      rows="4"
      placeholder="描述要完成的任务,例如:帮我写一篇 2000 字的科幻短篇并列出大纲…"
      spellcheck="false"
    />
    <button
      class="sv-btn primary sv-task-new-submit"
      :disabled="creatingTask || !taskTitle.trim()"
      @click="createAndRun"
    >
      {{ creatingTask ? '创建中…' : '▶ 创建并执行' }}
    </button>

    <details class="sv-task-advanced">
      <summary class="sv-task-advanced-summary">
        <span class="sv-task-advanced-label">执行设置</span>
        <span class="sv-task-advanced-state">{{ taskSettingsSummary }}</span>
        <span class="sv-task-advanced-caret" aria-hidden="true">▾</span>
      </summary>
      <div class="sv-task-advanced-body">
        <!-- 执行模式(批次 4 六模式;独立子组件,选择持久化在 task store) -->
        <TaskModeSelect />
        <!-- 绑定流程(二维批次 5a;仅自定义流程模式可见,空 = 跟随当前流程) -->
        <TaskFlowSelect />
        <!-- 逐任务选用连接(B 批 B1;与执行模式无关,故六模式都可见,空 = 跟随设置的默认连接) -->
        <TaskConnectionSelect />
        <!-- 执行者(独立执行者库;与角色扮演角色卡解耦,故不列 characters) -->
        <div class="sv-inp-row">
          <select v-model="executorId" class="sv-select" title="选择任务执行者">
            <option value="">执行者:通用执行者</option>
            <option v-for="e in executors" :key="e.id" :value="e.id">
              执行者:{{ e.name }}
            </option>
          </select>
          <button
            class="sv-btn ghost sv-btn-sm"
            title="管理执行者(新建/编辑/删除)"
            @click="store.taskExecutorsOpen = true"
          >管理</button>
        </div>
        <!-- 工作区(CODE-1;可选,空 = 未绑定,任务在草稿目录工作;绑定即冻结) -->
        <div class="sv-inp-row sv-ws-row">
          <input
            v-model="workspacePath"
            class="sv-input"
            placeholder="工作区:项目目录绝对路径"
            title="工作区:项目目录绝对路径(可选);留空 = 未绑定,任务在草稿目录内工作"
            spellcheck="false"
            :disabled="creatingTask"
          />
          <button
            v-if="canPickDir"
            class="sv-btn ghost sv-btn-sm"
            title="从本机选择项目目录"
            :disabled="creatingTask"
            @click="pickWorkspace"
          >选择…</button>
          <button
            v-if="workspacePath"
            class="sv-btn ghost sv-btn-sm"
            title="清空工作区(将创建为未绑定任务)"
            :disabled="creatingTask"
            @click="workspacePath = ''"
          >清空</button>
        </div>
        <div v-if="recentWorkspacesShown.length > 0 && !workspacePath" class="sv-ws-recent">
          <span class="sv-note">最近:</span>
          <button
            v-for="p in recentWorkspacesShown"
            :key="p"
            class="sv-ws-recent-item"
            :title="p"
            @click="workspacePath = p"
          >{{ p }}</button>
        </div>
        <!-- 工作区画像(CODE-4):探测到项目类型才渲染;未命中不占位(不把「没有」渲染成噪音) -->
        <div
          v-if="wsProfile && wsProfile.detected.length > 0"
          class="sv-ws-profile"
        >
          <span
            v-for="h in wsProfile.detected"
            :key="`${h.kind}:${h.marker}`"
            class="sv-tag sv-tag-muted"
            :title="projectHintTitle(h.kind, h.marker, h.suggested_command)"
          >{{ projectKindLabel(h.kind) }} · {{ h.suggested_command }}</span>
          <span
            v-if="wsProfile.truncated"
            class="sv-note"
            title="子目录过多,仅扫描了前若干个子目录——结果可能不全"
          >(未扫全)</span>
        </div>
        <!-- 探测失败:把后端中文原因原样上屏(与创建时的 400 同一套文案,前端不另造) -->
        <p v-else-if="wsProbeError" class="sv-note sv-ws-hint">{{ wsProbeError }}</p>
        <p v-if="wsHintVisible" class="sv-note sv-ws-hint">
          编码能力包已启用:未绑定工作区的任务将在任务草稿目录内工作。
        </p>
      </div>
    </details>
  </div>
</template>

<style scoped>
/* 工作区行(CODE-1;UIP-2):输入框占满剩余宽度;窄栏下整行可折行,
   输入框保 170px 下限——让 placeholder 完整可读优先于一行塞下全部控件 */
.sv-ws-row {
  flex-wrap: wrap;
  row-gap: var(--space-1-5);
}
.sv-ws-row .sv-input {
  flex: 1 1 170px;
  min-width: 170px;
}
.sv-ws-recent {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: var(--space-1-5);
  margin-top: var(--space-1-5);
}
.sv-ws-recent-item {
  max-width: 100%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  padding: 1px var(--space-1-5);
  font-size: 11px;
  background: transparent;
  border: var(--bw-thin) solid var(--sv-line-strong);
  color: var(--sv-ink-soft);
  cursor: pointer;
}
.sv-ws-recent-item:hover {
  border-color: var(--sv-ink);
  color: var(--sv-ink);
}
.sv-ws-hint {
  margin-top: var(--space-1-5);
}
/* 工作区画像(CODE-4):chips 与「(未扫全)」留痕同排;宽度不够时换行 */
.sv-ws-profile {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: var(--space-1-5);
  margin-top: var(--space-1-5);
}

/* ===== UIQ-4:任务下达区(首屏主件) =====
   本节是 .sv-task-new 的**唯一 CSS 所有者**：原先全局 task.css 里还有一份基础块，
   与 Sidebar scoped 覆盖值不同且实际生效的是后者；C-2A 把基础属性一并收进本组件，
   全局块已删除（只保留 task.css 里跨组件的 .sv-task-new 后代布局规则）。 */
.sv-task-new {
  padding: var(--space-4) 0 var(--space-3);
  border-bottom: var(--bw) solid var(--sv-ink);
}
.sv-task-new-title {
  display: flex;
  align-items: center;
  gap: var(--space-1-5);
  font-size: var(--text-2xs);
  font-weight: 800;
  letter-spacing: 0.16em;
  margin-bottom: var(--space-2);
  color: var(--sv-ink-soft);
}
.sv-task-new-title::before {
  content: '';
  width: 8px;
  height: 8px;
  background: var(--sv-ink-supreme);
  display: inline-block;
}
.sv-task-new-lede {
  margin: 0 0 var(--space-2-5);
  font-size: var(--text-xs);
  line-height: var(--lh-body);
  color: var(--sv-ink-faint);
}
.sv-task-new-input {
  width: 100%;
  min-height: 96px;
  resize: vertical;
  border: var(--bw) solid var(--sv-ink);
  background: var(--sv-white);
  padding: var(--space-3);
  font-family: var(--font-body);
  font-size: var(--text-sm);
  line-height: var(--lh-body);
  outline: none;
}
.sv-task-new-input:focus {
  border-color: var(--sv-ink);
  border-top: 3px solid var(--sv-red);
}
.sv-task-new-submit {
  width: 100%;
  min-height: 44px;
  margin-top: var(--space-2-5);
}
.sv-task-advanced {
  margin-top: var(--space-3);
  border-top: 1px solid var(--sv-line);
}
.sv-task-advanced-summary {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  min-height: var(--control-h-sm);
  padding: var(--space-2) 0;
  cursor: pointer;
  list-style: none;
  font-size: var(--text-xs);
  color: var(--sv-ink);
}
.sv-task-advanced-summary::-webkit-details-marker { display: none; }
.sv-task-advanced-label { flex: none; font-weight: 800; letter-spacing: 0.08em; }
.sv-task-advanced-state {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--sv-ink-faint);
}
.sv-task-advanced-caret {
  flex: none;
  color: var(--sv-ink-faint);
  transition: transform var(--dur-fast) var(--ease-standard);
}
.sv-task-advanced[open] .sv-task-advanced-caret { transform: rotate(180deg); }
.sv-task-advanced-body {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
  padding: var(--space-1) 0 var(--space-2-5);
}
.sv-ws-recent-item { font-size: var(--text-xs); }
</style>
