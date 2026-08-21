<script setup lang="ts">
// 聊天窗口:会话切换条 + 消息列表(编辑/删除)+ 底部输入区
import { ref, nextTick, watch, computed, onMounted, onBeforeUnmount } from 'vue';
import { storeToRefs } from 'pinia';
import { useAppStore } from '../store';
import { renderScopedScripts, hasStatusPlaceholderScript, buildMessageRenderText } from '../render';
import { installMvuGlobals } from '../mvu/host';
import { ScriptRunner } from '../scriptRunner';
import { executeCurrentMessageScripts as scheduleCurrentMessageScripts } from '../chatMessageScriptScheduler';
import { buildRenderDocument, RENDER_PANEL_CHANNEL } from '../renderPanel';
import { authorizedFetch, BASE } from '../api/client';
import { computeHitRate } from '../contextStats';
import { useVirtualMessages } from '../composables/useVirtualMessages';
import type { UiMessage } from '../sseReducer';
import ChatInput from './ChatInput.vue';
import ChatMessageItem from './ChatMessageItem.vue';

const store = useAppStore();
const {
  messages, generating, model, currentSessionId, currentCharacterId, sessions, currentCharacter, currentCharacterName,
  currentGreetings, renderHtml, currentScriptHash, currentScriptAuthorized, mvuVariables, initVarEntries,
} = storeToRefs(store);

const scrollArea = ref<HTMLElement | null>(null);
/** 脚本执行器(内部含 [InitVar] 解析缓存;角色切换/条目变化自动失效) */
const scriptRunner = new ScriptRunner();

/** 当前角色正则脚本(用于 HTML 渲染) */
const currentScripts = computed(() => currentCharacter.value?.regex_scripts ?? []);

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

/**
 * 脚本块解析缓存(流式性能优化):调度器触发时会为每条历史 assistant 消息执行
 * renderScopedScripts(全量正则 + sanitize,重活),流式期间每个 token 触发一次,
 * 历史消息解析结果却不变。按 (scopeId + scriptHash + 渲染文本) 记忆结果,
 * 历史消息零重算;消息数组被替换(会话切换/刷新历史)或脚本数组引用变化时整体失效。
 */
let scriptBlocksCache = new Map<string, ReturnType<typeof renderScopedScripts>>();
let scriptBlocksOwner: unknown = null;
let scriptBlocksScripts: unknown = null;

function renderScriptsCached(
  text: string,
  scripts: Parameters<typeof renderScopedScripts>[1],
  scopeId: string,
): ReturnType<typeof renderScopedScripts> {
  if (scriptBlocksOwner !== messages.value || scriptBlocksScripts !== scripts) {
    scriptBlocksCache = new Map();
    scriptBlocksOwner = messages.value;
    scriptBlocksScripts = scripts;
  }
  const key = `${scopeId}\n${currentScriptHash.value}\n${text}`;
  const hit = scriptBlocksCache.get(key);
  if (hit !== undefined || scriptBlocksCache.has(key)) return hit ?? null;
  const scoped = renderScopedScripts(text, scripts, scopeId);
  scriptBlocksCache.set(key, scoped);
  return scoped;
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
    isAuthorized: store.isCharacterScriptAuthorized,
    getScrollArea: () => scrollArea.value,
    afterRender: nextTick,
    renderText: renderTextFor,
    renderScripts: renderScriptsCached,
    runMessageScripts: (...args) => scriptRunner.runMessageScripts(...args),
  });
}

onMounted(async () => {
  installMvuGlobals();
  await executeCurrentMessageScripts();
  // 初始消息(会话历史)可能在 watch 建立前已渲染,资源卡片 iframe 需要显式补一次
  await hydrateRemoteResources();
  // 渲染面板(TH-render 等价物)同理由 watch 驱动,初始历史补一次
  await hydrateRenderPanels();
  // 渲染面板折叠按钮:全局事件委托(面板由 v-html 动态注入)
  scrollArea.value?.addEventListener('click', onRenderPanelToggleClick);
});

onBeforeUnmount(() => {
  scrollArea.value?.removeEventListener('click', onRenderPanelToggleClick);
  scriptRunner.cleanup();
});

/**
 * 渲染面板占位 HTML 已随消息渲染迁入 ChatMessageItem(渲染缓存)。
 * 渲染面板投递(阶段五 5b):面板 iframe 加载后,宿主经 postMessage 投递面板 HTML,
 * 宿主文档 appendChild 注入执行(仿 hydrateRemoteResources 的 ready/boot 双向认证 +
 * 超时兜底;面板 HTML 直接内嵌于 data-kd-render-code,无后端代理)。
 * 高度自适应:面板内 parent.postMessage 上报 {type:'kd-panel-resize', height},
 * 宿主据此调整 iframe 高度;事件上报 {type:'kd-panel-event'} 走 console 记录(扩展位)。
 */
async function hydrateRenderPanels(): Promise<void> {
  if (!scrollArea.value) return;
  const panels = scrollArea.value.querySelectorAll<HTMLElement>('[data-kd-render-panel="1"]');
  for (const panel of panels) {
    const frame = panel.querySelector<HTMLIFrameElement>('[data-kd-render-frame="1"]');
    if (!frame || frame.dataset.kdLoaded) continue;
    const nonce = panel.dataset.kdRenderNonce;
    const code = panel.dataset.kdRenderCode;
    if (!nonce || code === undefined) continue;
    const html = buildRenderDocument(code, RENDER_PANEL_CHANNEL, nonce);
    const win = frame.contentWindow;
    if (!win) {
      // 宿主文档尚未加载完成(contentWindow 为 null):延迟重试(仿资源卡片);
      // 此时 kdLoaded 未标记,重扫会再次进入本分支直至就绪
      const retry = (attempt: number): void => {
        if (frame.dataset.kdLoaded || !frame.isConnected) return;
        if (frame.contentWindow) void hydrateRenderPanels();
        else if (attempt < 50) setTimeout(() => retry(attempt + 1), 300);
      };
      setTimeout(() => retry(1), 300);
      continue;
    }
    frame.dataset.kdLoaded = '1';
    // 高度上报监听(宿主文档面板脚本触发;按 nonce 认证,防伪造面板)
    const onSize = (ev: MessageEvent): void => {
      const m = ev.data;
      if (!m || ev.source !== win || m.channel !== RENDER_PANEL_CHANNEL || m.nonce !== nonce) return;
      if (m.type === 'kd-panel-resize' && typeof m.height === 'number') {
        const h = Math.min(Math.max(Math.round(m.height), 80), 3000);
        if (frame.style.height !== `${h}px`) frame.style.height = `${h}px`;
      } else if (m.type === 'kd-panel-event') {
        console.info('[kedai-render-panel]', m.message ?? '');
      }
    };
    window.addEventListener('message', onSize);
    // ready 后投递;ready 先于监听发出时用超时兜底直接投递(闩锁只接受第一条 boot)
    let ready = false;
    let injected = false;
    const inject = (): void => {
      if (injected) return;
      injected = true;
      win.postMessage({ channel: RENDER_PANEL_CHANNEL, nonce, type: 'boot', html }, '*');
    };
    const onReady = (ev: MessageEvent): void => {
      const m = ev.data;
      if (ev.source !== win || !m) return;
      if (m.channel === RENDER_PANEL_CHANNEL && m.nonce === nonce && m.type === 'ready' && !ready) {
        ready = true;
        window.removeEventListener('message', onReady);
        inject();
      }
    };
    window.addEventListener('message', onReady);
    setTimeout(() => {
      if (!ready) {
        window.removeEventListener('message', onReady);
        inject();
      }
    }, 12000);
  }
}

/** iframe srcdoc 注入用:HTML 属性转义(资源页原文仅作 srcdoc 字符串,不拼接进本页面) */
function escapeAttr(s: string): string {
  return s.replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

/**
 * 渲染面板折叠切换(阶段五 5b):面板由 v-html 动态注入,直接绑定会随重渲染丢失,
 * 故用 scrollArea 上的全局事件委托。收起 = 隐藏 iframe 并展示代码块原文,
 * 展开 = 恢复 iframe(已注入的宿主文档保留,无需重新投递)。
 */
function onRenderPanelToggleClick(e: MouseEvent): void {
  const btn = (e.target as HTMLElement).closest<HTMLElement>('.sv-render-panel-toggle');
  if (!btn || !scrollArea.value?.contains(btn)) return;
  const panel = btn.closest<HTMLElement>('[data-kd-render-panel="1"]');
  const code = panel?.dataset.kdRenderCode;
  if (!panel || code === undefined) return;
  const frame = panel.querySelector<HTMLElement>('[data-kd-render-frame="1"]');
  const note = panel.querySelector<HTMLElement>('.sv-render-panel-note');
  if (panel.dataset.kdFolded === '1') {
    // 展开回面板
    panel.dataset.kdFolded = '';
    if (frame) frame.style.display = '';
    if (note) note.style.display = '';
    panel.querySelector<HTMLElement>('[data-kd-render-code-text]')?.remove();
    btn.textContent = '收起 ▾';
    void hydrateRenderPanels();
  } else {
    // 收起为代码块原文
    panel.dataset.kdFolded = '1';
    if (frame) frame.style.display = 'none';
    if (note) note.style.display = 'none';
    const codeEl = document.createElement('pre');
    codeEl.dataset.kdRenderCodeText = '1';
    codeEl.textContent = code;
    panel.appendChild(codeEl);
    btn.textContent = '展开 ▸';
  }
}

/**
 * 远程资源界面加载:卡片渲染后,父页面(带 token)经 /api/resource/proxy 取作者
 * 页面 HTML,投递给独立宿主文档 /resource-frame.html(iframe src 加载,不继承
 * 父页面 CSP,作者脚本可执行;宿主文档 DOM 重建触发脚本,详见其模板)。
 * 绕开作者服务器 X-Frame-Options 与 CSP 对 srcdoc/iframe 直嵌的拦截;
 * iframe sandbox 无 same-origin,与主页面完全隔离。
 */
async function hydrateRemoteResources(): Promise<void> {
  if (!scrollArea.value) return;
  const frames = scrollArea.value.querySelectorAll<HTMLIFrameElement>('iframe[data-kd-resource-frame="1"]');
  for (const frame of frames) {
    if (frame.dataset.kdLoaded) continue;
    const card = frame.closest('[data-kd-resource-url]') as HTMLElement | null;
    const rawUrl = card?.dataset.kdResourceUrl;
    const nonce = card?.dataset.kdResourceNonce;
    if (!rawUrl || !nonce) continue;
    const url = decodeURIComponent(rawUrl);
    const win = frame.contentWindow;
    if (!win) {
      // iframe 宿主文档尚未加载完成(contentWindow 为 null):延迟重试,
      // 不依赖 load 事件(load 可能先于监听触发或 lazy 加载延后,导致永不被投递)。
      // 每 300ms 重试,最多 50 次(15s);期间 kdLoaded 未标记,超时后放弃(卡片留有"新窗口打开"兜底)。
      const retry = (attempt: number): void => {
        if (frame.dataset.kdLoaded || !frame.isConnected) return;
        if (frame.contentWindow) {
          void hydrateRemoteResources();
        } else if (attempt < 50) {
          setTimeout(() => retry(attempt + 1), 300);
        }
      };
      setTimeout(() => retry(1), 300);
      continue;
    }
    frame.dataset.kdLoaded = '1';
    // 等宿主文档 ready 后投递(ready/boot 双向认证);ready 先于监听发出时
    // 用超时兜底直接投递(宿主文档闩锁只接受第一条 boot,重复投递无害)
    let ready = false;
    let injected = false;
    const inject = async (): Promise<void> => {
      if (injected) return;
      injected = true;
      /** iframe 不继承宿主 CSS 变量:运行时读取令牌拼 srcdoc 兜底样式,避免硬编码色值漂移 */
      const fallbackStyle = (): string => {
        const red = getComputedStyle(document.documentElement).getPropertyValue('--sv-red').trim() || '#e3342f';
        return `--sv-red:${red};--font-body:'Segoe UI','PingFang SC','Microsoft YaHei',system-ui,sans-serif;padding:16px;font-family:var(--font-body);color:var(--sv-red)`;
      };
      try {
        const res = await authorizedFetch(`${BASE}/resource/proxy?url=${encodeURIComponent(url)}`, undefined, false);
        const body = (await res.json().catch(() => ({}))) as { ok?: boolean; html?: string; base_url?: string; error?: string };
        if (body.ok) {
          const html = body.base_url ? `<base href="${escapeAttr(body.base_url)}">\n${body.html ?? ''}` : (body.html ?? '');
          win.postMessage({ channel: 'kedai-resource-frame-v1', nonce, type: 'boot', html }, '*');
        } else {
          frame.srcdoc = `<div style="${fallbackStyle()}">资源界面加载失败：${escapeAttr(body.error ?? '未知错误')}</div>`;
        }
      } catch (e) {
        frame.srcdoc = `<div style="${fallbackStyle()}">资源界面加载失败：${escapeAttr(String((e as Error).message || '网络错误'))}</div>`;
      }
    };
    const onMessage = (ev: MessageEvent): void => {
      const m = ev.data;
      if (ev.source !== win || !m) return;
      if (m.channel === 'kedai-resource-frame-v1' && m.nonce === nonce && m.type === 'ready' && !ready) {
        ready = true;
        window.removeEventListener('message', onMessage);
        void inject();
      }
    };
    window.addEventListener('message', onMessage);
    // ready 等待上限:资源页(如吸血鬼卡 1.25MB)经后端代理下载需要数秒,
    // 4s 内 ready 未到时直接投递易在宿主文档监听就绪前丢失 boot → 界面空白。
    // 放宽到 12s 覆盖代理下载 + 宿主文档重建时间。
    setTimeout(() => {
      if (!ready) {
        window.removeEventListener('message', onMessage);
        void inject();
      }
    }, 12000);
  }
}

/**
 * 消息列表变更纪元:中部消息被编辑保存 / swipe 切换(经子组件 mutated 事件)时 +1。
 * O(1) 签名只跟踪尾部消息,中部内容变更借纪元显式触发脚本重跑与滚动,语义与原全量签名一致。
 */
const listEpoch = ref(0);

/** 编辑中的消息 id(单编辑语义;编辑文本由子组件本地管理,击键不触发列表重渲染) */
const editingId = ref<number | null>(null);

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
    await nextTick();
    if (scrollArea.value) scrollArea.value.scrollTop = scrollArea.value.scrollHeight;
  },
);

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
    await Promise.allSettled([executeCurrentMessageScripts(), hydrateRemoteResources(), hydrateRenderPanels()]);
  },
  { flush: 'post', immediate: true },
);

// 会话切换
async function onSessionChange(e: Event): Promise<void> {
  const id = (e.target as HTMLSelectElement).value;
  if (id) await store.switchSession(id);
}

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

/** 选定开场:mode=new → 用该开场新建会话;mode=switch → 当前会话清空并重置为该开场 */
async function pickGreeting(index: number): Promise<void> {
  greetingPickerOpen.value = false;
  try {
    if (greetingPickerMode.value === 'new') {
      await store.newSession(index);
    } else {
      await store.switchGreeting(index);
    }
  } catch (err) {
    alert(`开场切换失败:${(err as Error).message}`);
  }
}

/** 当前请求的 prompt 缓存命中率:命中率计算抽到 contextStats.ts(computeHitRate,与优化面板共用) */
const hitRate = computed<number | null>(() => computeHitRate(store.lastUsage));

/** 头像来源(优先角色头像) */
const avatarUrl = computed(() => {
  const c = currentCharacter.value;
  if (!c?.avatar_path) return null;
  return `/api/avatars/${c.avatar_path.split(/[\\/]/).pop()}`;
});

// assistant 消息 HTML 渲染逻辑已迁入 ChatMessageItem(按消息 computed 缓存);
// 本组件仅保留列表编排、脚本调度与滚动行为。

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
    void Promise.allSettled([executeCurrentMessageScripts(), hydrateRemoteResources(), hydrateRenderPanels()]);
  },
});
</script>

<template>
  <section class="flex min-h-0 min-w-0 flex-1 flex-col">
    <!-- 顶栏 -->
    <header class="sv-topbar">
      <span class="flex items-center gap-2">
        <span class="sv-supreme blue" style="width: 10px; height: 10px" />
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
              @click="currentGreetings.length > 1 ? openGreetingPicker('new') : store.newSession()"
            >＋</button>
          </template>
          <!-- 开场切换(多开场角色:重置当前会话并以所选开场重新开始) -->
          <button
            v-if="currentGreetings.length > 1"
            class="sv-btn ghost sv-btn-sm"
            title="切换开场(清空当前会话并以所选开场重新开始)"
            @click="openGreetingPicker('switch')"
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
        <span class="sv-token-inline">
          CTX {{ store.contextTokens.toLocaleString() }}
          <span class="sv-supreme pink" style="width: 6px; height: 6px" />
          {{ store.lastUsage?.total_tokens ?? 0 }}
          <template v-if="hitRate !== null">
            <span class="sv-supreme green" style="width: 6px; height: 6px" />
            命中 {{ hitRate }}%
          </template>
        </span>
      </span>
    </header>

    <!-- 消息画布 -->
    <div ref="scrollArea" class="sv-canvas">
      <div v-if="messages.length === 0" class="sv-empty">
        <div class="sv-empty-geo">
          <span class="sq black" />
          <span class="sq pink" />
          <span class="sq deep" />
          <i class="diag" />
        </div>
        <p>选择左侧角色开始对话,或上传一张角色卡。</p>
        <p style="font-size: 11px; color: var(--sv-ink-faint)">
          输入含算式(如「帮我算 12*34」)时,Agent 会自动调用计算器工具。
        </p>
      </div>

      <!-- 消息列表(渲染缓存:每条消息独立子组件,流式仅重算当前消息;
           props 未变的子组件在父重渲染时自动跳过;长会话虚拟滚动:缓冲区外渲染等高占位) -->
      <template v-for="(m, i) in messages" :key="m.id">
        <div
          v-if="!vm.isActive(m.id, i)"
          class="sv-msg-ph"
          :style="vm.placeholderStyle(m)"
          :ref="(el) => vm.bindRow(m.id, el)"
        ></div>
        <ChatMessageItem
          v-else
          :ref="(inst) => vm.bindRow(m.id, inst)"
          :m="m"
          :editing="editingId === m.id"
          :avatar-url="avatarUrl"
          :character-name="currentCharacterName"
          :render-html="renderHtml"
          :scripts="currentScripts"
          :script-hash="currentScriptHash"
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
          <div class="sv-msg-bubble" style="border-top-width: 2px">
            <span class="sv-thinking"><i /><i /><i /></span>
          </div>
        </div>
      </div>
    </div>

    <!-- 底部输入区 -->
    <ChatInput />

    <!-- 开场选择器(多开场角色:新建会话选择 / 会话内切换) -->
    <div v-if="greetingPickerOpen" class="sv-modal-mask" @click.self="greetingPickerOpen = false">
      <div class="sv-modal sm">
        <div class="sv-modal-head">
          <h2 class="flex items-center gap-2">
            <span class="sv-supreme pink-deep" />
            {{ greetingPickerMode === 'new' ? '选择开场 · 新建会话' : '选择开场 · 重置当前会话' }}
          </h2>
          <button class="sv-btn ghost sv-btn-square" @click="greetingPickerOpen = false">✕</button>
        </div>
        <div class="sv-modal-body">
          <p class="sv-note" style="margin-bottom: 12px">
            {{ greetingPickerMode === 'new'
              ? '选择一个开场,以其作为新会话的第一条消息。'
              : '切换开场将清空当前会话全部消息,并以所选开场重新开始。' }}
          </p>
          <div class="sv-datalist">
            <button
              v-for="(g, i) in currentGreetings"
              :key="i"
              class="sv-data-row sv-greet-row"
              @click="pickGreeting(i)"
            >
              <div class="info">
                <b>
                  {{ i === 0 ? '主开场' : `备用 ${i}` }}
                  <span v-if="i === 0" class="sv-tag sv-tag-on">默认</span>
                </b>
                <span class="preview">{{ g }}</span>
              </div>
            </button>
          </div>
        </div>
        <div class="sv-modal-foot">
          <button class="sv-btn ghost" @click="greetingPickerOpen = false">取消</button>
        </div>
      </div>
    </div>
  </section>
</template>
