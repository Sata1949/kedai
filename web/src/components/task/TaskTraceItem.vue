<script setup lang="ts">
// 单条执行轨迹折叠卡(TMSHOW-2):思考 / 工具调用 / 工具结果 / 步骤产出。
// 由 TaskTranscript 的档 2(扁平)与档 3(分组内)共用同一张卡。
import { computed, ref } from 'vue';
import { TRACE_KIND_LABELS } from '../../utils/taskTranscript';
import { phaseText } from '../../utils/phaseLabel';
import type { TaskTrace } from '../../api';

const props = withDefaults(
  defineProps<{
    trace: TaskTrace;
    /** 步骤名解析器(给 step 类阶段补可读步骤名;取不到则不加后缀) */
    stepName?: (index: number) => string | null;
    /** 是否默认展开;不传则按内容长度与类型自动判定 */
    defaultOpen?: boolean;
  }>(),
  { stepName: undefined, defaultOpen: undefined },
);

/**
 * 正文折叠阈值(字符)。
 *
 * 取值理由:一轮思考常见 500~2000 字,全展开会让档 2 的单屏信息密度失控;
 * 600 字以内基本是「一眼读完」的量,超过就该由用户决定要不要展开。
 */
const COLLAPSE_THRESHOLD = 600;

/**
 * 是否默认展开。
 *
 * 默认规则:**思考**短文本展开(它是理解过程的主线),工具入参/出参与长正文一律收起
 * ——工具 JSON 动辄上千字且大部分是参数样板,展开它们会把时间线撑成滚动地狱。
 * 调用方可显式覆盖(档 3 分组内默认全收,由组级开关统一控制)。
 */
const initialOpen = computed(() => {
  if (props.defaultOpen !== undefined) return props.defaultOpen;
  return props.trace.kind === 'reasoning' && props.trace.content.length <= COLLAPSE_THRESHOLD;
});

const open = ref(initialOpen.value);

/** 副标题:阶段归属 + 工具名(仅工具类条目有) */
const subtitle = computed(() => {
  const parts: string[] = [];
  const label = phaseText(props.trace.phase, props.trace.step_index, props.stepName);
  if (label) parts.push(label);
  if (props.trace.name) parts.push(props.trace.name);
  return parts.join(' · ');
});

/** 正文是否值得显示折叠开关(极短文本没有折叠的意义) */
const collapsible = computed(() => props.trace.content.length > 120);
</script>

<template>
  <div class="sv-trace-item" :class="[`kind-${trace.kind}`]">
    <button
      v-if="collapsible"
      type="button"
      class="sv-trace-head"
      :aria-expanded="open"
      @click="open = !open"
    >
      <span class="sv-trace-caret" :class="{ open }" aria-hidden="true">▸</span>
      <span class="sv-trace-kind">{{ TRACE_KIND_LABELS[trace.kind] }}</span>
      <span v-if="subtitle" class="sv-trace-sub">{{ subtitle }}</span>
      <span v-if="trace.truncated" class="sv-trace-cut" title="正文超过写入侧上限，已截断">已截断</span>
    </button>
    <div v-else class="sv-trace-head static">
      <span class="sv-trace-kind">{{ TRACE_KIND_LABELS[trace.kind] }}</span>
      <span v-if="subtitle" class="sv-trace-sub">{{ subtitle }}</span>
      <span v-if="trace.truncated" class="sv-trace-cut" title="正文超过写入侧上限，已截断">已截断</span>
    </div>
    <!-- 正文一律纯文本插值:工具入参/出参是 JSON 文本,思考是原始文本。
         刻意不走 v-html(即便走 markdown 也不该——轨迹里混着未转义的模型输出) -->
    <pre v-if="!collapsible || open" class="sv-trace-body">{{ trace.content }}</pre>
  </div>
</template>

<style scoped>
/* 单条轨迹卡:左侧 2px 类型色条 + 紧凑排版。
   颜色语义与既有任务台一致(思考=墨、工具=粉、结果=灰绿、产出=黄)。 */
.sv-trace-item {
  border-left: 2px solid var(--sv-line-strong);
  padding: var(--space-2) 0 var(--space-2) var(--space-3);
  margin-bottom: var(--space-2);
}
.sv-trace-item.kind-reasoning { border-left-color: var(--sv-ink-faint); }
.sv-trace-item.kind-tool_call { border-left-color: var(--sv-pink-dark); }
.sv-trace-item.kind-tool_result { border-left-color: var(--sv-green); }
.sv-trace-item.kind-step_result { border-left-color: var(--sv-yellow); }

.sv-trace-head {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  width: 100%;
  padding: 0;
  border: 0;
  background: none;
  color: inherit;
  font: inherit;
  text-align: left;
  cursor: pointer;
}
.sv-trace-head.static { cursor: default; }
.sv-trace-caret {
  flex: none;
  font-size: var(--text-2xs);
  color: var(--sv-ink-faint);
  transition: transform var(--dur-fast) var(--ease-standard);
}
.sv-trace-caret.open { transform: rotate(90deg); }
.sv-trace-kind {
  flex: none;
  font-size: var(--text-xs);
  font-weight: 800;
  letter-spacing: 0.06em;
}
.sv-trace-sub {
  flex: 1;
  min-width: 0;
  font-size: var(--text-xs);
  color: var(--sv-ink-faint);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.sv-trace-cut {
  flex: none;
  font-size: var(--text-2xs);
  padding: 0 var(--space-1-5);
  border: var(--bw-thin) solid var(--sv-orange);
  color: var(--sv-orange);
}
.sv-trace-body {
  margin: var(--space-1-5) 0 0;
  padding: var(--space-2) var(--space-2-5);
  max-height: 420px;
  overflow: auto;
  background: var(--sv-surface-elevated);
  font-family: var(--font-mono, monospace);
  font-size: var(--text-xs);
  line-height: var(--lh-body);
  white-space: pre-wrap;
  word-break: break-word;
  color: var(--sv-ink-soft);
}
</style>
