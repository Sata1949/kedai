<script setup lang="ts">
// 双库分叉提示条(DB-7):另一套数据目录也有用户数据时,在应用顶部常驻一条 info 提示。
//
// 为什么要独立成组件而不是写在 App.vue 模板里:它是**可独立验收的展示单元**——
// 「有分叉时列出每一条路径与判据、无分叉时整块不渲染、关闭按钮生效」这三条判据
// 都能用 SSR 直接断言,不必拉起整个根组件(根组件挂着 Sidebar/ChatWindow/TaskBoard
// 与一批启动期 fetch,SSR 渲染它的成本远大于这里要验的东西)。
//
// 文案口径:给**路径**才是可操作信息——「聊天记录没了」是用户看到的现象,
// 「另一套库在 C:\…\data」才是能据以行动的事实。故逐条列出 path + evidence,
// 不把它压成一句「检测到多套数据目录」。
import type { SplitDataDirsNotice } from '../stores/uiPrefs';

defineProps<{
  /** 分叉提示载荷(`null` 由父组件 v-if 拦掉,这里只处理有值的情形) */
  notice: SplitDataDirsNotice;
}>();

const emit = defineEmits<{
  /** 用户关闭提示(本会话不再显示;下次启动重新检测) */
  (e: 'dismiss'): void;
}>();
</script>

<template>
  <div class="sv-render-hint sv-split-dirs-hint" role="status">
    <span class="sv-render-hint-text">
      检测到另一套数据目录也有用户数据(当前使用 {{ notice.dataDir }}):
      <template v-for="(other, i) in notice.others" :key="other.path">
        <span v-if="i > 0">;</span>{{ other.path }}<template v-if="other.evidence">({{ other.evidence }})</template>
      </template>
      。两套库互不可见，请确认要使用哪一套（设置 DATA_DIR，或清理其一）。
    </span>
    <button
      type="button"
      class="sv-icon-btn"
      title="关闭提示(下次启动会重新检测)"
      @click="emit('dismiss')"
    >✕</button>
  </div>
</template>

<style scoped>
/* 沿用全局 .sv-render-hint 的 info 视觉,只补两点(全局域样式只应缩小,
   见 tools/check-css-discipline.mjs 的行数 ratchet)。
   ① 文字允许换行:路径通常很长,`.sv-render-hint-text` 的 nowrap 会把唯一可操作
      信息截成省略号;② 左对齐:居中排版对多行路径最难读。 */
.sv-split-dirs-hint {
  align-items: flex-start;
}
.sv-split-dirs-hint .sv-render-hint-text {
  white-space: normal;
  text-align: left;
  line-height: 1.5;
}
</style>
