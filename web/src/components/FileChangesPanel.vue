<script setup lang="ts">
// 任务「文件变更」卡片(批次 4c,PRODCAP-4「交付可审计」):TaskBoard 内嵌区块,
// 数据自取自 store(与 CallTracePanel 同款:无 props,挂载点只负责位置)。
//
// 四条展示纪律(契约见 docs/契约.md「任务文件变更台账」小节):
//  ① 「没有变更」(空清单)/「基线不可用」(行级 truncated)/「扫描缺项」(顶层 undected)/
//     「未确认」(缺项且清单为空)是**四种不同**的答案,不许用空白或同一句话代替;
//  ② `diff` 字段可能是「改动过大,不生成逐行 diff…」的说明文本 → 原样展示,
//     不做 unified 行解析与语法着色(把说明文本当 diff 解析是这里最容易犯的错);
//  ③ 响应不含绝对路径(list 只发相对路径),本组件也不拼 `task.workspace`
//     (那是含 `\\?\` 前缀的本机路径);
//  ④ 基线不可用的行不给可点的死按钮:回滚按钮 `disabled` + 原因写在行内与 title。
import { onMounted, ref, watch } from 'vue';
import { useAppStore } from '../store';
import { storeToRefs } from 'pinia';
import { getTaskChangeDiff, rollbackTaskChange, type TaskFileChange } from '../api';

const store = useAppStore();
const { appMode, currentTaskId, taskFileChanges, taskChangesUndected } = storeToRefs(store);

/** 每行的 diff 状态(按 path 缓存;失败也缓存,避免反复点反复请求) */
interface DiffState {
  loading: boolean;
  /** 展示正文:有基线是 unified diff;新建是「全量新增行 + note」;「改动过大…」说明文本也在本字段 */
  text: string;
  note: string | null;
  reason: string | null;
}
const diffs = ref(new Map<string, DiffState>());
/** 模板取用:未请求过的行返回空态(避免非空断言——前端 lint ratchet 只降不升) */
const EMPTY_DIFF: DiffState = { loading: false, text: '', note: null, reason: null };
function diffOf(path: string): DiffState {
  return diffs.value.get(path) ?? EMPTY_DIFF;
}
/** 正在回滚的 path(防重复点击;也为按钮禁用提供依据) */
const busy = ref<string | null>(null);
/** 回滚结果反馈(行内展示) */
const feedback = ref<{ path: string; ok: boolean; text: string } | null>(null);

const OP_LABEL: Record<TaskFileChange['op'], string> = {
  create: '新建',
  modify: '修改',
  delete: '删除',
  rollback: '回滚',
};
const SOURCE_LABEL: Record<TaskFileChange['source'], string> = {
  tool: '工具精确记账',
  bash: '命令检出',
  rollback: '回滚',
};

/** 基线不可用时的原因(逐行;与 diff 端点的 `reason` 同一事,措辞对齐) */
function baselineReason(row: TaskFileChange): string | null {
  if (!row.truncated && row.has_baseline) return null;
  if (row.truncated) {
    return '基线不可用(改动前正文超出留存上限,或驻留预算打满)';
  }
  return '基线不可用(该行没有改动前正文)';
}

function canDiff(row: TaskFileChange): boolean {
  return baselineReason(row) === null;
}

/** tab 激活(v-show 常驻)/挂载/切换任务且清单为空时,主动拉取一次(样式照 CallTracePanel) */
function ensureLoaded(): void {
  if (appMode.value === 'task' && currentTaskId.value && taskFileChanges.value.length === 0) {
    void store.loadTaskChanges(currentTaskId.value);
  }
}
onMounted(ensureLoaded);
watch(currentTaskId, ensureLoaded);
// 启动时 appMode 已是 task 而 currentTaskId 由 restoreSelectedTask 稍后恢复:
// 与 CallTracePanel 同款补拉,避免 ensureLoaded 早于 currentTaskId 就绪
watch(appMode, ensureLoaded);

async function toggleDiff(row: TaskFileChange): Promise<void> {
  const existing = diffs.value.get(row.path);
  if (existing) {
    const next = new Map(diffs.value);
    next.delete(row.path);
    diffs.value = next;
    return;
  }
  if (!canDiff(row)) return;
  const next = new Map(diffs.value);
  next.set(row.path, { loading: true, text: '', note: null, reason: null });
  diffs.value = next;
  const taskId = currentTaskId.value;
  if (!taskId) return;
  try {
    const d = await getTaskChangeDiff(taskId, row.path);
    const updated = new Map(diffs.value);
    updated.set(row.path, {
      loading: false,
      text: d.available ? (d.diff ?? '') : '',
      note: d.note ?? null,
      // `available:false` 时说清为什么;绝不显示空 diff 冒充「没改动」
      reason: d.available ? null : (d.reason ?? '基线不可用'),
    });
    diffs.value = updated;
  } catch (e) {
    const updated = new Map(diffs.value);
    updated.set(row.path, {
      loading: false,
      text: '',
      note: null,
      reason: e instanceof Error ? e.message : 'diff 加载失败',
    });
    diffs.value = updated;
  }
}

async function doRollback(row: TaskFileChange): Promise<void> {
  const taskId = currentTaskId.value;
  if (!taskId || busy.value || !canDiff(row)) return;
  const ok = window.confirm(`把 ${row.path} 恢复到本任务内它上一次改动前的状态?`);
  if (!ok) return;
  busy.value = row.path;
  feedback.value = null;
  try {
    const r = await rollbackTaskChange(taskId, row.path);
    if (r.ok) {
      feedback.value = {
        path: row.path,
        ok: true,
        text: r.removed ? '回滚完成:该文件由本任务新建,已删除' : `回滚完成(恢复 ${r.restored_bytes ?? 0} 字节)`,
      };
      // 回滚自身会多记一条 op=rollback,必须重拉清单(不隐身)
      await store.loadTaskChanges(taskId);
    } else {
      feedback.value = { path: row.path, ok: false, text: `回滚未执行:${r.reason ?? '基线不可用'}` };
    }
  } catch (e) {
    // 409(任务进行中)与其它失败的原文都在这里;ApiError 已带服务端文案
    feedback.value = { path: row.path, ok: false, text: `回滚失败:${e instanceof Error ? e.message : '未知错误'}` };
  } finally {
    busy.value = null;
  }
}
</script>

<template>
  <div class="sv-task-file-changes">
    <!-- 扫描缺项横幅:文案用后端给的原因原文(前端不许用固定文案冒充) -->
    <div v-if="taskChangesUndected" class="sv-fc-undected">
      本轮命令产生的改动未能完整检出:{{ taskChangesUndected }}
    </div>

    <!-- 空清单:缺项时不许宣称「没有改动」(那正是本卡片要区分掉的歧义) -->
    <p v-if="!taskFileChanges.length" class="sv-note-mini">
      <template v-if="taskChangesUndected">清单为空不代表没有改动——本轮扫描不完整,请以上方说明为准。</template>
      <template v-else>本轮未改动文件</template>
    </p>

    <ul v-else class="sv-fc-list">
      <li v-for="row in taskFileChanges" :key="row.id" class="sv-fc-row">
        <div class="sv-fc-line">
          <span class="sv-fc-path" :title="row.path">{{ row.path }}</span>
          <span class="sv-fc-tag" :class="`op-${row.op}`">{{ OP_LABEL[row.op] }}</span>
          <span class="sv-fc-src">{{ SOURCE_LABEL[row.source] }}</span>
          <span class="sv-fc-bytes sv-tnum">
            <template v-if="row.op === 'delete'">0</template>
            <template v-else>{{ row.after_bytes }}</template> 字节
          </span>
          <button
            v-if="canDiff(row)"
            class="sv-fc-btn"
            :disabled="busy === row.path"
            @click="toggleDiff(row)"
          >{{ diffs.get(row.path) ? '收起 diff' : '查看 diff' }}</button>
          <button
            class="sv-fc-btn"
            :disabled="!canDiff(row) || busy === row.path"
            :title="baselineReason(row) ?? `回滚 ${row.path} 到本任务内上一次改动前`"
            @click="doRollback(row)"
          >{{ busy === row.path ? '回滚中…' : '回滚' }}</button>
        </div>
        <div v-if="!canDiff(row)" class="sv-fc-unavailable">{{ baselineReason(row) }}</div>
        <div v-if="feedback && feedback.path === row.path" class="sv-feedback" :class="feedback.ok ? 'ok' : 'err'">
          {{ feedback.text }}
        </div>
        <div v-if="diffs.has(row.path)" class="sv-fc-diff-wrap">
          <div v-if="diffOf(row.path).loading" class="sv-note-mini">diff 加载中…</div>
          <div v-else-if="diffOf(row.path).reason" class="sv-fc-unavailable">
            {{ diffOf(row.path).reason }}
          </div>
          <template v-else>
            <div v-if="diffOf(row.path).note" class="sv-note-mini">{{ diffOf(row.path).note }}</div>
            <!-- 原样展示:超预算时这里是「改动过大…」说明文本,不做 diff 解析/着色 -->
            <pre class="sv-fc-diff">{{ diffOf(row.path).text }}</pre>
          </template>
        </div>
      </li>
    </ul>
  </div>
</template>
