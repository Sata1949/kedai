<script setup lang="ts">
// 工具入参/出参折叠卡（C-2A，2026-10-25；替代 AgentPanel 里 6 段逐字重复的 7 行结构）。
//
// 职责只到「显示 + 折叠 + 复制请求」：展开态与复制回执仍由父级持有
// （open/copied 两个 prop），以保持原有「同一调用 key 的入参与出参共享已复制文案」行为。
// 滚动渐隐(has-scroll)在本组件内按真实元素测量，父级不再登记 DOM ref / onUpdated 全局重估。
import { computed, nextTick, onMounted, onUpdated, ref, watch } from 'vue';

const props = defineProps<{
  /** 「入参」/「出参」 */
  label: string;
  /** 原始值（组件内统一 JSON 序列化,展示与复制共用同一份文本） */
  value: unknown;
  open: boolean;
  copied: boolean;
}>();

const emit = defineEmits<{
  toggle: [];
  /** 复制请求：payload 为已序列化文本,剪贴板写入与回执仍归父级 */
  copy: [text: string];
}>();

const text = computed(() => JSON.stringify(props.value, null, 2));

const root = ref<HTMLElement | null>(null);
const body = ref<HTMLElement | null>(null);

/** 内容溢出 → 底部渐隐遮罩 */
function updateShadow(): void {
  const el = body.value;
  root.value?.classList.toggle('has-scroll', !!el && el.scrollHeight > el.clientHeight);
}

/** rAF 合帧:展开/收起与流式内容变化后都重估,避免逐次 layout 抖动 */
let shadowScheduled = false;
function scheduleShadowRefresh(): void {
  if (shadowScheduled) return;
  shadowScheduled = true;
  requestAnimationFrame(() => {
    shadowScheduled = false;
    updateShadow();
  });
}

onMounted(scheduleShadowRefresh);
onUpdated(scheduleShadowRefresh);
watch(() => props.open, () => void nextTick(scheduleShadowRefresh));

/** 滚到底时移除渐隐遮罩(提示后面还有内容才显示) */
function onScroll(e: Event): void {
  const el = e.target as HTMLElement;
  root.value?.classList.toggle('has-scroll', el.scrollTop + el.clientHeight < el.scrollHeight - 4);
}
</script>

<template>
  <div ref="root" class="sv-tool-io" :class="{ open }">
    <div class="sv-tool-io-bar">
      <button type="button" class="sv-tool-io-toggle" @click="emit('toggle')">{{ label }}</button>
      <button type="button" class="sv-tool-io-copy" @click="emit('copy', text)">{{ copied ? '已复制' : '复制' }}</button>
    </div>
    <pre ref="body" class="sv-tool-io-body" @scroll="onScroll">{{ text }}</pre>
  </div>
</template>
