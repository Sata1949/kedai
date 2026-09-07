// cardScriptHost.ts — 卡级脚本(角色卡内嵌酒馆助手脚本)长驻沙箱宿主
//
// 与消息级脚本(scriptRunner:随每条消息渲染一次性执行)不同,卡级脚本随角色选中常驻:
// 监听事件流(message_swiped 等)、轮询聊天消息、经宿主 RPC 读写角色内嵌世界书。
// 首个用例:舰娘卡「随着开场白切换自动开启关闭世界书」(tavern_events.MESSAGE_SWIPED +
// 500ms 轮询兜底,按 swipe 索引批量开关世界书条目 enabled)。
//
// 安全边界(与消息级脚本同一模型):
// - 逐卡主动授权、哈希绑定内容(哈希含卡级脚本正文,卡更新脚本→授权自动失效);
// - 沙箱无 token:世界书读写经宿主 rpcExtensions 由带 token 的宿主页面转发;
// - lorebook-set 只放行 {uid, enabled} 白名单字段,防卡脚本借道改写世界书提示词。

import {
  executeSandboxedCharacterScript,
  type SandboxCleanup,
  type SandboxExecutionContext,
} from './characterScriptSandbox';
import { emptyVariables } from './mvu/variables';
import { getCharacterWorldEntries, saveCharacterWorldEntries } from './api/worldbooks';
import type { WorldBookEntry } from './api/types';
import type { CardScript } from './cardScripts';

/** 聊天消息视图(getChatMessages 的数据源;由调用方注入以便测试与解耦 store) */
export interface CardChatMessage {
  role: string;
  content: string;
  extra?: Record<string, unknown>;
}

export interface CardScriptHostOptions {
  characterId: string;
  /** 授权哈希(含卡级脚本正文):沙箱键的一部分,内容变化→旧沙箱销毁重建 */
  scriptHash: string;
  /** 已启用的卡级脚本(enabledCardScriptsOf 输出;空数组调用方应直接清理) */
  scripts: CardScript[];
  /** 沙箱容器(聊天滚动区;DOM 白名单作用域,卡级脚本一般只读数据不操作 DOM) */
  container: HTMLElement;
  /** 主世界书名(角色卡内嵌 character_book.name;无内嵌世界书为 null) */
  lorebookName: string | null;
  /** 当前会话消息(按楼层 0-based 正序),供 TavernHelper.getChatMessages */
  readMessages: () => CardChatMessage[];
}

// ---- 世界书条目形态映射:kedai WorldBookEntry → 酒馆助手条目 ----
// 酒馆助手脚本按 uid/comment/enabled/key/content 读条目;kedai 侧主键是 id、
// 关键词是 keys/keys_secondary,此处双写字段名兼容两种读法。
function toHelperEntry(e: WorldBookEntry): Record<string, unknown> {
  const keys = Array.isArray(e.keys) ? e.keys : [];
  const keysSecondary = Array.isArray(e.keys_secondary) ? e.keys_secondary : [];
  return {
    uid: e.id,
    comment: e.comment ?? '',
    enabled: e.enabled !== false,
    content: e.content ?? '',
    key: keys,
    keys,
    keysecondary: keysSecondary,
    keys_secondary: keysSecondary,
    constant: e.constant === true,
    position: e.position ?? 0,
    depth: e.depth ?? 0,
    order: e.order ?? 100,
    probability: e.probability ?? null,
  };
}

/**
 * lorebook-set:GET 全量 → 只应用 {uid, enabled} 白名单 → PUT 全量 → 返回更新视图。
 * 并发说明: kedai 世界书写回只有全量 PUT(无单条 PATCH),脚本回写与用户手动编辑
 * 并发时后写覆盖先写——与酒馆全量保存语义一致,此处注释明示风险。
 */
async function applyLorebookSet(
  characterId: string,
  list: unknown[],
): Promise<{ updated: number; entries: Array<Record<string, unknown>> }> {
  const patches = new Map<number, boolean>();
  for (const item of list) {
    if (!item || typeof item !== 'object') continue;
    const o = item as { uid?: unknown; enabled?: unknown };
    const uid = Number(o.uid);
    if (!Number.isInteger(uid) || typeof o.enabled !== 'boolean') continue;
    patches.set(uid, o.enabled);
  }
  const current = await getCharacterWorldEntries(characterId);
  let updated = 0;
  const next = current.map((e) => {
    const state = patches.get(e.id);
    if (state === undefined || (e.enabled !== false) === state) return e;
    updated += 1;
    return { ...e, enabled: state };
  });
  const saved = updated > 0 ? await saveCharacterWorldEntries(characterId, next) : current;
  return { updated, entries: saved.map(toHelperEntry) };
}

/** kedai 消息 → 酒馆助手 getChatMessages 条目形态 */
function toHelperChatMessage(m: CardChatMessage): Record<string, unknown> {
  const extra = m.extra ?? {};
  // kedai extra.swipes 元素为 {swipe_id, content, ts}(含激活版本);酒馆形态
  // swipes 是全部版本文本数组、swipe_id 指向当前版本,此处降维为文本数组。
  // 无版本数据时退化为 [content]/0(单版本),轮询类脚本据此按 swipe 0 处理。
  const rawSwipes = Array.isArray(extra.swipes) ? extra.swipes : [];
  const texts = rawSwipes.map((s) => {
    if (s && typeof s === 'object' && typeof (s as { content?: unknown }).content === 'string') {
      return (s as { content: string }).content;
    }
    return typeof s === 'string' ? s : '';
  });
  const swipeId =
    typeof extra.swipe_id === 'number' && Number.isInteger(extra.swipe_id) ? extra.swipe_id : 0;
  return {
    role: m.role,
    content: m.content,
    swipes: texts.length > 0 ? texts : [m.content],
    swipe_id: swipeId >= 0 && swipeId < (texts.length || 1) ? swipeId : 0,
  };
}

/**
 * 沙箱 RPC 扩展:世界书读写 / 聊天消息读取(纯数据 op,不过 DOM 白名单)。
 * name 与 boot 下发的主世界书名不符时返回 null/false(对齐资源帧 shim 语义,
 * 防脚本读写其他角色的世界书——kedai 一角色一内嵌世界书)。
 */
export function makeCardScriptRpcExtensions(
  opts: Pick<CardScriptHostOptions, 'characterId' | 'lorebookName' | 'readMessages'>,
): NonNullable<SandboxExecutionContext['rpcExtensions']> {
  return async (op, args) => {
    switch (op) {
      case 'lorebook-entries': {
        const name = String(args[0] ?? '');
        if (!opts.lorebookName || name !== opts.lorebookName) return { handled: true, value: null };
        const entries = await getCharacterWorldEntries(opts.characterId);
        return { handled: true, value: entries.map(toHelperEntry) };
      }
      case 'lorebook-set': {
        const name = String(args[0] ?? '');
        if (!opts.lorebookName || name !== opts.lorebookName) return { handled: true, value: false };
        const list = Array.isArray(args[1]) ? args[1] : [];
        return { handled: true, value: await applyLorebookSet(opts.characterId, list) };
      }
      case 'chat-messages': {
        // 楼层序号 0-based;负数从末尾数(-1 = 最后一条);越界返回空数组
        const id = Number(args[0]);
        const msgs = opts.readMessages();
        const idx = Number.isInteger(id) ? (id >= 0 ? id : msgs.length + id) : -1;
        if (idx < 0 || idx >= msgs.length) return { handled: true, value: [] };
        return { handled: true, value: [toHelperChatMessage(msgs[idx])] };
      }
      default:
        return { handled: false };
    }
  };
}

// ---- ESM 脚本跳过 ----
// 沙箱模板以经典脚本执行(内联 <script>,CSP 仅 'unsafe-inline'):含顶层
// import/export 的脚本会整体语法错误、沙箱空转到超时。酒馆助手生态里这类脚本
// (如 MVU Zod schema)依赖外部 URL 模块,CSP 同样不会放行,跳过是最诚实的行为。
const ESM_SYNTAX = /^\s*(import\s+[\w{*'"]|export\s+(?:const|let|var|function|class|default|\{))/m;
export function isEsmScript(content: string): boolean {
  return ESM_SYNTAX.test(content);
}

// ---- 长驻沙箱生命周期:一角色一键( characterId:scriptHash )一组沙箱 ----
interface ActiveCardSandboxes {
  key: string;
  cleanups: SandboxCleanup[];
}

let active: ActiveCardSandboxes | null = null;
// 启动串行化:角色快速切换时 ensure/cleanup 不交错(旧沙箱先毁,新沙箱后建)
let queue: Promise<unknown> = Promise.resolve();

/** 销毁当前卡级脚本沙箱(切角色/撤授权/组件卸载时调用);幂等 */
export function cleanupCardScriptSandbox(): void {
  const prev = active;
  active = null;
  for (const cleanup of prev?.cleanups ?? []) {
    try {
      cleanup();
    } catch {
      /* 单个沙箱清理失败不阻断其余 */
    }
  }
}

async function ensureInner(opts: CardScriptHostOptions): Promise<void> {
  const key = `${opts.characterId}:${opts.scriptHash}`;
  if (active?.key === key) return;
  cleanupCardScriptSandbox();
  const runnable = opts.scripts.filter((s) => {
    if (isEsmScript(s.content)) {
      console.warn(`[kedai-card-script] 跳过 ES 模块卡级脚本(沙箱不支持 import/export): ${s.name || s.id}`);
      return false;
    }
    return true;
  });
  if (runnable.length === 0) return;
  const rpcExtensions = makeCardScriptRpcExtensions(opts);
  const cleanups: SandboxCleanup[] = [];
  active = { key, cleanups };
  for (const script of runnable) {
    try {
      const cleanup = await executeSandboxedCharacterScript(script.content, {
        container: opts.container,
        variables: emptyVariables(),
        characterId: opts.characterId,
        lorebookName: opts.lorebookName,
        rpcExtensions,
      });
      cleanups.push(cleanup);
    } catch (error) {
      // 单个脚本失败(语法不兼容/超时)不阻断其余脚本;错误提示已由沙箱挂到容器
      console.error(
        `[kedai-card-script] 卡级脚本执行失败: ${script.name || script.id}`,
        error instanceof Error ? error.message : error,
      );
    }
  }
  // 全部失败(如容器已卸载):立即清掉,不留空键占用导致下次 ensure 短路
  if (cleanups.length === 0 && active?.key === key) cleanupCardScriptSandbox();
}

/**
 * 幂等启动/切换卡级脚本长驻沙箱:键( characterId:scriptHash )不变为 no-op,
 * 变化则销毁旧沙箱后重建。调用方需保证已满足授权门禁。
 */
export function ensureCardScriptSandbox(opts: CardScriptHostOptions): Promise<void> {
  const task = queue.then(() => ensureInner(opts));
  queue = task.catch(() => {});
  return task;
}
