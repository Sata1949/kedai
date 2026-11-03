<script setup lang="ts">
// 侧栏底部:角色卡导入 / 任务附件导入 + 综合设置 + 退出 + 隐藏 file input（C-2A，2026-10-25）。
// 导入能力只在这一处；ChatWindow 空态通过 window 事件复用同一个 file input
// （原实现在 Sidebar setup 顶层注册监听，拆分后随本组件生命周期注册/摘除）。
//
// ATTACH-1（2026-11-03）：**按 appMode 分叉**——角色扮演仍是「导入角色卡」（单文件、
// 只吃 PNG/JSON、导入后自动选中），任务模式改成「导入附件」（任意文件、可多选）。
// 两条路径的差异不是配置，是语义：角色卡是**可执行的实体**（解析后进角色库），
// 附件只是**交给任务的文件**（落盘后由模型按需读取）。故各自一份 file input 与处理函数，
// 而不是给同一个 input 加条件属性——那样 accept/multiple 的默认值会在两模式间互相污染。
import { computed, onBeforeUnmount, ref } from 'vue';
import { storeToRefs } from 'pinia';
import { useAppStore } from '../../store';
import { useTaskAttachments } from '../../composables/useTaskAttachments';

const store = useAppStore();
const { appMode, currentTaskId } = storeToRefs(store);

const fileInput = ref<HTMLInputElement | null>(null);
const uploadError = ref('');

/** 任务模式的附件入口(与详情面板共享同一份单例状态) */
const attachments = useTaskAttachments();

/** 当前是任务模式吗(决定按钮文案、input 属性与选中后的去向) */
const isTaskMode = computed(() => appMode.value === 'task');

/** 附件相关的错误(上传失败 / 超限);与会话内的角色卡导入错误分开显示 */
const attachmentError = computed(() => attachments.lastError.value || uploadError.value);

function pickCharacterFile(): void {
  uploadError.value = '';
  fileInput.value?.click();
}

/**
 * 空态「导入角色卡」入口(UIQ-4):ChatWindow 没有文件输入,导入能力只在本组件;
 * 用一次点击事件复用同一个 file input,不复制导入逻辑,也不新增 store 字段。
 * 事件由用户点击同步触发,浏览器仍视为用户手势,file picker 可正常打开。
 */
function onExternalImportRequest(): void {
  pickCharacterFile();
}
window.addEventListener('kedai:open-character-import', onExternalImportRequest);

async function onFilePicked(e: Event): Promise<void> {
  const input = e.target as HTMLInputElement;
  const file = input.files?.[0];
  input.value = '';
  if (!file) return;
  uploadError.value = '';
  try {
    await store.uploadCharacter(file);
  } catch (err) {
    uploadError.value = (err as Error).message;
  }
}

// ==================== 任务附件路径 ====================

const taskFileInput = ref<HTMLInputElement | null>(null);

function pickTaskFiles(): void {
  attachments.dismissError();
  taskFileInput.value?.click();
}

/**
 * 任务模式的附件入口。
 *
 * **有时序差**:选中了任务就直传(用户要的是「给这个任务加文件」);没选中任务则
 * 先进待上传队列,由 `TaskComposer.createAndRun` 在创建成功后统一刷上去
 * ——否则用户的点击会落在一个还不存在的 id 上。
 */
async function onTaskFilesPicked(e: Event): Promise<void> {
  const input = e.target as HTMLInputElement;
  const files = Array.from(input.files ?? []);
  input.value = '';
  if (files.length === 0) return;
  const taskId = currentTaskId.value;
  if (taskId) {
    await attachments.uploadTo(taskId, files);
  } else {
    attachments.enqueue(files);
  }
}

/** 任务模式下按钮的提示文案(如实说明去向) */
const taskPickerTitle = computed(() =>
  currentTaskId.value
    ? '导入附件:任意文件,可多选(直接加到当前任务)'
    : '导入附件:任意文件,可多选(创建任务时一并提交)',
);

onBeforeUnmount(() => {
  window.removeEventListener('kedai:open-character-import', onExternalImportRequest);
});
</script>

<template>
  <div class="sv-side-foot">
    <div v-if="attachmentError" class="sv-feedback err">{{ attachmentError }}</div>
    <!-- 角色扮演:导入角色卡(行为与 ATTACH-1 之前逐字一致) -->
    <button
      v-if="!isTaskMode"
      class="sv-btn sv-side-btn"
      title="导入角色卡(PNG 卡片或 JSON;导入后自动选中)"
      @click="pickCharacterFile"
    >
      <span class="sv-side-btn-ico">
        <svg viewBox="0 0 24 24">
          <path d="M12 5v14" />
          <path d="M5 12h14" />
        </svg>
      </span>
      导入角色卡
    </button>
    <!-- 任务模式:导入附件(任意文件、可多选) -->
    <button
      v-else
      class="sv-btn sv-side-btn"
      :title="taskPickerTitle"
      @click="pickTaskFiles"
    >
      <span class="sv-side-btn-ico">
        <svg viewBox="0 0 24 24">
          <path d="M12 5v14" />
          <path d="M5 12h14" />
        </svg>
      </span>
      导入附件
    </button>
    <button
      class="sv-btn primary sv-side-btn"
      @click="store.settingsOpen = true"
    >
      <span class="sv-side-btn-ico">
        <svg viewBox="0 0 24 24">
          <circle cx="12" cy="12" r="3" />
          <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06-.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
        </svg>
      </span>
      综合设置
    </button>
    <!-- 角色卡 file input:accept/multiple 保持 ATTACH-1 之前的原值,角色扮演路径零变化 -->
    <input
      ref="fileInput"
      type="file"
      accept=".png,.json,image/png,application/json"
      class="hidden"
      @change="onFilePicked"
    />
    <!-- 任务附件 file input:**不带 accept**(任何文件)且多选 -->
    <input
      ref="taskFileInput"
      type="file"
      multiple
      class="hidden"
      @change="onTaskFilesPicked"
    />
    <!-- 退出入口(2026-09-17):此前界面上**没有任何**退出按钮——Android 只能靠
         返回键碰运气触发确认弹窗,桌面只能关窗口。补一个显式入口,点击后走同一个
         退出确认弹窗(不直接退:误触不丢未保存内容)。 -->
    <button
      class="sv-btn sv-side-btn sidebar-exit"
      title="退出 Kedai(会先弹确认)"
      @click="store.exitConfirmOpen = true"
    >
      <span class="sv-side-btn-ico">
        <svg viewBox="0 0 24 24">
          <path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4" />
          <path d="M16 17l5-5-5-5" />
          <path d="M21 12H9" />
        </svg>
      </span>
      退出 Kedai
    </button>
  </div>
</template>

<style scoped>
/* 退出按钮(2026-09-17 新增入口)。样式纪律(MAINTENANCE D-5):新增组件样式写 scoped,
   不进 style.css;这里只覆盖该按钮特有的一点点视觉,尺寸/图标沿用 .sv-side-btn。 */
.sidebar-exit {
  /* 与「综合设置」(primary 黑底)区分层级:空心描边,悬停才转红提示危险语义 */
  background: transparent;
  border: var(--bw-thin) solid var(--sv-line-strong);
  color: var(--sv-ink-dim);
}
.sidebar-exit:hover {
  border-color: var(--sv-red);
  color: var(--sv-red);
}
</style>
