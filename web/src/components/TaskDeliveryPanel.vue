<script setup lang="ts">
// 任务「交付校验」卡片（TMR-6）：必需 / 已生成 / 缺失 / 变更数。
//
// 四条展示纪律：
//  ① 未绑定工作区 → 明说「不做交付校验」，不显示 0/0 假装已校验；
//  ② 已绑定但 `delivery` 缺失（旧服务端）→ 明说摘要不可用；
//  ③ `checked=false`（规划器未声明必需文件）→ 明说只做「是否产生变更」弱校验；
//  ④ 工作区不可访问 → 明确警示「缺失判定不可信」，不把空 missing 当齐全。
// 变更明细仍由 FileChangesPanel 承担，本卡只给摘要，不重复列 diff。
import { computed } from 'vue';
import type { TaskDeliverySummary } from '../api';

const props = defineProps<{
  delivery: TaskDeliverySummary | null;
  workspaceBound: boolean;
}>();

// 旧服务端不下发这两个字段（SKY-6 新增）：按「无问题」容错。
const contentIssues = computed(() => props.delivery?.content_issues ?? []);
const truncatedSteps = computed(() => props.delivery?.truncated ?? []);
// SKY-1 交付卫生（2026-10-25）：根目录临时/草稿文件只作提示，不影响终态。
const hygieneIssues = computed(() => props.delivery?.hygiene_issues ?? []);
// SKY-9-L1（2026-10-25）：运行期自检问题由后端持久化后下发，与错误条同源。
const runtimeIssues = computed(() => props.delivery?.runtime_issues ?? []);
</script>

<template>
  <div class="sv-delivery">
    <template v-if="!props.workspaceBound">
      <div class="sv-delivery-hint">本任务未绑定工作区，不做文件交付校验。</div>
    </template>
    <template v-else-if="!props.delivery">
      <div class="sv-delivery-hint">交付摘要不可用（旧服务端未下发，或任务详情尚未加载）。</div>
    </template>
    <template v-else>
      <div class="sv-delivery-row">
        <span class="sv-delivery-chip">必需 {{ props.delivery.required.length }}</span>
        <span class="sv-delivery-chip" :class="{ ok: props.delivery.missing.length === 0 }">
          已生成 {{ props.delivery.required.length - props.delivery.missing.length }}
        </span>
        <span class="sv-delivery-chip" :class="{ bad: props.delivery.missing.length > 0 }">
          缺失 {{ props.delivery.missing.length }}
        </span>
        <span class="sv-delivery-chip">变更 {{ props.delivery.changed_count }}</span>
        <span
          v-if="truncatedSteps.length"
          class="sv-delivery-chip bad"
        >未完成 {{ truncatedSteps.length }}</span>
      </div>
      <div v-if="truncatedSteps.length" class="sv-delivery-warn">
        未完成的子目标：{{ truncatedSteps.join('、') }}（成果可能不完整，可追加指令继续完成）
      </div>
      <div
        v-for="issue in contentIssues"
        :key="issue"
        class="sv-delivery-warn"
      >交付内容校验：{{ issue }}</div>
      <div
        v-for="issue in runtimeIssues"
        :key="`runtime-${issue}`"
        class="sv-delivery-warn"
      >运行期自检：{{ issue }}</div>
      <div
        v-for="issue in hygieneIssues"
        :key="issue"
        class="sv-delivery-hint"
      >交付卫生：{{ issue }}</div>
      <div v-if="!props.delivery.workspace_available" class="sv-delivery-warn">
        工作区当前不可访问，缺失判定不可信。
      </div>
      <div v-if="!props.delivery.checked" class="sv-delivery-hint">
        本任务未声明必需文件；仅按「本次是否产生文件变更」校验。
      </div>
      <ul v-if="props.delivery.required.length" class="sv-delivery-files">
        <li
          v-for="file in props.delivery.required"
          :key="file.path"
          :class="{ bad: !file.exists }"
        >
          <span class="sv-delivery-mark">{{ file.exists ? '✓' : '✗' }}</span>
          <code class="sv-delivery-path">{{ file.path }}</code>
          <span class="sv-delivery-owner">{{ file.owner }}</span>
        </li>
      </ul>
    </template>
  </div>
</template>

<style scoped>
.sv-delivery {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
  font-size: var(--text-sm);
  color: var(--sv-ink-soft);
}
.sv-delivery-row {
  display: flex;
  flex-wrap: wrap;
  gap: var(--space-1-5);
}
.sv-delivery-chip {
  display: inline-flex;
  align-items: center;
  padding: var(--space-1) var(--space-2);
  border: var(--bw-thin) solid var(--sv-line-strong);
  font-size: var(--text-xs);
  color: var(--sv-ink-dim);
  background: var(--sv-surface-elevated);
}
.sv-delivery-chip.ok {
  border-color: var(--sv-green);
  color: var(--sv-green-deep);
}
.sv-delivery-chip.bad {
  border-color: var(--sv-red);
  color: var(--sv-red-deep);
}
.sv-delivery-hint {
  font-size: var(--text-xs);
  color: var(--sv-ink-faint);
}
.sv-delivery-warn {
  padding: var(--space-1-5) var(--space-2);
  border-left: var(--bw) solid var(--sv-red);
  background: var(--sv-pink-light);
  font-size: var(--text-xs);
  color: var(--sv-red-deep);
}
.sv-delivery-files {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
}
.sv-delivery-files li {
  display: flex;
  align-items: baseline;
  gap: var(--space-2);
  min-width: 0;
}
.sv-delivery-mark {
  color: var(--sv-green-deep);
  font-weight: 700;
}
.sv-delivery-files li.bad .sv-delivery-mark {
  color: var(--sv-red-deep);
}
.sv-delivery-path {
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  overflow-wrap: anywhere;
}
.sv-delivery-owner {
  margin-left: auto;
  font-size: var(--text-2xs);
  color: var(--sv-ink-faint);
  white-space: nowrap;
}
</style>
