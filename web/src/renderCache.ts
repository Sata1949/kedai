// 渲染结果有界缓存(2026-09-17 P-9)。
//
// 背景:虚拟滚动(`composables/useVirtualMessages`)离开缓冲区的消息会**卸载组件**、降级为
// 等高占位 div;滚回来时组件重新挂载,而 `ChatMessageItem` 的 `html` 是**组件内 computed**,
// 缓存随实例销毁。于是同一条消息来回滚动会反复付
// `renderScopedScripts` / `renderMarkdown` / `sanitize-html` 的成本
// (参照实测:sanitize-html 处理 16.8k 字符 ≈ 3.057ms,约 markdown-it 的 7 倍)。
//
// 做法:把缓存提到**模块级**,跨组件实例存活;容量有上限并按写入顺序淘汰,避免长会话下
// 无界增长(内存风险)。淘汰策略与容量都在此处集中定义,不在调用点各写一份。
//
// 键纪律(**本批次唯一真风险**):键必须包含**渲染输入本身**(文本内容 + 全部渲染开关),
// 而不是消息 id——否则「内容变了但 id 没变」会命中陈旧条目,表现为界面显示旧文本。
import type { ScopedRenderResult } from './render';

/** 默认容量:足以覆盖多屏来回滚动,又不至于让长会话累积成内存负担 */
export const DEFAULT_RENDER_CACHE_LIMIT = 500;

export interface CacheStats {
  hits: number;
  misses: number;
  evictions: number;
  size: number;
}

export interface BoundedCache<V> {
  /** 取值;未命中返回 `undefined` 并计入 misses(故 `V` 不应含 `undefined`) */
  get(key: string): V | undefined;
  /** 写入;超出容量时淘汰最久未被写入的条目 */
  set(key: string, value: V): void;
  /** 清空条目并归零统计(测试隔离与调试用) */
  clear(): void;
  stats(): CacheStats;
}

/**
 * 创建有容量上限的字符串键缓存。
 * 淘汰语义:FIFO(按**写入**顺序),重复写入同一键会刷新其位置——即「很久没写过的先走」。
 */
export function createBoundedCache<V>(limit: number = DEFAULT_RENDER_CACHE_LIMIT): BoundedCache<V> {
  // 容量下限钳到 1:容量 0 会让缓存永远不命中(功能静默失效),不如让它退化为「只留最新一条」
  const cap = Number.isFinite(limit) && limit >= 1 ? Math.floor(limit) : 1;
  const map = new Map<string, V>();
  let hits = 0;
  let misses = 0;
  let evictions = 0;

  return {
    get(key) {
      if (map.has(key)) {
        hits += 1;
        return map.get(key);
      }
      misses += 1;
      return undefined;
    },
    set(key, value) {
      if (map.has(key)) map.delete(key);
      map.set(key, value);
      while (map.size > cap) {
        const oldest = map.keys().next();
        if (oldest.done) break;
        map.delete(oldest.value);
        evictions += 1;
      }
    },
    clear() {
      map.clear();
      hits = 0;
      misses = 0;
      evictions = 0;
    },
    stats() {
      return { hits, misses, evictions, size: map.size };
    },
  };
}

/**
 * 消息 HTML 渲染结果缓存(`ChatMessageItem` 的 `html` computed 使用)。
 * 键由 `buildMessageRenderKey` 统一构造,保证「命中即等价于重算」。
 */
export const messageHtmlCache = createBoundedCache<string>(DEFAULT_RENDER_CACHE_LIMIT);

/**
 * 脚本块解析结果缓存(`ChatWindow` 的脚本调度器使用)。
 * 值为 `renderScopedScripts` 的完整返回(含待执行脚本块),**`null` 是有效值**
 * (表示未命中任何脚本),故调用方必须区分「`null`」与「`undefined`(未命中)」。
 */
export const scriptBlocksCache = createBoundedCache<ScopedRenderResult | null>(
  DEFAULT_RENDER_CACHE_LIMIT,
);

/** 消息渲染缓存的键输入(全部为渲染实际读取的输入) */
export interface MessageRenderKeyInput {
  /** 渲染文本(正文 + 状态栏占位符;即 `paintText`) */
  text: string;
  /** HTML 渲染开关:决定是否走 scoped 脚本分支 */
  renderHtml: boolean;
  /** 正则脚本内容版本 hash:脚本变化时必变(脚本字段也参与渲染) */
  scriptHash: string;
  /** 消息楼层深度:脚本 min_depth/max_depth 过滤依据 */
  depth: number;
  /** 渲染作用域 id(容器标识;参与 scoped HTML 输出) */
  scopeId: string;
  /** 宏展开用角色名(脚本替换串中的 {{char}}) */
  charName: string;
}

/**
 * 构造消息渲染缓存键。
 * 以 `\n` 分隔并**以文本结尾**:各字段不含换行(scopeId/hash/数字/角色名为单行),
 * 故不存在「不同输入拼出同一键」的歧义。
 */
export function buildMessageRenderKey(i: MessageRenderKeyInput): string {
  return `${i.scopeId}\n${i.scriptHash}\n${i.depth}\n${i.renderHtml ? 1 : 0}\n${i.charName}\n${i.text}`;
}

/**
 * 构造脚本块解析缓存键。口径沿用原 `ChatWindow` 实现(scopeId + scriptHash + depth + text),
 * `depth` 参与是因为同一条消息在不同楼层深度下脚本适用性不同(实跑问题 7 R2)。
 */
export function buildScriptBlocksKey(
  scopeId: string,
  scriptHash: string,
  depth: number,
  text: string,
): string {
  return `${scopeId}\n${scriptHash}\n${depth}\n${text}`;
}
