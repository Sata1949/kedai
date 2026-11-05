<script setup lang="ts">
// 数据盘符选择器(首启教程第二步 / 老用户强制弹窗 / 设置页共用)。
// 只负责展示与选择,不调用 API:写指针与迁移由调用方按场景处理(教程/弹窗/设置)。
import { computed, ref, watch } from 'vue';
import type { DataLocationStatus } from '../api';

const props = withDefaults(defineProps<{
  status: DataLocationStatus | null;
  busy?: boolean;
  disabled?: boolean;
}>(), {
  busy: false,
  disabled: false,
});

const emit = defineEmits<{ choose: [letter: string] }>();

/** 当前选中的盘符;默认取后端给的 default_drive(D 优先,无 D 回退 C) */
const selected = ref('');

watch(
  () => props.status,
  (status) => {
    if (!status) return;
    if (selected.value && status.drives.some((d) => d.letter === selected.value)) return;
    selected.value = status.default_drive ?? status.drives[0]?.letter ?? '';
  },
  { immediate: true },
);

const canApply = computed(() => !!selected.value && !props.busy && !props.disabled);
</script>

<template>
  <div v-if="status" class="sv-stack">
    <div class="sv-mode">
      <button
        v-for="drive in status.drives"
        :key="drive.letter"
        :class="{ active: selected === drive.letter }"
        :disabled="busy"
        @click="selected = drive.letter"
      >
        {{ drive.letter }}:
      </button>
    </div>
    <p v-if="status.drives.length === 0" class="sv-feedback err">
      未检测到可用盘符，无法选择数据位置。
    </p>
    <p class="sv-note">
      将创建 <code>{{ selected || '?' }}:\Kedai-Data</code>。默认优先 D 盘，没有 D 盘时自动回退 C 盘。
    </p>
    <button class="sv-btn primary" :disabled="!canApply" @click="emit('choose', selected)">
      {{ busy ? '处理中…' : '使用该位置' }}
    </button>
  </div>
  <p v-else class="sv-note">正在读取盘符…</p>
</template>
