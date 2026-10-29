<script setup lang="ts">
// 聊天窗口:顶栏(ChatTopbar)+ 消息列表(编辑/删除,画布容器 RenderPanelHost)+ 底部输入区
import { ref, nextTick, watch, computed, onMounted, onBeforeUnmount, defineAsyncComponent } from 'vue';
import { storeToRefs } from 'pinia';
import { useAppStore } from '../store';
import { hasStatusPlaceholderScript, buildMessageRenderText } from '../render';
import { installMvuGlobals } from '../mvu/host';
import { ScriptRunner, type SandboxRpcExtensions } from '../scriptRunner';
import { executeCurrentMessageScripts as scheduleCurrentMessageScripts } from '../chatMessageScriptScheduler';
import { enabledCardScriptsOf } from '../cardScripts';
import { cleanupCardScriptSandbox, ensureCardScriptSandbox, makeCardScriptRpcExtensions } from '../cardScriptHost';
import { useActiveThemeId } from '../composables/useActiveTheme';
import { useVirtualMessages } from '../composables/useVirtualMessages';
import { useResourceFrames } from '../composables/useResourceFrames';
import { useUserPersona } from '../composables/useUserPersona';
import { createScriptBlocksRender } from '../scriptBlocksRender';
import { mergedRegexScripts } from '../utils/regexMerge';
import type { UiMessage } from '../sseReducer';
import ChatInput from './ChatInput.vue';
import ChatMessageItem from './ChatMessageItem.vue';
import ChatTopbar from './ChatTopbar.vue';
import GreetingPickerModal from './GreetingPickerModal.vue';
import RenderPanelHost from './RenderPanelHost.vue';
/** 环境层懒加载:shader/WebGL 生命周期不进首屏 index chunk(预算见 check-bundle)。 */
const AmbientField = defineAsyncComponent(() => import('./AmbientField.vue'));

const store = useAppStore();
const activeThemeId = useActiveThemeId();
const { userName } = useUserPersona();
const {
  messages, generating, currentSessionId, currentCharacterId, currentCharacter, currentCharacterName,
  renderHtml, currentScriptHash, currentScriptAuthorized, mvuVariables, initVarEntries, lastError,
  regexScripts, characters, sessions, model, connStatus,
} = storeToRefs(store);

/** 空态快捷角色:最多 3 个,复用列表已有数据,不额外请求 */
const quickStartCharacters = computed(() => characters.value.slice(0, 3));

/** 会话标题(空态 meta 与有消息时的 masthead 共用) */
const currentSessionTitle = computed(
  () => sessions.value.find((s) => s.id === currentSessionId.value)?.title || '当前会话',
);

/** 连接状态中文(与侧栏同一套语义;空态给用户一个可判断的连接事实) */
const connectionLabel = computed(() => {
  switch (connStatus.value) {
    case 'ok': return '已连接';
    case 'fail': return '未连接';
    default: return '检测中';
  }
});

/** 无角色时,导入入口在 Sidebar;空态按钮经事件触发同一 file input(不复制导入逻辑) */
function requestCharacterImport(): void {
  window.dispatchEvent(new Event('kedai:open-character-import'));
}

/** 回到底部浮钮:离底超过 400px 才出现,避免在正常阅读时制造噪音。 */
const showScrollButton = ref(false);
const SCROLL_BOTTOM_THRESHOLD = 400;

function updateScrollButton(): void {
  const el = scrollArea.value;
  if (!el) {
    showScrollButton.value = false;
    return;
  }
  showScrollButton.value = el.scrollHeight - el.scrollTop - el.clientHeight > SCROLL_BOTTOM_THRESHOLD;
}

function scrollToLatest(): void {
  const el = scrollArea.value;
  if (!el) return;
  const reduce = typeof window.matchMedia === 'function'
    && window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  el.scrollTo({ top: el.scrollHeight, behavior: reduce ? 'auto' : 'smooth' });
  showScrollButton.value = false;
}

/** 角色头像 URL(空态快捷卡用;与聊天头像同一路径规则) */
function charAvatar(c: { avatar_path?: string | null }): string | null {
  if (!c.avatar_path) return null;
  return `/api/avatars/${c.avatar_path.split(/[\\/]/).pop()}`;
}

/** 错误条「重试」锚点:最后一条**已落库**用户消息(与服务端重发校验口径一致,见 resendMessage) */
const retryTarget = computed<number | null>(() => {
  for (let i = messages.value.length - 1; i >= 0; i--) {
    const m = messages.value[i];
    if (m.role === 'user' && m.id > 0) return m.id;
  }
  return null;
});

/** 重试上一次失败的生成:重发锚点消息(成功 finish 会清掉错误条) */
function retryLastError(): void {
  if (retryTarget.value === null) return;
  void store.resendMessage(retryTarget.value);
}

/** 消息画布宿主(渲染面板折叠切换与水合的实现组件;el 为原 scrollArea 容器) */
const panelHost = ref<InstanceType<typeof RenderPanelHost> | null>(null);
const scrollArea = computed<HTMLElement | null>(() => panelHost.value?.el ?? null);
/** 脚本执行器(内部含 [InitVar] 解析缓存;角色切换/条目变化自动失效) */
const scriptRunner = new ScriptRunner();

/** 滚动容器挂载/换会话后接线「回到底部」判定;监听在卸载时显式摘除。 */
watch(scrollArea, (el, old) => {
  old?.removeEventListener('scroll', updateScrollButton);
  el?.addEventListener('scroll', updateScrollButton, { passive: true });
  updateScrollButton();
}, { flush: 'post' });

/** 当前角色正则脚本 + 全局正则库(显示层生效;合并顺序:全局在前、卡级在后,
 *  见 utils/regexMerge.ts。全局库变化会同时改 currentScriptHash 使渲染缓存失效) */
const currentScripts = computed(() =>
  mergedRegexScripts(regexScripts.value, currentCharacter.value?.regex_scripts ?? []),
);

/** 角色卡是否含渲染 HTML 状态栏的脚本(命中 <StatusPlaceHolderImpl/> 占位符) */
const hasStatusScript = computed(() => hasStatusPlaceholderScript(currentScripts.value));

/** 消息显示文本:优先服务端宏展开后的 content_display({{char}}/{{getvar}} 等已展开),
 *  缺省回退原文。编辑消息时仍用 content(原文),避免污染存储。 */
function displayText(m: { content: string; content_display?: string }): string {
  return m.content_display ?? m.content;
}

/** 消息状态栏文本(两步生成落库 extra.status_bar;无则 null) */
function statusBar(m: { extra?: Record<string, unknown> }): string | null {
  const bar = m.extra?.status_bar;
  return typeof bar === 'string' && bar.trim() ? bar : null;
}

/**
 * 消息渲染文本:正文 + 状态栏占位符。
 * 带状态栏(extra.status_bar)且角色卡存在 HTML 状态栏脚本时,追加 <StatusPlaceHolderImpl/>
 * 使状态栏被「状态栏」正则脚本识别,渲染为 HTML 卡片(脚本用变量树填充动态数据);
 * 否则原样返回正文(状态栏走纯文本气泡)。渲染与脚本执行共用,保证两者看到的文本一致。
 */
function renderTextFor(m: { id: number; content: string; content_display?: string; extra?: Record<string, unknown> }): string {
  return buildMessageRenderText(displayText(m), statusBar(m), hasStatusScript.value);
}

// 脚本块解析缓存(流式性能优化;实现与键纪律见 scriptBlocksRender.ts,USERPERSONA-1 起下沉)
const renderScriptsCached = createScriptBlocksRender({
  scriptHash: () => currentScriptHash.value,
  charName: () => currentCharacter.value?.chara_name ?? currentCharacter.value?.name ?? '',
  userName: () => userName.value,
});

// 消息级沙箱只读 RPC 扩展(世界书读取 / 聊天消息读取),供首楼脚本获取聊天上下文。
// 实跑问题 7 主因:此前不注入,数据 RPC 落到 DOM 白名单分发抛错 → 沙箱 cleanup
// 摘除全部事件监听。复用卡级沙箱同一实现(opts 与卡级一致:一角色一内嵌世界书 +
// 当前消息列表),不扩大写权限(消息级仍无世界书写入口)。
function messageRpcExtensions(): SandboxRpcExtensions | undefined {
  const id = currentCharacter.value?.id;
  if (!id) return undefined;
  const book = (currentCharacter.value?.data_raw as Record<string, unknown> | undefined)
    ?.character_book as { name?: unknown } | undefined;
  const lorebookName =
    typeof book?.name === 'string' && book.name.trim() ? book.name : null;
  return makeCardScriptRpcExtensions({
    characterId: id,
    lorebookName,
    readMessages: () => messages.value,
  });
}

// 脚本执行:非流式 assistant 消息渲染完成后,按 scopeId 定位容器并执行脚本
async function executeCurrentMessageScripts(): Promise<void> {
  await scheduleCurrentMessageScripts({
    renderHtml: renderHtml.value,
    authorized: currentScriptAuthorized.value,
    scripts: currentScripts.value,
    messages: messages.value,
    characterId: currentCharacter.value?.id ?? '',
    scriptHash: currentScriptHash.value,
    initVarEntries: initVarEntries.value,
    mvuVariables: mvuVariables.value,
    charName: currentCharacter.value?.chara_name ?? currentCharacter.value?.name ?? '',
    userName: userName.value,
    rpcExtensions: messageRpcExtensions(),
    isAuthorized: store.isCharacterScriptAuthorized,
    getScrollArea: () => scrollArea.value,
    afterRender: nextTick,
    renderText: renderTextFor,
    depthOf: (_m, i, total) => total - 1 - i,
    renderScripts: renderScriptsCached,
    runMessageScripts: (...args) => scriptRunner.runMessageScripts(...args),
  });
}

// 远程资源卡片宿主(扫描/投递/存储同步/TavernHelper RPC 桥,实现见 composable);
// 宽屏按钮点击委托由 composable 挂载时自行接线,window message 监听下方显式接线
const frames = useResourceFrames({ scrollArea, currentCharacter, currentCharacterId });

/** 渲染面板投递(实现已迁入 RenderPanelHost;此处保留薄封装,原调用点不动) */
async function hydrateRenderPanels(): Promise<void> {
  await panelHost.value?.hydrate();
}

onMounted(async () => {
  installMvuGlobals();
  // 全局正则库(IMPHUB ⑥):幂等确保已加载(选角链路通常已拉过;此处兜底,失败不挡渲染)
  void store.ensureRegexScriptsLoaded();
  // 资源框架常驻消息监听(ready 重 boot / store-sync 持久化),生命周期与组件一致
  window.addEventListener('message', frames.handleMessage);
  await executeCurrentMessageScripts();
  // 初始消息(会话历史)可能在 watch 建立前已渲染,资源卡片 iframe 需要显式补一次
  await frames.hydrate();
  // 渲染面板(TH-render 等价物)同理由 watch 驱动,初始历史补一次
  await hydrateRenderPanels();
  // 首屏历史可能在 watch 建立前已加载:显式补一次「定位到最后一条」
  await scrollToBottom();
  // 卡级脚本长驻沙箱首次同步(watch 无 immediate:此时 scrollArea 已挂载,
  // 重进恢复的角色若已授权,swipe 世界书联动等卡级脚本从这里起跑)
  void syncCardScriptSandbox();
});

onBeforeUnmount(() => {
  window.removeEventListener('message', frames.handleMessage);
  scrollArea.value?.removeEventListener('scroll', updateScrollButton);
  scriptRunner.cleanup();
  cleanupCardScriptSandbox();
});

/**
 * 消息列表变更纪元:中部消息被编辑保存 / swipe 切换(经子组件 mutated 事件)时 +1。
 * O(1) 签名只跟踪尾部消息,中部内容变更借纪元显式触发脚本重跑与滚动,语义与原全量签名一致。
 */
const listEpoch = ref(0);

/** 编辑中的消息 id(单编辑语义;编辑文本由子组件本地管理,击键不触发列表重渲染) */
const editingId = ref<number | null>(null);

/** 滚到最后一条:等 DOM/虚拟行高度稳定后再补一帧(首屏/切会话时 watch 可能早于布局)。 */
async function scrollToBottom(): Promise<void> {
  await nextTick();
  const el = scrollArea.value;
  if (!el) return;
  // 空态不是消息流:它需要从顶部读起。旧实现无条件吸底,移动端会把标题/引导滚出视口。
  if (messages.value.length === 0) {
    el.scrollTop = 0;
    showScrollButton.value = false;
    return;
  }
  el.scrollTop = el.scrollHeight;
  showScrollButton.value = false;
  requestAnimationFrame(() => {
    if (scrollArea.value) scrollArea.value.scrollTop = scrollArea.value.scrollHeight;
  });
}

// 消息变化时自动滚到底部
// 签名 O(1):仅读「长度 + 最后一条消息 + 生成态 + 编辑态 + 纪元」,
// 替代原 messages.map(displayText).join('|') 的全量遍历(流式每 token 拼全量字符串)。
watch(
  () => {
    const arr = messages.value;
    const last = arr[arr.length - 1];
    return [
      arr.length,
      last?.id ?? 0,
      last?.streaming ? 1 : 0,
      typeof last?.extra?.swipe_id === 'number' ? last.extra.swipe_id : -1,
      last ? displayText(last) : '',
      generating.value,
      editingId.value ?? 0,
      listEpoch.value,
    ].join('\u0001');
  },
  async () => {
    await scrollToBottom();
  },
);

// 切会话:历史尚未到达时先滚一次,消息到达后上面的 watch 再补一次
watch(currentSessionId, async () => {
  await scrollToBottom();
});

// 渲染后副作用(脚本执行 + 资源卡片/渲染面板投递)
// 签名同样 O(1):最后一条消息渲染文本 + HTML 开关 + 脚本版本(hash/长度/授权)+ 编辑态 + 纪元;
// 历史消息内容不可变(编辑/swipe 走纪元),脚本列表变化经 hash/长度捕获。
watch(
  () => {
    const arr = messages.value;
    const last = arr[arr.length - 1];
    return [
      arr.length,
      last?.id ?? 0,
      last?.streaming ? 1 : 0,
      last ? renderTextFor(last) : '',
      renderHtml.value,
      currentScripts.value.length,
      currentScriptHash.value,
      currentScriptAuthorized.value,
      editingId.value ?? 0,
      listEpoch.value,
    ].join('\u0001');
  },
  async () => {
    await nextTick();
    await Promise.allSettled([executeCurrentMessageScripts(), frames.hydrate(), hydrateRenderPanels()]);
  },
  { flush: 'post', immediate: true },
);

// 卡级脚本(角色卡内嵌酒馆助手脚本)长驻沙箱:随角色选中常驻,监听 swipe 事件流、
// 读写内嵌世界书(舰娘卡「随开场白切换世界书」)。只过 JS 授权门禁,不要求 renderHtml
// 开关(它不是渲染)。键 = characterId:scriptHash(哈希含卡级脚本正文):换卡/卡更新
// 脚本/授权变化→重建或销毁,由 ensureCardScriptSandbox 内部幂等处理。
async function syncCardScriptSandbox(force = false): Promise<void> {
  const id = currentCharacterId.value;
  const authorized = currentScriptAuthorized.value;
  const hash = currentScriptHash.value;
  if (!id || !authorized || !hash) {
    cleanupCardScriptSandbox();
    return;
  }
  // 列表项无 data_raw(体积大):卡级脚本与主世界书名必须读详情缓存
  const detail = await store.fetchCharacterDetail(id);
  // await 期间可能已切卡/撤授权/哈希升级:以最新值为准,失效则放弃本次启动
  if (currentCharacterId.value !== id || !currentScriptAuthorized.value || currentScriptHash.value !== hash) return;
  const scripts = enabledCardScriptsOf(detail);
  if (scripts.length === 0 || !scrollArea.value) {
    cleanupCardScriptSandbox();
    return;
  }
  const book = (detail?.data_raw as Record<string, unknown> | undefined)?.character_book as
    | { name?: unknown }
    | undefined;
  await ensureCardScriptSandbox({
    characterId: id,
    scriptHash: hash,
    scripts,
    container: scrollArea.value,
    lorebookName: typeof book?.name === 'string' && book.name.trim() ? book.name : null,
    readMessages: () => messages.value,
    force,
  });
}
watch([currentCharacterId, currentScriptAuthorized, currentScriptHash], () => {
  void syncCardScriptSandbox();
});

// 实跑问题 7 R5:卡级脚本常在 boot 时扫描聊天历史注入界面。历史尚未就绪时启动会
// 读到空列表且不再补跑 → 首屏空。这里在「历史首次变为非空」时为当前会话强制重建一次;
// 会话切换后重置闩锁,下一次历史就绪同样补跑。
let cardSandboxHistoryReady = false;
watch(
  () => `${currentSessionId.value ?? ''}\u0001${messages.value.length}`,
  () => {
    const hasHistory = messages.value.length > 0;
    if (!hasHistory) {
      cardSandboxHistoryReady = false;
      return;
    }
    if (cardSandboxHistoryReady) return;
    cardSandboxHistoryReady = true;
    void syncCardScriptSandbox(true);
  },
);

// ===== 多开场切换 =====
/** 开场选择器:null=关闭;mode='new'=新建会话时选择开场,'switch'=当前会话内重置开场 */
const greetingPickerOpen = ref(false);
const greetingPickerMode = ref<'new' | 'switch'>('new');

/** 打开开场选择器(调用方已保证 currentGreetings.length > 1) */
function openGreetingPicker(mode: 'new' | 'switch'): void {
  if (generating.value) return;
  greetingPickerMode.value = mode;
  greetingPickerOpen.value = true;
}

/** 头像来源(优先角色头像) */
const avatarUrl = computed(() => {
  const c = currentCharacter.value;
  if (!c?.avatar_path) return null;
  return `/api/avatars/${c.avatar_path.split(/[\\/]/).pop()}`;
});

// assistant 消息 HTML 渲染逻辑已迁入 ChatMessageItem(按消息 computed 缓存);
// 本组件仅保留列表编排、脚本调度与滚动行为。

/** 进入编辑态(编辑文本在 ChatMessageItem 本地维护,进入时用消息原文初始化) */
function startEdit(m: UiMessage): void {
  editingId.value = m.id;
}

/** 保存编辑:resend=true 时保存并丢弃其后所有消息重新生成;成功后推进列表纪元(触发脚本重跑) */
async function saveEdit(payload: { id: number; text: string; resend: boolean }): Promise<void> {
  if (!currentSessionId.value) return;
  try {
    if (payload.resend) {
      await store.resendMessage(payload.id, payload.text);
    } else {
      await store.updateMessage(payload.id, payload.text);
    }
  } catch (err) {
    alert(`编辑失败:${(err as Error).message}`);
    return;
  }
  editingId.value = null;
  listEpoch.value += 1;
}

// 切换会话/角色时清空消息编辑草稿:ChatWindow 常驻挂载,若不清除,
// 旧草稿会残留到新会话并误保存到别的角色/会话(「草稿未正确削除」)。
watch(
  [currentSessionId, currentCharacterId],
  () => {
    editingId.value = null;
  },
);

/**
 * 消息列表虚拟滚动(轻量实现,无依赖):长会话(>80 条)时视口缓冲区外的消息
 * 渲染为等高占位 div(实测高度缓存/角色估算),进入缓冲区再真实挂载;
 * 尾部 30 条常驻真实挂载,流式自动吸底行为不变。行挂载状态变化后补跑
 * 脚本执行与资源/面板水合(新挂载的消息可能含脚本容器或 iframe)。
 */
const vm = useVirtualMessages({
  messages,
  scrollArea,
  pinnedId: editingId,
  resetKey: currentSessionId,
  onRowsChanged: () => {
    void Promise.allSettled([executeCurrentMessageScripts(), frames.hydrate(), hydrateRenderPanels()]);
  },
});

// 模板 ref 回调参数被 Vue 推断为 Element | ComponentPublicInstance | null,
// 与虚拟滚动 bindRow 期望的 RowRefTarget(HTMLElement 或带 rootEl 的组件暴露,
// 未导出)不直接兼容;此处仅做类型收窄,运行时透传(resolveRowEl 内部读 rootEl)。
type VmRowTarget = Parameters<typeof vm.bindRow>[1];
function bindRowRef(id: number, target: unknown): void {
  vm.bindRow(id, target as VmRowTarget);
}
</script>

<template>
  <section class="flex min-h-0 min-w-0 flex-1 flex-col">
    <!-- 顶栏(含渲染引导条):会话/开场/渲染开关/JS 授权/Agent 面板/token 统计 -->
    <ChatTopbar @open-greeting-picker="openGreetingPicker" />

    <!-- 舞台:环境层在画布背后,消息/空态在实色表面上(z-index 1)。
         彩窗主题改用 App 级整窗环境层,这里不再挂第二张 canvas。 -->
    <div class="sv-stage">
      <AmbientField
        v-if="activeThemeId !== 'stained-glass'"
        :variant="'paper'"
        :intensity="messages.length ? 0.22 : 0.85"
        :paused="generating || store.overlayOpen"
      />

      <!-- 消息画布(渲染面板折叠切换与水合由宿主组件承载) -->
      <RenderPanelHost ref="panelHost" transparent>
        <div v-if="messages.length === 0" class="sv-start">
          <div class="sv-start-grid">
            <div class="sv-start-copy">
              <div class="sv-start-kicker">
                <span class="sq" />角色扮演<span class="sv-start-kicker-en">ROLEPLAY</span>
              </div>
              <h2 class="sv-start-title">
                {{ currentCharacter ? `和 ${currentCharacterName} 开始对话` : '先选一个角色' }}
              </h2>
              <p class="sv-start-desc">
                {{ currentCharacter
                  ? '输入消息开始，或先确认连接与模型。左侧可随时切换角色与模式。'
                  : '从左侧角色列表选择，或点击「导入角色卡」开始。' }}
              </p>

              <dl class="sv-start-meta" aria-label="当前连接与会话">
                <div class="sv-start-meta-item">
                  <dt>模型</dt>
                  <dd>{{ model || '未配置' }}</dd>
                </div>
                <div class="sv-start-meta-item">
                  <dt>连接</dt>
                  <dd>{{ connectionLabel }}</dd>
                </div>
                <div class="sv-start-meta-item">
                  <dt>会话</dt>
                  <dd class="sv-tnum">{{ sessions.length }} 个</dd>
                </div>
              </dl>

              <div class="sv-start-actions">
                <button
                  v-if="!characters.length"
                  type="button"
                  class="sv-btn primary sv-btn-sm"
                  @click="requestCharacterImport"
                >导入角色卡</button>
                <button
                  v-else
                  type="button"
                  class="sv-btn primary sv-btn-sm"
                  @click="store.settingsOpen = true"
                >连接与模型</button>
                <button
                  type="button"
                  class="sv-btn ghost sv-btn-sm"
                  :disabled="!currentCharacterId"
                  @click="store.newSession()"
                >新建会话</button>
              </div>
              <p class="sv-start-hint">输入含算式（如「帮我算 12*34」）时，Agent 会自动调用计算器工具。</p>
            </div>

            <aside class="sv-start-cast" aria-labelledby="sv-cast-head">
              <div id="sv-cast-head" class="sv-start-cast-head">
                <span>角色索引</span>
                <span class="sv-start-cast-count sv-tnum">
                  {{ quickStartCharacters.length.toString().padStart(2, '0') }} /
                  {{ characters.length.toString().padStart(2, '0') }}
                </span>
              </div>
              <ol v-if="quickStartCharacters.length" class="sv-start-chars">
                <li v-for="(c, i) in quickStartCharacters" :key="c.id">
                  <button
                    type="button"
                    class="sv-start-char"
                    :class="{ active: c.id === currentCharacterId }"
                    @click="store.selectCharacter(c.id)"
                  >
                    <span class="sv-start-char-idx sv-tnum">{{ (i + 1).toString().padStart(2, '0') }}</span>
                    <span class="sv-avatar small char">
                      <img v-if="charAvatar(c)" :src="charAvatar(c)!" alt="" />
                      <template v-else>{{ c.chara_name.charAt(0) }}</template>
                    </span>
                    <span class="sv-start-char-body">
                      <span class="sv-start-char-name">{{ c.chara_name }}</span>
                      <span class="sv-start-char-desc">{{ c.description || '无提示词' }}</span>
                    </span>
                    <span class="sv-start-char-arrow" aria-hidden="true">→</span>
                  </button>
                </li>
              </ol>
              <div v-else class="sv-start-cast-empty">
                <span class="sv-start-cast-mark" aria-hidden="true" />
                <p>还没有角色卡</p>
                <p>从左侧「导入角色卡」开始，或点上方按钮选择文件。</p>
              </div>
              <p class="sv-start-cast-note">选择角色后，开场白会自动进入对话。</p>
            </aside>
          </div>
        </div>

        <!-- 有消息时的对话 masthead:角色 / 模型 / 会话 / 条数 -->
        <div v-else class="sv-chat-masthead">
          <div class="sv-chat-masthead-main">
            <span class="sv-chat-masthead-kicker">角色扮演</span>
            <h2 class="sv-chat-masthead-title">{{ currentCharacterName }}</h2>
          </div>
          <div class="sv-chat-masthead-meta">
            <span>{{ model || '未配置模型' }}</span>
            <span>{{ currentSessionTitle }}</span>
            <span class="sv-tnum">{{ messages.length }} 条消息</span>
          </div>
        </div>

        <!-- 消息列表(渲染缓存:每条消息独立子组件,流式仅重算当前消息;
             props 未变的子组件在父重渲染时自动跳过;长会话虚拟滚动:缓冲区外渲染等高占位) -->
        <template v-for="(m, i) in messages" :key="m.id">
          <div
            v-if="!vm.isActive(m.id, i)"
            :ref="(el) => bindRowRef(m.id, el)"
            :style="vm.placeholderStyle(m)"
          ></div>
          <ChatMessageItem
            v-else
            :ref="(inst) => bindRowRef(m.id, inst)"
            :m="m"
            :editing="editingId === m.id"
            :avatar-url="avatarUrl"
            :character-name="currentCharacterName"
            :render-html="renderHtml"
            :scripts="currentScripts"
            :script-hash="currentScriptHash"
            :depth="messages.length - 1 - i"
            @start-edit="startEdit"
            @save-edit="saveEdit"
            @cancel-edit="editingId = null"
            @mutated="listEpoch += 1"
          />
        </template>

        <!-- 思考中占位 -->
        <div v-if="generating" class="sv-msg assistant">
          <span class="sv-avatar char">
            <template v-if="avatarUrl"><img :src="avatarUrl" alt="" /></template>
            <template v-else>{{ currentCharacterName.charAt(0) }}</template>
          </span>
          <div>
            <div class="sv-msg-name">{{ currentCharacterName }}</div>
            <div class="sv-msg-bubble thinking">
              <span class="sv-thinking"><i /><i /><i /></span>
            </div>
          </div>
        </div>
      </RenderPanelHost>

      <button
        v-if="showScrollButton && messages.length"
        type="button"
        class="sv-scroll-bottom"
        aria-label="回到最新消息"
        title="回到最新消息"
        @click="scrollToLatest"
      >
        <span aria-hidden="true">↓</span><span>最新</span>
      </button>
    </div>

    <!-- 发送/生成失败条(SENDFIX-1):错误必须可见——旧版错误只写进 Agent 面板且随即被
         历史回拉擦除,界面表现为「消息发出后什么都没发生」;此处为对话区内的常驻可见层 -->
    <div v-if="lastError" class="sv-feedback err sv-chat-error" role="alert">
      <span class="sv-chat-error-text">
        <b>{{ lastError.rejected ? '发送失败' : '生成失败' }}</b>
        <span class="sv-chat-error-msg">{{ lastError.message }}</span>
        <span v-if="lastError.code" class="sv-chat-error-code">({{ lastError.code }})</span>
      </span>
      <button
        v-if="!lastError.rejected && lastError.retryable && retryTarget !== null"
        type="button"
        class="sv-btn primary sv-btn-sm"
        :disabled="generating || !currentSessionId"
        title="以同一条用户消息重新生成(上游限流/超时/网络类错误适用)"
        @click="retryLastError()"
      >重试</button>
      <span v-else-if="lastError.rejected" class="sv-chat-error-hint">输入框已保留原文，可直接重新发送</span>
      <button
        type="button"
        class="sv-icon-btn sv-chat-error-close"
        aria-label="关闭错误提示"
        title="关闭"
        @click="store.dismissLastError()"
      >✕</button>
    </div>

    <!-- 底部输入区 -->
    <ChatInput />

    <!-- 开场选择器(多开场角色:新建会话选择 / 会话内切换) -->
    <GreetingPickerModal v-if="greetingPickerOpen" :mode="greetingPickerMode" @close="greetingPickerOpen = false" />
  </section>
</template>

<style scoped>
/* 发送/生成失败条(SENDFIX-1):对话区内的常驻可见层;间距走 --space-* 令牌(UIP-10 纪律)。
   仅本组件作用域覆盖 .sv-feedback 的外边距(margin-top 来自全局域,此处按对话区版式改写)。 */
.sv-chat-error {
  flex-wrap: wrap;
  margin: 0 var(--space-3) var(--space-2);
}
.sv-chat-error-text {
  display: flex;
  align-items: baseline;
  flex: 1;
  min-width: 0;
  gap: var(--space-1-5);
}
.sv-chat-error-msg { word-break: break-word; }
.sv-chat-error-code { font-size: 11px; opacity: 0.7; }
.sv-chat-error-hint { font-size: 11px; opacity: 0.85; }
.sv-chat-error-close { flex: none; }

/* ===== 舞台与环境层 ===== */
.sv-stage {
  position: relative;
  flex: 1;
  min-height: 0;
  display: flex;
  overflow: hidden;
  background-color: var(--sv-paper);
  background-image:
    radial-gradient(72% 56% at 80% 10%, color-mix(in srgb, var(--sv-pink) 12%, transparent), transparent 72%),
    radial-gradient(58% 48% at 14% 92%, color-mix(in srgb, var(--sv-blue) 4%, transparent), transparent 76%);
}
.sv-stage :deep(.sv-canvas) {
  position: relative;
  z-index: 1;
}

/* 回到底部:离底 >400px 才出现;不参与阅读路径,只在需要时给一个明确出口 */
.sv-scroll-bottom {
  position: absolute;
  right: clamp(16px, 3vw, 32px);
  bottom: var(--space-4);
  z-index: 3;
  display: inline-flex;
  align-items: center;
  gap: var(--space-1-5);
  min-height: var(--control-h-sm);
  padding: 0 var(--space-3);
  border: var(--bw) solid var(--sv-ink);
  background: var(--sv-white);
  color: var(--sv-ink);
  box-shadow: var(--shadow-card);
  font-size: var(--text-xs);
  font-weight: 800;
  letter-spacing: 0.06em;
  cursor: pointer;
}
.sv-scroll-bottom:hover {
  background: var(--sv-pink-light);
  box-shadow: var(--shadow-hover);
}
.sv-scroll-bottom:active {
  box-shadow: var(--shadow-press);
  transform: translate(2px, 2px);
}

/* ===== 空态:两栏「编辑部开场」 =====
   左栏是介绍与下一步,右栏是角色索引;顶线 2px + 编号 + 箭头,
   不再堆叠彩块/盒中盒。环境层从背后透出,文字仍在白场实色上。 */
.sv-start {
  position: relative;
  max-width: 1060px;
  margin: clamp(var(--space-8), 8vh, calc(var(--space-8) * 3)) auto 0;
  padding: 0 0 var(--space-5);
  border-top: var(--bw) solid var(--sv-ink);
  background: transparent;
  box-shadow: none;
}
.sv-start::before {
  content: '';
  position: absolute;
  right: 6px;
  top: 26px;
  width: 18px;
  height: 18px;
  background: var(--sv-ink-supreme);
}
.sv-start-grid {
  display: grid;
  grid-template-columns: minmax(0, 1.1fr) minmax(300px, 0.9fr);
  gap: clamp(var(--space-8), 5vw, calc(var(--space-8) * 2));
  align-items: start;
}
.sv-start-copy { min-width: 0; }
.sv-start-kicker {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  margin-top: var(--space-3);
  font-size: var(--text-2xs);
  letter-spacing: 0.18em;
  color: var(--sv-ink-dim);
  text-transform: uppercase;
}
.sv-start-kicker .sq { width: 10px; height: 10px; background: var(--sv-red); display: inline-block; flex: none; }
.sv-start-kicker-en { margin-left: var(--space-1); color: var(--sv-ink-faint); letter-spacing: 0.2em; }
.sv-start-title {
  max-width: 16ch;
  margin: var(--space-4) 0 var(--space-3);
  font-family: var(--font-display);
  font-size: clamp(28px, 3.4vw, 38px);
  line-height: 1.12;
  letter-spacing: -0.01em;
}
.sv-start-desc {
  max-width: 52ch;
  margin: 0;
  font-size: var(--text-base);
  line-height: var(--lh-prose);
  color: var(--sv-ink-dim);
}
.sv-start-meta {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  margin: var(--space-5) 0 0;
  border-top: 1px solid var(--sv-line-strong);
  border-bottom: 1px solid var(--sv-line-strong);
}
.sv-start-meta-item {
  min-width: 0;
  padding: var(--space-2-5) var(--space-3) var(--space-2-5) 0;
  border-right: 1px solid var(--sv-line);
}
.sv-start-meta-item:last-child { border-right: none; padding-right: 0; }
.sv-start-meta dt {
  font-size: var(--text-2xs);
  letter-spacing: 0.14em;
  color: var(--sv-ink-faint);
  text-transform: uppercase;
}
.sv-start-meta dd {
  margin: var(--space-1) 0 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: var(--text-sm);
  font-weight: 700;
  color: var(--sv-ink);
}
.sv-start-actions { display: flex; flex-wrap: wrap; gap: var(--space-2); margin-top: var(--space-5); }
.sv-start-hint {
  max-width: 52ch;
  margin: var(--space-4) 0 0;
  padding-top: var(--space-2-5);
  border-top: 1px solid var(--sv-line);
  font-size: var(--text-xs);
  line-height: var(--lh-body);
  color: var(--sv-ink-faint);
}

/* 右栏:角色索引(列表行,不是三张盒子卡) */
.sv-start-cast {
  min-width: 0;
  border-top: var(--bw) solid var(--sv-ink);
  padding-top: var(--space-2-5);
}
.sv-start-cast-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-3);
  padding-bottom: var(--space-2-5);
  border-bottom: 1px solid var(--sv-line-strong);
  font-size: var(--text-2xs);
  font-weight: 800;
  letter-spacing: 0.14em;
  color: var(--sv-ink-soft);
  text-transform: uppercase;
}
.sv-start-cast-count { color: var(--sv-ink-faint); font-variant-numeric: tabular-nums; }
.sv-start-chars { list-style: none; margin: 0; padding: 0; }
.sv-start-chars li { margin: 0; }
.sv-start-char {
  display: grid;
  grid-template-columns: 26px 36px minmax(0, 1fr) 16px;
  align-items: center;
  gap: var(--space-3);
  width: 100%;
  min-height: 62px;
  padding: var(--space-3) 0;
  border: none;
  border-bottom: 1px solid var(--sv-line);
  background: transparent;
  color: var(--sv-ink);
  text-align: left;
  cursor: pointer;
  transition: background var(--dur-fast) var(--ease-standard), box-shadow var(--dur-fast) var(--ease-standard);
}
.sv-start-char:hover {
  background: var(--sv-white);
  box-shadow: inset 2px 0 0 var(--sv-ink);
}
.sv-start-char.active {
  background: var(--sv-pink-light);
  box-shadow: inset 2px 0 0 var(--sv-ink);
}
.sv-start-char-idx {
  align-self: start;
  padding-top: var(--space-1);
  font-family: var(--font-display);
  font-size: var(--text-2xs);
  color: var(--sv-ink-faint);
}
.sv-start-char .sv-avatar { width: 36px; height: 36px; }
.sv-start-char-body { display: flex; flex-direction: column; gap: var(--space-1); min-width: 0; }
.sv-start-char-name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: var(--text-sm);
  font-weight: 800;
}
.sv-start-char-desc {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: var(--text-xs);
  color: var(--sv-ink-faint);
}
.sv-start-char-arrow {
  font-size: var(--text-base);
  color: var(--sv-ink-faint);
  transition: transform var(--dur-fast) var(--ease-standard), color var(--dur-fast) var(--ease-standard);
}
.sv-start-char:hover .sv-start-char-arrow { transform: translateX(3px); color: var(--sv-ink); }
.sv-start-cast-empty {
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
  padding: var(--space-5) 0;
  color: var(--sv-ink-faint);
}
.sv-start-cast-empty p { margin: 0; font-size: var(--text-sm); }
.sv-start-cast-empty p:first-of-type { color: var(--sv-ink); font-weight: 700; }
.sv-start-cast-mark {
  position: relative;
  width: 18px;
  height: 18px;
  margin-bottom: var(--space-2);
  background: var(--sv-ink-supreme);
}
.sv-start-cast-mark::after {
  content: '';
  position: absolute;
  left: 12px;
  top: 14px;
  width: 26px;
  height: 3px;
  background: var(--sv-red);
  transform: rotate(-38deg);
  transform-origin: left center;
}
.sv-start-cast-note {
  margin: var(--space-3) 0 0;
  font-size: var(--text-xs);
  line-height: var(--lh-body);
  color: var(--sv-ink-faint);
}

/* 有消息:masthead 给对话一个清晰的章节起点 */
.sv-chat-masthead {
  display: flex;
  align-items: flex-end;
  justify-content: space-between;
  gap: var(--space-4);
  max-width: var(--measure-chat);
  margin: 0 auto var(--space-6);
  padding-bottom: var(--space-3);
  border-bottom: var(--bw) solid var(--sv-ink);
}
.sv-chat-masthead-kicker {
  font-size: var(--text-2xs);
  letter-spacing: 0.18em;
  color: var(--sv-ink-faint);
  text-transform: uppercase;
}
.sv-chat-masthead-title {
  margin: var(--space-1) 0 0;
  font-family: var(--font-display);
  font-size: var(--text-lg);
  line-height: var(--lh-tight);
}
.sv-chat-masthead-meta {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  justify-content: flex-end;
  gap: var(--space-2);
  font-size: var(--text-xs);
  color: var(--sv-ink-faint);
}
.sv-chat-masthead-meta span + span {
  padding-left: var(--space-2);
  border-left: 1px solid var(--sv-line);
}

@media (max-width: 1099px) {
  .sv-start-grid { grid-template-columns: minmax(0, 1fr); gap: var(--space-6); }
  .sv-start-cast { max-width: 560px; }
}
@media (max-width: 900px) {
  .sv-start::before { display: none; }
}
@media (max-width: 767px) {
  .sv-start { margin: var(--space-4) var(--space-3) 0; padding: 0 0 var(--space-4); }
  .sv-start-title { font-size: var(--text-xl); }
  .sv-start { margin-top: var(--space-2); }
  .sv-start-title { margin: var(--space-2) 0 var(--space-2); }
  .sv-start-meta { grid-template-columns: repeat(3, minmax(0, 1fr)); margin-top: var(--space-4); }
  .sv-start-meta-item { padding: var(--space-1-5) var(--space-2) var(--space-1-5) 0; }
  .sv-start-meta-item:nth-child(2) { border-right: 1px solid var(--sv-line); padding-right: var(--space-2); }
  .sv-start-meta-item:nth-child(3) { grid-column: auto; border-top: none; }
  .sv-start-desc { line-height: var(--lh-body); }
  .sv-start-hint { margin-top: var(--space-2-5); padding-top: var(--space-1-5); }
  .sv-start-cast { padding-top: var(--space-1-5); }
  .sv-chat-masthead { align-items: flex-start; flex-direction: column; gap: var(--space-2); }
  .sv-chat-masthead-meta { justify-content: flex-start; }
}
</style>
