// store 间回调桥(架构护栏:M5 断环)。
//
// 用途:当一个 store 需要读/调用另一个 store 的状态或动作、而对方（直接或间接）
// 又依赖自己时,顶层的 `import { useXStore }` 就会成环。做法是:由**被依赖方**在
// 自己的 setup 里把「读值函数」或「动作」注册进来,依赖方通过本模块取用——
// 依赖方向因此变为单向(都指向本叶子模块)。
//
// 类型化注册(批次 5.1):六条桥统一由 createBridge 构造,每条桥是「一个注册口 +
// 一个取用口」的对象;register 的 owner 必填——同一 owner 重复注册是幂等覆盖
// (store 重建实例等正常场景),不同 owner 抢同一桥会 console.warn 诊断,不静默覆盖。
// get() 在未注册时返回本桥构造时声明的**显式降级实现**(见 storeBridges 各条注释),
// 与断环前各调用点的旧行为一一对应。
//
// 为什么不把状态提到模块级 ref:Pinia 的实例隔离依赖「状态创建于 setup 内」。
// 模块级 ref 会让状态脱离 Pinia 实例(测试里 createPinia() 无法重置、多实例串数据)。
// 故本模块只保存**引用/函数**,状态仍住在各自 store 的 setup 里。
//
// 纪律:本模块不 import 任何 store;未注册时的降级行为要显式、可诊断。
import type * as api from '../api';

// ---------- 桥的通用结构 ----------

/** 类型化回调桥:一个注册口 + 一个取用口。 */
export interface StoreBridge<T> {
  /**
   * 注册实现。owner 必填(如 'character' / 'genSettings'):
   * 同一 owner 重复注册 = 幂等覆盖;不同 owner 覆盖 = 桥的 owner 约定被打破,
   * 打印警告便于定位误注册(仍以最后一次注册为准)。
   */
  register(owner: string, fn: T): void;
  /** 取用实现;未注册时返回构造时声明的显式降级实现。 */
  get(): T;
}

/** 造一条桥。name 仅用于诊断日志;fallback 是未注册时的显式降级行为。 */
function createBridge<T>(name: string, fallback: T): StoreBridge<T> {
  let impl: T | null = null;
  let owner: string | null = null;
  return {
    register(nextOwner: string, fn: T): void {
      if (owner !== null && owner !== nextOwner) {
        console.warn(`[kedai] storeBridge「${name}」被不同 owner 重复注册:${owner} → ${nextOwner}(后者生效)`);
      }
      impl = fn;
      owner = nextOwner;
    },
    get(): T {
      return impl ?? fallback;
    },
  };
}

// ---------- 六条桥的定义 ----------

/** 读当前选中角色 id。 */
export type CharacterIdProvider = () => string | null;

/** 队列化保存运行期设置(genSettings.queueSettingsSave)。 */
export type SettingsSaver = (patch: api.RuntimeSettingsPatch) => Promise<void>;

/** 重新加载运行期设置(genSettings.loadSettings)。 */
export type SettingsLoader = () => Promise<void>;

/**
 * 「全局 render_html 默认值已变更」的通知口。
 * 由 uiPrefs 注册:收到后写 defaultRenderHtml 并按角色卡记忆重算生效值。
 */
export type RenderHtmlDefaultSink = (renderHtmlDefault: boolean) => void;

/** 聊天侧 SSE 事件上报口(chat.onSseEvent):任务事件原样透传事件监控面板。 */
export type ChatEventSink = (ev: api.SseEvent) => void;

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

/**
 * 六条桥(owner 与降级行为集中登记;业务调用点走下方具名包装,owner 已固定)。
 *
 * 降级语义一览(未注册时 get() 的行为):
 *   - characterId → () => null(等同「未选角色」);
 *   - settingsSaver → reject(调用方本就有 catch,走既有「保存失败回滚」分支,
 *     与网络失败同路径,不会静默丢数据);
 *   - settingsLoader → no-op;
 *   - renderHtmlDefault → no-op(仅影响该展示值);
 *   - chatEvent → no-op;
 *   - uiPrefs → 安全空实现(不崩、不误展开面板,isCallTraceOpen 恒 false)。
 */
export const storeBridges = {
  /** 读当前角色 id(owner:character store;读取方:chat / uiPrefs) */
  characterId: createBridge<CharacterIdProvider>('characterId', () => null),
  /** 设置保存(owner:genSettings;调用方:uiPrefs / task) */
  settingsSaver: createBridge<SettingsSaver>('settingsSaver', () =>
    Promise.reject(new Error('设置服务尚未就绪(genSettings store 未初始化),已跳过保存')),
  ),
  /** 设置重载(owner:genSettings;调用方:task 切换顶层模式后) */
  settingsLoader: createBridge<SettingsLoader>('settingsLoader', async () => {}),
  /** 全局 render_html 默认值通知(owner:uiPrefs;通知方:genSettings) */
  renderHtmlDefault: createBridge<RenderHtmlDefaultSink>('renderHtmlDefault', () => {}),
  /** 聊天事件上报(owner:chat;调用方:task 的 SSE 分发) */
  chatEvent: createBridge<ChatEventSink>('chatEvent', () => {}),
  /** 界面偏好能力(owner:uiPrefs;调用方:task) */
  uiPrefs: createBridge<UiPrefsSink>('uiPrefs', {
    collapseAgentPanel: () => {},
    autoOpenAgentPanel: () => {},
    isCallTraceOpen: () => false,
  }),
} as const;

// ---------- 具名包装(向后兼容:调用点名字与签名不变,owner 在此固定) ----------

/** 由 character store 在 setup 阶段注册(传入读自己 currentCharacterId 的闭包)。 */
export function registerCharacterIdProvider(fn: CharacterIdProvider): void {
  storeBridges.characterId.register('character', fn);
}

/** 当前选中角色 id;未注册(character store 未初始化)时为 null(等同「未选角色」)。 */
export function currentCharacterIdValue(): string | null {
  return storeBridges.characterId.get()();
}

/** 由 genSettings store 在 setup 阶段注册(同一 owner 重复注册为幂等覆盖)。 */
export function registerSettingsSaver(fn: SettingsSaver): void {
  storeBridges.settingsSaver.register('genSettings', fn);
}

/**
 * 队列化保存设置(uiPrefs 持久化 render_html 默认值用)。
 * 未注册(genSettings 从未被实例化)时返回 reject —— 调用方本就有 catch,
 * 走既有的「保存失败回滚」分支,与网络失败同路径,不会静默丢数据。
 */
export function queueSettingsSave(patch: api.RuntimeSettingsPatch): Promise<void> {
  return storeBridges.settingsSaver.get()(patch);
}

/** 由 genSettings store 在 setup 阶段注册。 */
export function registerSettingsLoader(fn: SettingsLoader): void {
  storeBridges.settingsLoader.register('genSettings', fn);
}

/** 重新加载设置(切换顶层模式后按新模式覆盖层刷新)。未注册时静默跳过。 */
export function reloadSettings(): void {
  void storeBridges.settingsLoader.get()();
}

/** 由 uiPrefs store 在 setup 阶段注册。 */
export function registerRenderHtmlDefaultSink(fn: RenderHtmlDefaultSink): void {
  storeBridges.renderHtmlDefault.register('uiPrefs', fn);
}

/** 通知全局 render_html 默认值变更。未注册时静默跳过(仅影响该展示值)。 */
export function notifyRenderHtmlDefault(renderHtmlDefault: boolean): void {
  storeBridges.renderHtmlDefault.get()(renderHtmlDefault);
}

/** 由 chat store 在 setup 阶段注册。 */
export function registerChatEventSink(fn: ChatEventSink): void {
  storeBridges.chatEvent.register('chat', fn);
}

/** 上报聊天侧 SSE 事件。未注册时静默跳过。 */
export function reportChatEvent(ev: api.SseEvent): void {
  storeBridges.chatEvent.get()(ev);
}

/** 由 uiPrefs store 在 setup 阶段注册。 */
export function registerUiPrefsSink(fn: UiPrefsSink): void {
  storeBridges.uiPrefs.register('uiPrefs', fn);
}

/** 取 uiPrefs 能力;未注册时返回安全的空实现(不崩、不误展开面板)。 */
export function uiPrefsBridge(): UiPrefsSink {
  return storeBridges.uiPrefs.get();
}
