<script setup lang="ts">
// 数据位置选择器(首启教程第二步 / 老用户强制弹窗 / 设置页共用)。
// 只负责展示与选择,不调用 API:写指针与迁移由调用方按场景处理(教程/弹窗/设置)。
//
// 双平台(2026-11-06 Android 修复):
//   - 桌面:盘符选择(原行为不变);
//   - Android:没有盘符概念,改用绝对路径输入,默认
//     `/storage/emulated/0/Download/Kedai-Data`;公共目录写入需要「所有文件访问」
//     权限,status.needs_permission 为真时提供系统授权入口。
import { computed, ref, watch } from 'vue';
import type { DataLocationStatus } from '../api';
import { requestAllFilesAccess } from '../platform';

const props = withDefaults(defineProps<{
  status: DataLocationStatus | null;
  busy?: boolean;
  disabled?: boolean;
}>(), {
  busy: false,
  disabled: false,
});

const emit = defineEmits<{
  choose: [target: string];
  /** 请求父级重新拉取 status(Android 从系统授权页返回后重新检测) */
  refresh: [];
}>();

/** Android 默认根(与后端 data_location::ANDROID_DEFAULT_ROOT 同值) */
const ANDROID_DEFAULT_ROOT = '/storage/emulated/0/Download/Kedai-Data';

/** 当前选中的盘符;默认取后端给的 default_drive(D 优先,无 D 回退 C) */
const selected = ref('');
/** Android 路径模式当前输入值 */
const androidRoot = ref(ANDROID_DEFAULT_ROOT);
const granting = ref(false);

const isAndroid = computed(() => props.status?.platform === 'android');

watch(
  () => props.status,
  (status) => {
    if (!status) return;
    if (status.platform === 'android') {
      androidRoot.value = status.suggested_root || status.root || ANDROID_DEFAULT_ROOT;
      return;
    }
    if (selected.value && status.drives.some((d) => d.letter === selected.value)) return;
    selected.value = status.default_drive ?? status.drives[0]?.letter ?? '';
  },
  { immediate: true },
);

const canApply = computed(() => {
  if (props.busy || props.disabled) return false;
  if (isAndroid.value) return androidRoot.value.trim().startsWith('/');
  return !!selected.value;
});

function apply(): void {
  if (!canApply.value) return;
  emit('choose', isAndroid.value ? androidRoot.value.trim() : selected.value);
}

/** 打开系统「所有文件访问」授权页;返回应用后由用户点「重新检测」刷新状态 */
async function grantPermission(): Promise<void> {
  if (granting.value) return;
  granting.value = true;
  try {
    await requestAllFilesAccess();
  } finally {
    granting.value = false;
  }
}
</script>

<template>
  <div v-if="status" class="sv-stack">
    <!-- Android:路径模式 -->
    <template v-if="isAndroid">
      <div class="sv-inp-row">
        <span class="sv-inp-tag">数据根</span>
        <input
          v-model="androidRoot"
          class="sv-input"
          spellcheck="false"
          autocomplete="off"
          :disabled="busy || disabled"
          aria-label="数据根路径"
        />
      </div>
      <p class="sv-note">
        默认使用公共「下载」目录下的 <code>Kedai-Data</code>,卸载应用后数据仍保留。
        Android 11+ 直接读写公共目录需要「所有文件访问」权限。
      </p>
      <p v-if="status.needs_permission" class="sv-feedback err">
        尚未获得「所有文件访问」权限，当前路径不可写。请先授权，返回后点「重新检测」。
      </p>
      <div class="sv-btn-row">
        <button
          v-if="status.needs_permission"
          type="button"
          class="sv-btn ghost"
          :disabled="busy || granting"
          @click="grantPermission"
        >
          {{ granting ? '正在打开…' : '去授权所有文件访问' }}
        </button>
        <button type="button" class="sv-btn ghost" :disabled="busy" @click="emit('refresh')">
          重新检测
        </button>
        <button type="button" class="sv-btn primary" :disabled="!canApply" @click="apply">
          {{ busy ? '处理中…' : '使用该位置' }}
        </button>
      </div>
    </template>

    <!-- 桌面:盘符选择(原行为) -->
    <template v-else>
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
      <button class="sv-btn primary" :disabled="!canApply" @click="apply">
        {{ busy ? '处理中…' : '使用该位置' }}
      </button>
    </template>
  </div>
  <p v-else class="sv-note">正在读取盘符…</p>
</template>
