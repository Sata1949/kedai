<script setup lang="ts">
// 设置区:数据管理(聊天导出 / 导入 / 清空当前会话 + 回退快照开关)。
// 从 SettingsModal.vue 双模板合并而来:两分支内容一致。
// 状态由壳(SettingsModal)创建一次后经 prop 传入,与 UiSection 共享 useDataManager。
// TM-SET-3:导出/导入/清空为聊天(会话)专属,任务模式下隐藏;回退快照为全局开关常显。
import { computed, ref } from 'vue';
import { useAppStore } from '../../store';
import { storeToRefs } from 'pinia';
import type { useDataManager } from '../../composables/useDataManager';
import * as api from '../../api';

const props = withDefaults(defineProps<{
  /** useDataManager 的返回对象(壳共享实例) */
  state: ReturnType<typeof useDataManager>;
  /** 是否显示(embedded 模式按 activeSection 切换;standalone 恒 true) */
  show?: boolean;
}>(), {
  show: true,
});

const {
  importInput, importError, exportMsg, clearMsg,
  onImportFile, clearAllData, exportChat,
  undoEnabled, undoMsg, saveUndoEnabled,
} = props.state;

const store = useAppStore();
const { currentSessionId } = storeToRefs(store);
/** 任务模式:无会话概念,聊天导出/导入/清空隐藏 */
const isTaskMode = computed(() => store.appMode === 'task');

/** 任务草稿立即清理(PRODCAP-5):与空闲看守的自动清理共用同一实现 */
const scratchBusy = ref(false);
const scratchMsg = ref('');
async function onCleanupScratch(): Promise<void> {
  scratchBusy.value = true;
  scratchMsg.value = '';
  try {
    const r = await api.cleanupTaskScratch();
    scratchMsg.value =
      r.keep_days === 0
        ? '保留策略已关闭(KEDAI_TASK_SCRATCH_KEEP_DAYS=0),未执行清理'
        : `已清理 ${r.removed.length} 个任务草稿目录(保留中 ${r.kept_fresh},跳过进行中 ${r.skipped_active.length}${
            r.failed.length > 0 ? `,失败 ${r.failed.length}(见服务端日志)` : ''
          })`;
  } catch (e) {
    scratchMsg.value = `清理失败:${e instanceof Error ? e.message : String(e)}`;
  } finally {
    scratchBusy.value = false;
  }
}
</script>

<template>
  <div v-show="props.show" class="sv-field">
    <div class="sv-field-label"><span class="sv-supreme blue" /> 数据管理</div>
    <div class="sv-datalist">
      <template v-if="!isTaskMode">
      <div class="sv-data-row">
        <div class="info">
          <b>导出聊天</b>
          <span>当前会话导出为 SillyTavern 兼容 JSON</span>
        </div>
        <button class="sv-btn ghost" :disabled="!currentSessionId" @click="exportChat">导出</button>
      </div>
      <div class="sv-data-row">
        <div class="info">
          <b>导入聊天</b>
          <span>导入 JSON 替换当前会话内容</span>
        </div>
        <button class="sv-btn ghost" :disabled="!currentSessionId" @click="importInput?.click()">导入</button>
      </div>
      <div class="sv-data-row">
        <div class="info">
          <b>清空当前会话</b>
          <span>删除本会话全部消息,保留角色定义</span>
        </div>
        <button class="sv-btn danger" :disabled="!currentSessionId" @click="clearAllData">清空</button>
      </div>
      </template>
      <div class="sv-data-row">
        <div class="info">
          <b>回退快照(undo)</b>
          <span>写工具(写文件/改变量等)执行前自动存档,可在 Agent 面板回退到该次修改前(全局设置,两模式共用)</span>
        </div>
        <label style="display: flex; gap: var(--space-1-5); align-items: center; cursor: pointer" title="开启后写工具执行前自动保存快照,Agent 面板工具调用项出现「回退到此处」入口">
          <input v-model="undoEnabled" type="checkbox" style="flex-shrink: 0" @change="saveUndoEnabled" />
          <span class="sv-note">{{ undoEnabled ? '已开启' : '已关闭' }}</span>
        </label>
      </div>
      <div class="sv-data-row">
        <div class="info">
          <b>清理任务草稿</b>
          <span>删除任务产物目录中超过保留期(默认 30 天,见 KEDAI_TASK_SCRATCH_KEEP_DAYS)且不在运行/待批准任务的内容;自动清理由空闲看守顺带执行</span>
        </div>
        <button class="sv-btn ghost" :disabled="scratchBusy" @click="onCleanupScratch">立即清理</button>
      </div>
    </div>
    <div v-if="importError" class="sv-feedback err">{{ importError }}</div>
    <div v-if="exportMsg" class="sv-feedback ok">{{ exportMsg }}</div>
    <div v-if="clearMsg" class="sv-feedback ok">{{ clearMsg }}</div>
    <div v-if="undoMsg" class="sv-feedback" :class="undoMsg.startsWith('保存失败') ? 'err' : 'ok'">{{ undoMsg }}</div>
    <div v-if="scratchMsg" class="sv-feedback" :class="scratchMsg.startsWith('清理失败') ? 'err' : 'ok'">{{ scratchMsg }}</div>
    <input ref="importInput" type="file" accept=".json,application/json" class="hidden" @change="onImportFile" />
    <p v-if="!isTaskMode" class="sv-note">
      导入格式与 SillyTavern 兼容:<code>[{"role":"user","content":"..."}]</code>
    </p>
    <p v-else class="sv-note">
      任务模式没有会话概念,聊天导出 / 导入 / 清空已隐藏;回退快照为全局开关,两模式共用。
    </p>
  </div>
</template>
