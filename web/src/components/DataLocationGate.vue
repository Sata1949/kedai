<script setup lang="ts">
// 老用户升级后的**不可关闭**数据位置弹窗。
//
// 触发条件 = 后端 pending(无指针/指针根不可用)且教程没有打开:
//   - 新用户首次启动:教程第二步负责选择,本组件让位(store.onboardingOpen);
//   - 老用户(教程已完成)升级后:本组件强制选择;
//   - 用户在教程里跳过:教程关闭后本组件接管。
// 选择成功后写指针并迁移旧数据,再请求壳重启(浏览器提示手动重启)。
import { computed, onMounted, ref } from 'vue';
import { useAppStore } from '../store';
import * as api from '../api';
import { restartApp } from '../platform';
import DataLocationChooser from './DataLocationChooser.vue';

const store = useAppStore();
const status = ref<api.DataLocationStatus | null>(null);
const busy = ref(false);
const error = ref('');
const saved = ref(false);

const visible = computed(
  () => !!status.value?.pending && !store.onboardingOpen,
);
const isAndroid = computed(() => status.value?.platform === 'android');

async function load(): Promise<void> {
  try {
    status.value = await api.getDataLocation();
  } catch (e) {
    error.value = `读取数据位置失败:${(e as Error).message}`;
  }
}

onMounted(load);

async function choose(target: string): Promise<void> {
  if (busy.value || saved.value) return;
  busy.value = true;
  error.value = '';
  try {
    // Android 传绝对路径根,桌面传盘符:以是否以 / 开头区分(桌面盘符恒为单字母)
    await api.selectDataLocation(target.startsWith('/') ? { root: target } : target);
    saved.value = true;
    const restarted = await restartApp();
    if (!restarted) {
      error.value = '数据位置已保存。请关闭并重新启动 Kedai,重启后即可使用新位置';
    }
  } catch (e) {
    error.value = (e as Error).message;
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <div
    v-if="visible"
    class="sv-modal-mask"
    role="dialog"
    aria-modal="true"
    aria-label="选择数据保存位置"
  >
    <div class="sv-modal md">
      <div class="sv-modal-head">
        <h2><span class="sv-supreme blue" /> 选择数据保存位置</h2>
      </div>
      <div class="sv-modal-body">
        <p class="sv-note">
          <template v-if="isAndroid">
            Kedai 需要先确定数据保存位置才能继续使用。Android 默认使用公共「下载」目录下的
            <code>Kedai-Data</code>（卸载应用后数据仍保留），内含系统数据、产物、任务记录、
            综合导入文件四个子目录。
          </template>
          <template v-else>
            Kedai 需要先确定数据保存的盘符才能继续使用。选择后会在该盘创建
            <code>Kedai-Data</code>,内含系统数据、产物、任务记录、综合导入文件四个子目录。
          </template>
        </p>
        <p v-if="status?.error" class="sv-feedback err">{{ status.error }}</p>
        <p v-if="status?.legacy_data" class="sv-note">
          检测到已有数据：确认后会复制到新位置，原目录保留（不会删除）。
        </p>
        <DataLocationChooser
          :status="status"
          :busy="busy"
          :disabled="saved"
          @choose="choose"
          @refresh="load"
        />
        <p v-if="error" class="sv-feedback err">{{ error }}</p>
        <p v-if="saved && !error" class="sv-feedback ok">数据位置已保存，正在重启…</p>
      </div>
    </div>
  </div>
</template>
