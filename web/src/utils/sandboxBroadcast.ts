// sandboxBroadcast.ts — 存活沙箱注册表与卡级事件广播（L1）。
//
// C-2F-3 自 `sandbox/iframe-lifecycle.ts` 下沉：广播只是「向已登记的 contentWindow
// 发 postMessage」，不含任何沙箱实现依赖；下沉到 L1 后 stores(L2) 可直接调用，
// 不必反向 import L3 的沙箱门面（原 `stores → characterScriptSandbox` 债务回边）。
//
// 生命周期约定：iframe 创建时 `registerSandboxSink(nonce, contentWindow)`，
// 销毁/清理时必须 `unregisterSandboxSink(nonce)`；未登记时广播是零副作用 no-op。

import type { MvuVariables } from '../mvu/variables';
import { CHANNEL, MAX_MESSAGE_BYTES, cloneData, utf8Bytes } from './sandboxProtocol';

/** 广播只需要 postMessage；收窄到结构类型让测试桩与真实 Window 都能直接登记。 */
export interface SandboxSink {
  postMessage(message: unknown, targetOrigin: string): void;
}

/** 存活沙箱注册表(nonce → contentWindow):MVU 变量更新时向全部在跑的状态栏脚本广播 */
const liveSandboxes = new Map<string, SandboxSink>();

/** 沙箱创建后登记消息出口（iframe-lifecycle 在 boot 注入成功后调用） */
export function registerSandboxSink(nonce: string, win: SandboxSink): void {
  liveSandboxes.set(nonce, win);
}

/** 沙箱销毁时注销消息出口；重复注销是安全的 no-op */
export function unregisterSandboxSink(nonce: string): void {
  liveSandboxes.delete(nonce);
}

/**
 * 向全部存活沙箱广播最新变量树(消息落库应用 UpdateVariable 后由 chat store 调用)。
 * 沙箱内合并变量并触发 eventOn 订阅(Mvu.events.VARIABLE_UPDATE_ENDED),状态栏随之重渲染。
 */
export function broadcastMvuUpdate(variables: MvuVariables): void {
  if (liveSandboxes.size === 0) return;
  let payload: { stat_data: Record<string, unknown>; display_data: Record<string, unknown> };
  try {
    payload = {
      stat_data: cloneData((variables.stat_data ?? {}) as Record<string, unknown>),
      display_data: cloneData((variables.display_data ?? {}) as Record<string, unknown>),
    };
    if (utf8Bytes(payload) > MAX_MESSAGE_BYTES) return; // 变量树超大时跳过广播,重渲染自取新快照
  } catch {
    return;
  }
  for (const [nonce, win] of liveSandboxes) {
    try {
      win.postMessage(
        { channel: CHANNEL, nonce, type: 'mvu-event', event: 'mag_variable_updated', payload, variables: payload },
        '*',
      );
    } catch {
      /* 沙箱已销毁:忽略 */
    }
  }
}

/**
 * 向全部存活沙箱广播卡级事件(swipe 切换等;对齐酒馆助手事件流)。
 * 沙箱内经事件总线 __kdFireEvent 派发给 eventOn 订阅者(如舰娘卡
 * 「随着开场白切换自动开启关闭世界书」脚本监听 tavern_events.MESSAGE_SWIPED)。
 */
export function broadcastCardEvent(name: string, payload?: unknown): void {
  if (liveSandboxes.size === 0 || !name) return;
  let cloned: unknown = payload;
  try {
    cloned = cloneData(payload ?? null);
    if (utf8Bytes(cloned) > MAX_MESSAGE_BYTES) return;
  } catch {
    return;
  }
  for (const [nonce, win] of liveSandboxes) {
    try {
      win.postMessage({ channel: CHANNEL, nonce, type: 'card-event', name, payload: cloned }, '*');
    } catch {
      /* 沙箱已销毁:忽略 */
    }
  }
}
