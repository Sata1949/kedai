<script setup lang="ts">
// 聊天顶栏(自 ChatWindow 拆出):会话切换/新建、开场切换入口、HTML 渲染开关、
// JS 脚本授权按钮、Agent 面板开关、token 统计,以及顶栏下的渲染引导条。
// 全部状态直读 store(与 ChatInput 等组件同一约定);多开场入口经事件上抛父组件。
import { ref, computed } from 'vue';
import { storeToRefs } from 'pinia';
import { useAppStore } from '../store';
import { computeHitRate } from '../contextStats';

const emit = defineEmits<{
  /** 打开开场选择器(new=新建会话时选择;switch=当前会话内重置开场) */
  (e: 'open-greeting-picker', mode: 'new' | 'switch'): void;
}>();

const store = useAppStore();
const {
  model, currentSessionId, currentCharacterId, sessions, currentCharacter, currentCharacterName,
  currentGreetings, renderHtml, currentScriptAuthorized,
} = storeToRefs(store);

/** 当前角色正则脚本(渲染引导条的可渲染判定) */
const currentScripts = computed(() => currentCharacter.value?.regex_scripts ?? []);

/**
 * 渲染引导条(bug:wuwa 卡「只有文字没有界面」的主要成因是用户不知道要开渲染):
 * 角色卡含有替换体的启用脚本(即可渲染界面)且 HTML 渲染处于关闭态时,在顶栏下
 * 显示一键开启提示;关闭动作按角色记忆,换卡后重新评估。
 */
const hasRenderableScripts = computed(() =>
  currentScripts.value.some((s) => s.enabled && !!s.replace_string?.trim()),
);
const renderHintDismissed = ref<Set<string>>(new Set());
const renderHintVisible = computed(() => {
  const id = currentCharacterId.value;
  return !!id && hasRenderableScripts.value && !renderHtml.value && !renderHintDismissed.value.has(id);
});
function dismissRenderHint(): void {
  const id = currentCharacterId.value;
  if (!id) return;
  const next = new Set(renderHintDismissed.value);
  next.add(id);
  renderHintDismissed.value = next;
}
/** 一键开启 HTML 渲染;并顺带提示可再授权 JS(授权按钮常驻,指引用户二次确认) */
function enableRenderFromHint(): void {
  store.renderHtml = true;
  dismissRenderHint();
}

// 会话切换
async function onSessionChange(e: Event): Promise<void> {
  const id = (e.target as HTMLSelectElement).value;
  if (id) await store.switchSession(id);
}

function confirmScriptAuthorization(): void {
  if (currentScriptAuthorized.value) {
    if (confirm(`撤销“${currentCharacterName.value}”当前脚本版本的执行授权？\n\n安全 HTML 仍可显示，但角色卡 JavaScript 将停止执行。`)) {
      store.revokeCharacterScripts(currentCharacter.value?.id ?? '');
    }
    return;
  }
  const accepted = confirm(
    `允许“${currentCharacterName.value}”执行角色卡 JavaScript？\n\n` +
    '脚本可能修改消息卡片并读取当前会话变量。Kedai 会隔离宿主 window/document,并遮蔽网络、存储与 API token，' +
    '但兼容执行器不是浏览器级安全沙箱。仅在信任角色卡来源时开启。\n\n' +
    '授权仅适用于此角色与当前脚本内容；脚本变化后会自动失效。',
  );
  if (accepted) store.authorizeCurrentCharacterScripts();
}

/** 当前请求的 prompt 缓存命中率:命中率计算抽到 contextStats.ts(computeHitRate,与优化面板共用) */
const hitRate = computed<number | null>(() => computeHitRate(store.lastUsage));
</script>

<template>
  <!-- 顶栏 -->
  <header class="sv-topbar">
    <span class="flex items-center gap-2">
      <span class="sv-supreme blue sm" />
      <span class="sv-topbar-title">{{ currentCharacterName }}</span>
      <span v-if="model" class="sv-topbar-sub">{{ model }}</span>
    </span>

    <span class="sv-topbar-right">
      <!-- 会话操作:会话切换 + 开场切换 -->
      <div class="sv-topbar-group">
        <template v-if="sessions.length > 1">
          <label class="sv-topbar-sub" for="session-select">会话</label>
          <select
            id="session-select"
            class="sv-session-select"
            :value="currentSessionId ?? ''"
            @change="onSessionChange"
          >
            <option v-for="s in sessions" :key="s.id" :value="s.id">
              {{ s.title }} · {{ new Date(s.updated_at).toLocaleString('zh-CN', { hour12: false }) }}
            </option>
          </select>
          <button
            class="sv-icon-btn"
            title="新建会话(多开场角色可先选开场)"
            @click="currentGreetings.length > 1 ? emit('open-greeting-picker', 'new') : store.newSession()"
          >＋</button>
        </template>
        <!-- 开场切换(多开场角色:重置当前会话并以所选开场重新开始) -->
        <button
          v-if="currentGreetings.length > 1"
          class="sv-btn ghost sv-btn-sm"
          title="切换开场(清空当前会话并以所选开场重新开始)"
          @click="emit('open-greeting-picker', 'switch')"
        >
          开场
        </button>
      </div>
      <!-- 渲染开关:HTML 渲染 + JS 脚本授权 -->
      <div class="sv-topbar-group">
      <!-- HTML 渲染开关(Toggle 样式;按角色卡记忆;常驻显示——无脚本的卡同样记忆开关状态,
           只是无脚本时开关不产生 scoped 渲染效果) -->
      <button
        type="button"
        class="sv-render-toggle-v2"
        :aria-pressed="renderHtml"
        :title="renderHtml ? '安全 HTML 渲染已开启(仅此角色卡记忆)' : '安全 HTML 渲染已关闭(仅此角色卡记忆)'"
        @click="store.renderHtml = !store.renderHtml"
      >
        <span class="toggle-track" :class="{ on: renderHtml }">
          <span class="toggle-thumb" />
        </span>
        <span class="toggle-label">HTML</span>
      </button>
      <!--
        JS 授权按钮:常驻显示,无论角色卡是否带脚本。
        执行门禁是「有脚本 && 已授权」(chatMessageScriptScheduler),与 replace_string
        是否含字面 <script 无关;按旧条件,脚本不含 <script 字样的卡(如爱托邦卡)
        按钮消失 → 永远无法授权/撤销,与执行条件不一致。无脚本卡授权仅记忆状态,
        执行层因脚本数组为空永不执行。
      -->
      <button
        type="button"
        class="sv-btn ghost sv-btn-sm"
        :aria-pressed="currentScriptAuthorized"
        :title="currentScriptAuthorized ? '撤销当前角色脚本授权' : '查看风险并授权当前角色脚本'"
        @click="confirmScriptAuthorization"
      >
        JS {{ currentScriptAuthorized ? '已授权' : '禁用' }}
      </button>
      </div>
      <!-- Agent 面板开关(面板合并后原「调用情况」独立拨杆收编:与任务工作台顶栏同一入口,
           开合并面板(Agent 状态 / 调用情况双 tab,tab 位置由 uiPrefs.callTraceOpen 记忆),两模式共用) -->
      <div class="sv-topbar-group">
        <button
          type="button"
          class="sv-render-toggle-v2"
          :aria-pressed="store.agentPanelOpen"
          :title="store.agentPanelOpen ? 'Agent 面板已开启(含调用情况)' : 'Agent 面板已关闭(含调用情况)'"
          @click="store.agentPanelOpen = !store.agentPanelOpen"
        >
          <span class="toggle-track" :class="{ on: store.agentPanelOpen }">
            <span class="toggle-thumb" />
          </span>
          <span class="toggle-label">AGENT</span>
        </button>
      </div>
      <span class="sv-token-inline">
        CTX {{ store.contextTokens.toLocaleString() }}
        <span class="sv-supreme pink xxs" />
        {{ store.lastUsage?.total_tokens ?? 0 }}
        <template v-if="hitRate !== null">
          <span class="sv-supreme green xxs" />
          命中 {{ hitRate }}%
        </template>
      </span>
    </span>
  </header>

  <!-- 渲染引导条:角色卡含界面脚本但当前纯文本显示时,给一键开启入口(bug 4 可发现性修复) -->
  <div v-if="renderHintVisible" class="sv-render-hint" role="status">
    <span class="sv-render-hint-text">此角色卡带有界面脚本,当前以纯文本显示,可能看不到状态栏/开场界面。</span>
    <button type="button" class="sv-btn ghost sv-btn-sm" @click="enableRenderFromHint">开启 HTML 渲染</button>
    <button type="button" class="sv-btn ghost sv-btn-sm" title="查看风险后为当前角色卡启用 JavaScript" @click="confirmScriptAuthorization">授权 JS</button>
    <button type="button" class="sv-icon-btn" title="不再提示(仅此角色)" @click="dismissRenderHint">✕</button>
  </div>
</template>
