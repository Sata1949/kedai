// store 间回调桥(架构护栏:M5 断环)。
//
// 用途:当一个 store 需要读/调用另一个 store 的状态或动作、而对方（直接或间接）
// 又依赖自己时,顶层的 `import { useXStore }` 就会成环。做法是:由**被依赖方**在
// 自己的 setup 里把「读值函数」或「动作」注册进来,依赖方通过本模块取用——
// 依赖方向因此变为单向(都指向本叶子模块)。
//
// 为什么不把状态提到模块级 ref:Pinia 的实例隔离依赖「状态创建于 setup 内」。
// 模块级 ref 会让状态脱离 Pinia 实例(测试里 createPinia() 无法重置、多实例串数据)。
// 故本模块只保存**引用/函数**,状态仍住在各自 store 的 setup 里。
//
// 纪律:本模块不 import 任何 store;未注册时的降级行为要显式、可诊断。
import type * as api from '../api';

// ---------- 当前角色 id(owner:character store;读取方:chat / uiPrefs) ----------

/** 读当前选中角色 id。 */
type CharacterIdProvider = () => string | null;

let characterIdProvider: CharacterIdProvider | null = null;

/** 由 character store 在 setup 阶段注册(传入读自己 currentCharacterId 的闭包)。 */
export function registerCharacterIdProvider(fn: CharacterIdProvider): void {
  characterIdProvider = fn;
}

/** 当前选中角色 id;未注册(character store 未初始化)时为 null(等同「未选角色」)。 */
export function currentCharacterIdValue(): string | null {
  return characterIdProvider ? characterIdProvider() : null;
}

// ---------- 设置读写(owner:genSettings;调用方:uiPrefs / task) ----------

/** 队列化保存运行期设置(genSettings.queueSettingsSave)。 */
type SettingsSaver = (patch: api.RuntimeSettingsPatch) => Promise<void>;

let settingsSaver: SettingsSaver | null = null;

/** 由 genSettings store 在 setup 阶段注册(幂等:重复注册覆盖)。 */
export function registerSettingsSaver(fn: SettingsSaver): void {
  settingsSaver = fn;
}

/**
 * 队列化保存设置(uiPrefs 持久化 render_html 默认值用)。
 * 未注册(genSettings 从未被实例化)时返回 reject —— 调用方本就有 catch,
 * 走既有的「保存失败回滚」分支,与网络失败同路径,不会静默丢数据。
 */
export function queueSettingsSave(patch: api.RuntimeSettingsPatch): Promise<void> {
  if (!settingsSaver) {
    return Promise.reject(new Error('设置服务尚未就绪(genSettings store 未初始化),已跳过保存'));
  }
  return settingsSaver(patch);
}

/** 重新加载运行期设置(genSettings.loadSettings)。 */
type SettingsLoader = () => Promise<void>;

let settingsLoader: SettingsLoader | null = null;

export function registerSettingsLoader(fn: SettingsLoader): void {
  settingsLoader = fn;
}

/** 重新加载设置(切换顶层模式后按新模式覆盖层刷新)。 */
export function reloadSettings(): void {
  void settingsLoader?.();
}

// ---------- 全局 render_html 默认值(owner:uiPrefs;通知方:genSettings) ----------

/**
 * 「全局 render_html 默认值已变更」的通知口。
 * 由 uiPrefs 注册:收到后写 defaultRenderHtml 并按角色卡记忆重算生效值。
 */
type RenderHtmlDefaultSink = (renderHtmlDefault: boolean) => void;

let renderHtmlDefaultSink: RenderHtmlDefaultSink | null = null;

/** 由 uiPrefs store 在 setup 阶段注册。 */
export function registerRenderHtmlDefaultSink(fn: RenderHtmlDefaultSink): void {
  renderHtmlDefaultSink = fn;
}

/** 通知全局 render_html 默认值变更。未注册时静默跳过(仅影响该展示值)。 */
export function notifyRenderHtmlDefault(renderHtmlDefault: boolean): void {
  renderHtmlDefaultSink?.(renderHtmlDefault);
}

// ---------- 聊天事件上报(owner:chat;调用方:task 的 SSE 分发) ----------

/** 聊天侧 SSE 事件上报口(chat.onSseEvent):任务事件原样透传事件监控面板。 */
type ChatEventSink = (ev: api.SseEvent) => void;

let chatEventSink: ChatEventSink | null = null;

export function registerChatEventSink(fn: ChatEventSink): void {
  chatEventSink = fn;
}

export function reportChatEvent(ev: api.SseEvent): void {
  chatEventSink?.(ev);
}

// ---------- 界面偏好能力(owner:uiPrefs;调用方:task) ----------

/**
 * uiPrefs 侧能力(task store 需要的三个界面操作)。
 * 以整体注册而非三组函数,便于 uiPrefs 一处声明、语义集中。
 */
export type UiPrefsSink = {
  /** 进入任务模式时收起 Agent 面板(走 collapseAgentPanel,记抑制标记) */
  collapseAgentPanel: () => void;
  /** 新建/切换任务时自动展开 Agent 面板(受用户抑制标记约束) */
  autoOpenAgentPanel: () => void;
  /** 调用追踪面板是否展开(决定 llm_call 事件是否触发重拉) */
  isCallTraceOpen: () => boolean;
};

let uiPrefsSink: UiPrefsSink | null = null;

export function registerUiPrefsSink(fn: UiPrefsSink): void {
  uiPrefsSink = fn;
}

/** 取 uiPrefs 能力;未注册时返回安全的空实现(不崩、不误展开面板)。 */
export function uiPrefsBridge(): UiPrefsSink {
  return (
    uiPrefsSink ?? {
      collapseAgentPanel: () => {},
      autoOpenAgentPanel: () => {},
      isCallTraceOpen: () => false,
    }
  );
}
