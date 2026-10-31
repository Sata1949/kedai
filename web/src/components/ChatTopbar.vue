<script setup lang="ts">
// 聊天顶栏(自 ChatWindow 拆出):会话切换/新建、开场切换入口、HTML 渲染开关、
// JS 脚本授权按钮、Agent 面板开关、token 统计,以及顶栏下的渲染引导条。
// 全部状态直读 store(与 ChatInput 等组件同一约定);多开场入口经事件上抛父组件。
import { ref, computed, onMounted, onBeforeUnmount } from 'vue';
import { storeToRefs } from 'pinia';
import { useAppStore } from '../store';
import { computeHitRate } from '../contextStats';
import { hasRenderableScripts as cardHasRenderableScripts, shouldShowJsAuthHint, shouldShowRenderHint } from '../renderHints';

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
const hasRenderableScripts = computed(() => cardHasRenderableScripts(currentScripts.value));
const renderHintDismissed = ref<Set<string>>(new Set());
const jsHintDismissed = ref<Set<string>>(new Set());
/** 两条提示共用的判定输入(真值表见 renderHints.ts 与其测试) */
const hintState = computed(() => {
  const id = currentCharacterId.value;
  return {
    characterId: id,
    renderHtml: renderHtml.value,
    scriptAuthorized: currentScriptAuthorized.value,
    hasRenderable: hasRenderableScripts.value,
    renderHintDismissed: !!id && renderHintDismissed.value.has(id),
    jsHintDismissed: !!id && jsHintDismissed.value.has(id),
  };
});
const renderHintVisible = computed(() => shouldShowRenderHint(hintState.value));
function dismissRenderHint(): void {
  const id = currentCharacterId.value;
  if (!id) return;
  const next = new Set(renderHintDismissed.value);
  next.add(id);
  renderHintDismissed.value = next;
}
/** 一键开启 HTML 渲染;JS 授权由引导条上的独立按钮触发(高危操作不合并进一次点击) */
function enableRenderFromHint(): void {
  store.setRenderHtml(true);
  dismissRenderHint();
}

/**
 * JS 授权引导条(实跑问题:赛马娘卡首楼界面点击全失效的可发现性缺口)。
 * HTML 渲染已开、卡内含界面脚本、但 JS 尚未授权时,界面能显示却完全不响应点击
 * (下拉不更新描述、按钮无反应),用户无从判断原因。这里给出显式提示与授权入口。
 * 与 HTML 提示各用独立 dismissed 集合:关掉其一不影响另一条的提示。
 */
const jsAuthHintVisible = computed(() => shouldShowJsAuthHint(hintState.value));
function dismissJsAuthHint(): void {
  const id = currentCharacterId.value;
  if (!id) return;
  const next = new Set(jsHintDismissed.value);
  next.add(id);
  jsHintDismissed.value = next;
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

/**
 * 单次生成 token 预算 HUD(HB-1):预算为 0 = 关闭,不显示。
 * 已用取本轮 usage 的 total_tokens(prompt+completion,与后端判定口径一致);
 * 超限标红——后端此时要么已停止工具循环(stop),要么已提示过一次(warn)。
 */
const budgetLimit = computed(() => store.sessionTokenBudget ?? 0);
const budgetUsed = computed(() => store.lastUsage?.total_tokens ?? 0);
const budgetOver = computed(() => budgetLimit.value > 0 && budgetUsed.value >= budgetLimit.value);

/**
 * 顶栏技术矩阵(UIQ-4):CTX / 本轮 / 命中 / 预算。
 * 宽屏显示为 1×4 发丝格;<1280px 收进「⋯」菜单,避免把会话/模型挤出顶栏。
 */
const topbarStats = computed(() => {
  const items = [
    { key: 'ctx', label: 'CTX', value: store.contextTokens.toLocaleString(), over: false },
    { key: 'turn', label: '本轮', value: (store.lastUsage?.total_tokens ?? 0).toLocaleString(), over: false },
  ];
  if (hitRate.value !== null) {
    items.push({ key: 'hit', label: '命中', value: `${hitRate.value}%`, over: false });
  }
  if (budgetLimit.value > 0) {
    items.push({
      key: 'budget',
      label: '预算',
      value: `${budgetUsed.value.toLocaleString()}/${budgetLimit.value.toLocaleString()}`,
      over: budgetOver.value,
    });
  }
  return items;
});

/** 窄屏「⋯」菜单:HTML/JS 开关与 token 矩阵的收容处;点击外部/Esc 关闭。 */
const moreOpen = ref(false);
const topbarEl = ref<HTMLElement | null>(null);

function onDocumentPointerDown(event: PointerEvent): void {
  if (!moreOpen.value) return;
  const target = event.target as Node | null;
  if (target && !topbarEl.value?.contains(target)) moreOpen.value = false;
}

function onDocumentKeydown(event: KeyboardEvent): void {
  if (event.key === 'Escape') moreOpen.value = false;
}

onMounted(() => {
  document.addEventListener('pointerdown', onDocumentPointerDown);
  document.addEventListener('keydown', onDocumentKeydown);
});
onBeforeUnmount(() => {
  document.removeEventListener('pointerdown', onDocumentPointerDown);
  document.removeEventListener('keydown', onDocumentKeydown);
});
</script>

<template>
  <!-- 顶栏 -->
  <header ref="topbarEl" class="sv-topbar">
    <span class="sv-topbar-identity">
      <span class="sv-supreme blue sm" />
      <span class="sv-topbar-title truncate min-w-0" :title="currentCharacterName">{{ currentCharacterName }}</span>
      <span v-if="model" class="sv-topbar-sub truncate" :title="model">{{ model }}</span>
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
      <!-- 渲染开关:HTML 渲染 + JS 脚本授权(窄屏收进右侧「⋯」菜单) -->
      <div class="sv-topbar-group sv-topbar-render">
      <!-- HTML 渲染开关(Toggle 样式;按角色卡记忆;常驻显示——无脚本的卡同样记忆开关状态,
           只是无脚本时开关不产生 scoped 渲染效果) -->
      <button
        type="button"
        class="sv-render-toggle-v2"
        :aria-pressed="renderHtml"
        :title="renderHtml ? '安全 HTML 渲染已开启(仅此角色卡记忆)' : '安全 HTML 渲染已关闭(仅此角色卡记忆)'"
        @click="store.toggleRenderHtml()"
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
        class="sv-btn ghost sv-btn-sm sv-topbar-js"
        :aria-pressed="currentScriptAuthorized"
        :title="currentScriptAuthorized ? '撤销当前角色脚本授权' : '查看风险并授权当前角色脚本'"
        @click="confirmScriptAuthorization"
      >
        <span class="sv-topbar-js-dot" :class="{ on: currentScriptAuthorized }" />
        JS {{ currentScriptAuthorized ? '已授权' : '禁用' }}
      </button>
      </div>
      <!-- Agent 面板开关(面板合并后原「调用情况」独立拨杆收编:与任务工作台顶栏同一入口,
           开合并面板(Agent 状态 / 调用情况双 tab,tab 位置由 uiPrefs.callTraceOpen 记忆),两模式共用)
           类名 sv-topbar-agent 供移动端隐藏:窄屏顶栏空间不足,该入口已由底部导航「AGENT」承载 -->
      <div class="sv-topbar-group sv-topbar-agent">
        <button
          type="button"
          class="sv-render-toggle-v2"
          :aria-pressed="store.agentPanelOpen"
          :title="store.agentPanelOpen ? 'Agent 面板已开启(含调用情况)' : 'Agent 面板已关闭(含调用情况)'"
          @click="store.toggleAgentPanel()"
        >
          <span class="toggle-track" :class="{ on: store.agentPanelOpen }">
            <span class="toggle-thumb" />
          </span>
          <span class="toggle-label">AGENT</span>
        </button>
      </div>
      <!-- 技术矩阵:1×4 发丝格;窄屏整组收进「⋯」菜单 -->
      <dl class="sv-topbar-stats" aria-label="上下文与用量">
        <div v-for="s in topbarStats" :key="s.key" class="sv-topbar-stat">
          <dt>{{ s.label }}</dt>
          <dd class="sv-tnum" :class="{ 'sv-budget-over': s.over }">{{ s.value }}</dd>
        </div>
      </dl>

      <!-- 窄屏收容菜单:HTML/JS 开关 + 技术矩阵;宽屏由 CSS 隐藏 -->
      <div class="sv-topbar-more">
        <button
          type="button"
          class="sv-icon-btn sv-topbar-more-btn"
          :aria-expanded="moreOpen"
          aria-haspopup="menu"
          aria-label="更多会话与渲染设置"
          title="更多设置"
          @click="moreOpen = !moreOpen"
        >⋯</button>
        <div v-if="moreOpen" class="sv-topbar-more-menu" role="menu">
          <div class="sv-topbar-more-title">渲染与脚本</div>
          <button
            type="button"
            class="sv-topbar-more-item"
            role="menuitemcheckbox"
            :aria-checked="renderHtml"
            @click="store.toggleRenderHtml()"
          >
            <span>HTML 渲染</span><b>{{ renderHtml ? '开' : '关' }}</b>
          </button>
          <button
            type="button"
            class="sv-topbar-more-item"
            role="menuitemcheckbox"
            :aria-checked="currentScriptAuthorized"
            @click="confirmScriptAuthorization"
          >
            <span>角色卡 JS</span><b>{{ currentScriptAuthorized ? '已授权' : '未授权' }}</b>
          </button>
          <div class="sv-topbar-more-title">上下文与用量</div>
          <dl class="sv-topbar-stats sv-topbar-stats-menu">
            <div v-for="s in topbarStats" :key="`menu-${s.key}`" class="sv-topbar-stat">
              <dt>{{ s.label }}</dt>
              <dd class="sv-tnum" :class="{ 'sv-budget-over': s.over }">{{ s.value }}</dd>
            </div>
          </dl>
        </div>
      </div>
    </span>
  </header>

  <!-- 渲染引导条:角色卡含界面脚本但当前纯文本显示时,给一键开启入口(bug 4 可发现性修复) -->
  <div v-if="renderHintVisible" class="sv-render-hint" role="status">
    <span class="sv-render-hint-text">此角色卡带有界面脚本，当前以纯文本显示，可能看不到状态栏/开场界面。</span>
    <button type="button" class="sv-btn ghost sv-btn-sm" @click="enableRenderFromHint">开启 HTML 渲染</button>
    <button type="button" class="sv-btn ghost sv-btn-sm" title="查看风险后为当前角色卡启用 JavaScript" @click="confirmScriptAuthorization">授权 JS</button>
    <button type="button" class="sv-icon-btn" title="不再提示(仅此角色)" @click="dismissRenderHint">✕</button>
  </div>

  <!-- JS 授权引导条:界面已显示但脚本未授权,交互不会生效(赛马娘卡首楼点击全失效的可发现性缺口) -->
  <div v-if="jsAuthHintVisible" class="sv-render-hint" role="status">
    <span class="sv-render-hint-text">此角色卡的界面脚本尚未授权 JS,界面上的下拉/按钮点击不会生效。</span>
    <button type="button" class="sv-btn ghost sv-btn-sm" title="查看风险后为当前角色卡启用 JavaScript" @click="confirmScriptAuthorization">授权 JS</button>
    <button type="button" class="sv-icon-btn" title="不再提示(仅此角色)" @click="dismissJsAuthHint">✕</button>
  </div>
</template>

<style scoped>
/* UIP-10:角色名超长时截断(truncate/min-w-0 负责收窄与省略号,此处给宽度上限),
   不再把右侧开关组挤出顶栏;全文走 title */
.sv-topbar-identity {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  min-width: 0;
}
.sv-topbar-title { max-width: 320px; }
.sv-topbar-sub { max-width: 160px; }

/* UIPOL-1/A1:顶栏三个渲染开关统一为「裸开关」规格。
   JS 授权原本是带 2px 黑框白底的 .sv-btn(高 34px),与左右两个 22px 的 HTML/AGENT
   开关排在同一行时顶边高 6px、底边低 6px,在一条水平线上形成视觉鼓包(实测 22/34/22)。
   它和 HTML、AGENT 同属渲染管线开关、语义同级,强调应由状态本身承担,不该改变控件形态;
   故拉平到 22px 无边框无底色,状态用前缀方块表达(关闭 = 粉灰、已授权 = 粉深)。
   方块取 10px(.sv-supreme.sm 同档),与顶栏身份区的标识方块同一尺度,不抢 HTML/AGENT 的
   40×22 轨道——那两枚是「开/关」拨杆,这枚是「已授权/未授权」状态灯,视觉上刻意区分。 */
.sv-topbar-render .sv-topbar-js {
  height: 22px;
  min-height: 22px;
  padding: 0;
  border: 0;
  background: transparent;
  box-shadow: none;
  display: inline-flex;
  align-items: center;
  gap: var(--space-2);
  font-size: 11px;
  font-weight: 700;
  letter-spacing: 0.05em;
  color: var(--sv-ink-soft);
  white-space: nowrap;
}
.sv-topbar-render .sv-topbar-js:hover:not(:disabled),
.sv-topbar-render .sv-topbar-js:active:not(:disabled) {
  background: transparent;
  border-color: transparent;
  color: var(--sv-ink);
  box-shadow: none;
  transform: none;
}
.sv-topbar-render .sv-topbar-js-dot {
  width: 10px;
  height: 10px;
  flex: none;
  background: var(--sv-line-strong);
}
.sv-topbar-render .sv-topbar-js-dot.on { background: var(--sv-pink-deep); }

/* 技术矩阵:1×4 发丝格(标签 11 / 值 12,等宽数字);只跨一格边框,不盒中盒 */
.sv-topbar-stats {
  display: flex;
  align-items: stretch;
  margin: 0;
  border: 1px solid var(--sv-line-strong);
  background: var(--sv-white);
}
.sv-topbar-stat {
  min-width: 64px;
  padding: var(--space-1) var(--space-2);
  display: flex;
  flex-direction: column;
  justify-content: center;
  border-right: 1px solid var(--sv-line);
}
.sv-topbar-stat:last-child { border-right: none; }
.sv-topbar-stat dt {
  font-size: var(--text-2xs);
  line-height: 1.2;
  letter-spacing: 0.08em;
  color: var(--sv-ink-faint);
  text-transform: uppercase;
}
.sv-topbar-stat dd {
  margin: var(--space-1) 0 0;
  font-size: var(--text-xs);
  line-height: 1.25;
  font-weight: 700;
  color: var(--sv-ink);
  white-space: nowrap;
}

/* 窄屏收容菜单 */
.sv-topbar-more { display: none; position: relative; }
.sv-topbar-more-btn { font-size: 16px; line-height: 1; }
.sv-topbar-more-menu {
  position: absolute;
  right: 0;
  top: calc(100% + var(--space-2));
  z-index: var(--z-popover);
  width: 248px;
  padding: var(--space-2);
  border: var(--bw) solid var(--sv-ink);
  background: var(--sv-white);
  box-shadow: var(--shadow-hover);
}
.sv-topbar-more-title {
  padding: var(--space-1-5) var(--space-1);
  border-bottom: 1px solid var(--sv-line);
  font-size: var(--text-2xs);
  font-weight: 800;
  letter-spacing: 0.12em;
  color: var(--sv-ink-faint);
  text-transform: uppercase;
}
.sv-topbar-more-title:not(:first-child) { margin-top: var(--space-2); }
.sv-topbar-more-item {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-2);
  width: 100%;
  min-height: var(--control-h-sm);
  padding: var(--space-1) var(--space-1-5);
  border: none;
  background: transparent;
  color: var(--sv-ink);
  font-size: var(--text-xs);
  text-align: left;
  cursor: pointer;
}
.sv-topbar-more-item:hover { background: var(--sv-pink-light); }
.sv-topbar-more-item b { font-weight: 800; color: var(--sv-ink-soft); }
.sv-topbar-stats-menu {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  margin-top: var(--space-1-5);
}
.sv-topbar-stats-menu .sv-topbar-stat {
  border-right: 1px solid var(--sv-line);
  border-bottom: 1px solid var(--sv-line);
  min-width: 0;
}
.sv-topbar-stats-menu .sv-topbar-stat:nth-child(2n) { border-right: none; }

/* <1280:统计与渲染开关收进菜单,顶栏只留身份/会话/Agent */
@media (max-width: 1279px) {
  .sv-topbar-stats,
  .sv-topbar-render { display: none; }
  .sv-topbar-more { display: block; }
}
@media (max-width: 900px) {
  .sv-topbar-sub { display: none; }
  .sv-session-select { max-width: 38vw; }
}
@media (max-width: 767px) {
  .sv-topbar-title { max-width: 42vw; }
}
</style>
