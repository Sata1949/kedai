<script setup lang="ts">
// 设置区:界面(Agent 面板开关 / 安全 HTML 渲染 / 角色卡 JavaScript 授权管理)。
// 从 SettingsModal.vue 双模板合并而来:取 embedded 版本(授权说明与撤销列表更完整)。
import { useAppStore } from '../../store';
import { storeToRefs } from 'pinia';
import { onMounted, onUnmounted, ref } from 'vue';
import type { useDataManager } from '../../composables/useDataManager';
import {
  applyTheme,
  getActiveThemeId,
  listThemes,
  preloadTheme,
  subscribeThemes,
  type ThemeSummary,
} from '../../theme/registry';

const props = withDefaults(defineProps<{
  /** useDataManager 的返回对象(壳共享实例,本区仅用 characterLabel / 撤销授权) */
  state: ReturnType<typeof useDataManager>;
  /** 是否显示(embedded 模式按 activeSection 切换;standalone 恒 true) */
  show?: boolean;
}>(), {
  show: true,
});

const { characterLabel, confirmRevokeScriptAuthorization } = props.state;

const store = useAppStore();
const { scriptAuthorizations } = storeToRefs(store);

// 外观主题:注册表是纯前端扩展点,不写 settings.json;订阅只在浏览器挂载后建立(SSR 不泄漏监听)。
const themes = ref<ThemeSummary[]>(listThemes());
const activeThemeId = ref(getActiveThemeId());
const switchingTheme = ref<string | null>(null);
let unsubscribeThemes: (() => void) | undefined;
onMounted(() => {
  unsubscribeThemes = subscribeThemes((snapshot) => {
    themes.value = snapshot.themes;
    activeThemeId.value = snapshot.activeId;
  });
});
onUnmounted(() => unsubscribeThemes?.());

/** 插件主题没有专用色板时用通用渐变,内置两款走各自的识别色。 */
function swatchClass(id: string): string {
  if (id === 'stained-glass') return 'swatch-glass';
  if (id === 'default') return 'swatch-default';
  return 'swatch-generic';
}

/**
 * 先预载 CSS,再原子切换:避免 apply 先改令牌、CSS 后到造成的「半截主题」,
 * 也避免首次切到彩窗时把主题 CSS 的解析/重绘压在点击那一帧。
 */
async function selectTheme(id: string): Promise<void> {
  if (switchingTheme.value) return;
  switchingTheme.value = id;
  try {
    await preloadTheme(id);
    await applyTheme(id);
  } finally {
    switchingTheme.value = null;
  }
}
</script>

<template>
  <div v-show="props.show" class="sv-field">
    <div class="sv-field-label"><span class="sv-supreme yellow" /> 界面</div>

    <!-- 外观主题:内置主题与外部插件主题同一注册表;选中即应用并写入 localStorage -->
    <div class="sv-theme-block">
      <div class="sv-field-label"><span class="sv-supreme purple" /> 外观主题</div>
      <div class="sv-theme-grid" role="group" aria-label="外观主题">
        <button
          v-for="theme in themes"
          :key="theme.id"
          type="button"
          class="sv-theme-card"
          :class="{ active: theme.id === activeThemeId }"
          :aria-pressed="theme.id === activeThemeId"
          :aria-busy="switchingTheme === theme.id"
          :disabled="switchingTheme !== null"
          @click="selectTheme(theme.id)"
        >
          <span class="sv-theme-swatch" :class="swatchClass(theme.id)" aria-hidden="true" />
          <span class="sv-theme-copy">
            <b>
              {{ theme.name }}<em v-if="theme.builtin">内置</em><em v-if="switchingTheme === theme.id" class="sv-theme-active">切换中…</em><em v-else-if="theme.id === activeThemeId" class="sv-theme-active">使用中</em>
            </b>
            <span>{{ theme.description }}</span>
          </span>
        </button>
      </div>
      <p class="sv-note sv-mt8">
        主题是前端扩展点：外部插件可通过 <code>window.KedaiUI.registerTheme()</code> 注册；
        字体 Cinzel / Cormorant Garamond 为 SIL OFL 1.1，中文回退系统宋体。
      </p>
    </div>

    <div class="sv-datalist">
      <!-- 默认模式(2026-09-27 首启引导批次):偏好决定每次启动进入的模式,
           未设置时沿用旧行为「按上次用过的模式进入」。放在本区首行——它比下面几项
           更基础(决定启动后落到哪个工作台),且与顶栏/底部导航的模式切换互为补充。 -->
      <div class="sv-data-row">
        <div class="info">
          <b>默认模式</b>
          <span>决定每次启动 Kedai 后默认进入的模式（未设置时按上次使用过的模式进入）。点击即切换并保存，首次启动的引导里选的也是这一项。</span>
        </div>
        <!-- 两按钮组:shrink-0 + nowrap 是必须的——行内说明文字会占满宽度,不给这两条则按钮被压成
             竖排折行(实测「角色扮演」折成 4 行);选中态用 sv-btn-on(粉底)表示,未设置过时两者都不高亮。 -->
        <div class="flex items-center gap-2 shrink-0 whitespace-nowrap">
          <button
            type="button"
            class="sv-btn ghost"
            :class="{ 'sv-btn-on': store.defaultAppMode === 'roleplay' }"
            :aria-pressed="store.defaultAppMode === 'roleplay'"
            @click="store.setDefaultAppMode('roleplay')"
          >
            角色扮演
          </button>
          <button
            type="button"
            class="sv-btn ghost"
            :class="{ 'sv-btn-on': store.defaultAppMode === 'task' }"
            :aria-pressed="store.defaultAppMode === 'task'"
            @click="store.setDefaultAppMode('task')"
          >
            任务
          </button>
        </div>
      </div>
      <div class="sv-data-row">
        <div class="info">
          <b>Agent 面板</b>
          <span>生成时自动展开，展示步骤、推理链与工具调用</span>
        </div>
        <button class="sv-btn ghost" @click="store.toggleAgentPanel()">
          {{ store.agentPanelOpen ? '展开中' : '已折叠' }}
        </button>
      </div>
      <div class="sv-data-row">
        <div class="info">
          <b>安全 HTML 渲染</b>
          <span>应用角色卡正则替换并清理 script/style 属性、事件处理器、javascript URL 与外部资源；不代表允许 JavaScript。开关按角色卡记忆，未设置的角色卡回退全局默认</span>
        </div>
        <button type="button" class="sv-btn ghost" :aria-pressed="store.renderHtml" @click="store.toggleRenderHtml()">
          {{ store.renderHtml ? '已开启' : '已关闭' }}
        </button>
      </div>
      <div class="sv-data-row" style="align-items: flex-start">
        <div class="info">
          <b>角色卡 JavaScript 授权</b>
          <span>默认禁用；按角色 ID 与脚本哈希授权，内容变化后自动失效。授权记录仅保存在当前浏览器配置中。</span>
          <div v-if="scriptAuthorizations.length" style="display: grid; gap: var(--space-2); margin-top: var(--space-2-5)">
            <div v-for="grant in scriptAuthorizations" :key="grant.characterId" class="flex items-center gap-2">
              <span>{{ characterLabel(grant.characterId) }} · {{ new Date(grant.authorizedAt).toLocaleString('zh-CN', { hour12: false }) }}</span>
              <button type="button" class="sv-btn ghost sv-btn-sm" @click="confirmRevokeScriptAuthorization(grant.characterId)">撤销</button>
            </div>
          </div>
          <span v-else>暂无已授权角色卡。</span>
        </div>
      </div>
      <!-- 退出应用(2026-09-17):此前界面上没有退出入口,只能靠 Android 返回键
           碰运气触发,或桌面关窗口。放在「界面」区末尾——危险操作不与其它设置项混排,
           且点击后走同一个退出确认弹窗(不直接退,误触不丢未保存内容)。 -->
      <div class="sv-data-row">
        <div class="info">
          <b>退出应用</b>
          <span>结束 Kedai 进程(正在生成或执行中的任务会被中断)。Android 上也可连按两次返回键退出。</span>
        </div>
        <button type="button" class="sv-btn danger" @click="store.exitConfirmOpen = true">退出 Kedai</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* 外观主题选择:主题卡只做识别色预览,真实样式由主题令牌/CSS 注入 */
.sv-theme-block {
  margin-bottom: var(--space-4);
  padding-bottom: var(--space-4);
  border-bottom: 1px solid var(--sv-line);
}
.sv-theme-grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
  gap: var(--space-2);
}
.sv-theme-card {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
  padding: var(--space-2);
  border: 1px solid var(--sv-line-strong);
  background: var(--sv-surface-elevated);
  text-align: left;
  cursor: pointer;
}
.sv-theme-card:hover { border-color: var(--sv-ink); }
.sv-theme-card.active {
  border-color: var(--sv-ink);
  box-shadow: inset 0 0 0 1px var(--sv-ink);
}
.sv-theme-swatch {
  display: block;
  height: 52px;
  border: 1px solid var(--sv-ink);
}
.swatch-default {
  background: linear-gradient(135deg, #fdfafb 0 52%, #d62b25 52% 64%, #17181a 64%);
}
.swatch-glass {
  background-color: #141016;
  background-image:
    radial-gradient(circle 24px at 82% 50%, #e6c072 0 3px, #9c2540 3px 10px, #143a7c 10px 22px, transparent 22px),
    var(--kg-mosaic, none),
    linear-gradient(90deg, #141016 0 18%, #1c4a9c 18% 48%, #9c2540 48% 72%, #c99a3b 72% 100%);
  background-size: auto, 160px 73px, 100% 100%;
  background-repeat: no-repeat, repeat, no-repeat;
}
.swatch-generic {
  background: linear-gradient(135deg, var(--sv-pink-light), var(--sv-pink-deep), var(--sv-ink));
}
.sv-theme-copy {
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
  min-width: 0;
}
.sv-theme-copy b {
  display: flex;
  align-items: center;
  gap: var(--space-1-5);
  font-size: var(--text-xs);
}
.sv-theme-copy b em {
  font-style: normal;
  font-size: 10px;
  letter-spacing: 0.08em;
  color: var(--sv-ink-faint);
  border: 1px solid var(--sv-line-strong);
  padding: 0 var(--space-1);
}
.sv-theme-copy b em.sv-theme-active {
  border-color: var(--sv-ink);
  background: var(--sv-ink);
  color: var(--sv-white);
}
.sv-theme-copy span {
  font-size: var(--text-2xs);
  line-height: var(--lh-body);
  color: var(--sv-ink-faint);
}
</style>
