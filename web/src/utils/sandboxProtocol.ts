// sandboxProtocol.ts — 沙箱通信协议的纯常量与序列化原语（L1）。
//
// C-2F-3 自 `sandbox/protocol.ts` 下沉：stores(L2) 需要在消息状态变更后广播给沙箱，
// 而广播函数只能依赖 L1 才能消除 L2→L3 回边。这里的常量/函数无任何沙箱实现依赖，
// 归 L1；`sandbox/protocol.ts` 继续从这里再导出，保持既有导入面不变。

export const CHANNEL = 'kedai-character-script-v1';

/**
 * 沙箱 boot 消息上限。角色卡开场/状态栏脚本动辄几十 KB(如 WuWa 状态栏 107KB,
 * 开场 40KB),64KB 会把它们整段拒掉 → 协议弹窗/悬浮球/状态栏永不显示。
 * postMessage 跨窗口传输无浏览器级硬上限(与内存一致),这里放宽到 1MB 防内存压力。
 */
export const MAX_MESSAGE_BYTES = 1024 * 1024;

/** 深拷贝为纯数据（跨 realm postMessage 的结构化克隆语义由调用方依赖） */
export function cloneData<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

/** JSON 序列化后的 UTF-8 字节数（用于 1MB 消息上限判定） */
export function utf8Bytes(value: unknown): number {
  return new TextEncoder().encode(JSON.stringify(value)).byteLength;
}
