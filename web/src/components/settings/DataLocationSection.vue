<script setup lang="ts">
// 设置区:数据位置(2026-11 盘符选择)。展示当前根与四个子目录,支持更改。
// 更改会迁移旧数据并要求重启(SQLite 与数据根在服务启动期绑定,运行期不能热切换)。
import { onMounted, ref } from 'vue';
import * as api from '../../api';
import { restartApp } from '../../platform';
import DataLocationChooser from '../DataLocationChooser.vue';

withDefaults(defineProps<{ show?: boolean }>(), { show: true });

const status = ref<api.DataLocationStatus | null>(null);
const editing = ref(false);
const busy = ref(false);
const msg = ref<{ kind: 'ok' | 'err'; text: string } | null>(null);

async function load(): Promise<void> {
  try {
    status.value = await api.getDataLocation();
  } catch (e) {
    msg.value = { kind: 'err', text: `读取数据位置失败:${(e as Error).message}` };
  }
}

onMounted(load);

async function choose(target: string): Promise<void> {
  if (busy.value) return;
  const hasData = !!status.value?.legacy_data || !!status.value?.root;
  if (hasData && !window.confirm('将把现有数据复制到新位置(原目录保留),之后需要重启 Kedai。继续?')) {
    return;
  }
  busy.value = true;
  msg.value = null;
  try {
    // Android 传绝对路径根,桌面传盘符:以是否以 / 开头区分(桌面盘符恒为单字母)
    const result = await api.selectDataLocation(
      target.startsWith('/') ? { root: target } : target,
    );
    msg.value = {
      kind: 'ok',
      text: result.migrated
        ? `已保存并迁移到 ${result.root},正在重启…`
        : `已保存数据位置 ${result.root},正在重启…`,
    };
    editing.value = false;
    const restarted = await restartApp();
    if (!restarted) {
      msg.value = { kind: 'ok', text: `已保存到 ${result.root};请关闭并重新启动 Kedai 以生效` };
    }
  } catch (e) {
    msg.value = { kind: 'err', text: (e as Error).message };
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <div v-show="show" class="sv-field">
    <div class="sv-field-label"><span class="sv-supreme blue" /> 数据位置</div>
    <div class="sv-datalist">
      <div class="sv-data-row">
        <div class="info">
          <b>当前数据根</b>
          <span>{{ status?.root ?? '未选择(当前使用旧默认目录)' }}</span>
        </div>
        <button class="sv-btn ghost" :disabled="busy" @click="editing = !editing">
          {{ editing ? '取消' : '更改位置' }}
        </button>
      </div>
      <div class="sv-data-row">
        <div class="info"><b>系统数据</b><span>{{ status?.data_dir ?? '—' }}</span></div>
      </div>
      <div class="sv-data-row">
        <div class="info"><b>产物</b><span>{{ status?.images_dir ?? '—' }}</span></div>
      </div>
      <div class="sv-data-row">
        <div class="info"><b>任务记录</b><span>{{ status?.task_records_dir ?? '—' }}</span></div>
      </div>
      <div class="sv-data-row">
        <div class="info"><b>综合导入文件</b><span>{{ status?.imports_dir ?? '—' }}</span></div>
      </div>
    </div>
    <DataLocationChooser
      v-if="editing"
      :status="status"
      :busy="busy"
      @choose="choose"
      @refresh="load"
    />
    <p v-if="msg" class="sv-feedback" :class="msg.kind === 'err' ? 'err' : 'ok'">{{ msg.text }}</p>
    <p class="sv-note">
      更改后旧数据会复制到新位置（原目录保留），并需要重启 Kedai；
      若所选位置已有 Kedai 用户数据，会被拒绝以免覆盖。
    </p>
  </div>
</template>
