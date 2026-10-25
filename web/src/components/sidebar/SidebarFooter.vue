<script setup lang="ts">
// 侧栏底部:角色卡导入 + 综合设置 + 退出 + 隐藏 file input（C-2A，2026-10-25）。
// 导入能力只在这一处；ChatWindow 空态通过 window 事件复用同一个 file input
// （原实现在 Sidebar setup 顶层注册监听，拆分后随本组件生命周期注册/摘除）。
import { onBeforeUnmount, ref } from 'vue';
import { useAppStore } from '../../store';

const store = useAppStore();

const fileInput = ref<HTMLInputElement | null>(null);
const uploadError = ref('');

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

onBeforeUnmount(() => {
  window.removeEventListener('kedai:open-character-import', onExternalImportRequest);
});
</script>

<template>
  <div class="sv-side-foot">
    <div v-if="uploadError" class="sv-feedback err">{{ uploadError }}</div>
    <button
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
    <input
      ref="fileInput"
      type="file"
      accept=".png,.json,image/png,application/json"
      class="hidden"
      @change="onFilePicked"
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
