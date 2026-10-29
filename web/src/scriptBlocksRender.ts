// 脚本块渲染缓存包装(USERPERSONA-1 起从 ChatWindow 下沉:组件已顶到 1000 行阈值)。
//
// 背景(流式性能):调度器触发时会为每条历史 assistant 消息执行 renderScopedScripts
// (全量正则 + sanitize,重活),流式期间每个 token 触发一次,历史消息解析结果却不变。
// 按 (scopeId + scriptHash + depth + 渲染文本 + 用户称呼) 记忆结果,历史消息零重算。
//
// 2026-09-17 P-9:缓存由「组件作用域 + 无上限」改为**模块级有界**(容量 500,FIFO 淘汰,
// 见 renderCache.ts)。理由:原实现只在 messages/scripts 引用变化时整体重建,长会话下
// 条目无界增长;且组件卸载(如切走再切回聊天页)会丢失全部缓存。
// role 判定:renderScopedScripts 返回 null 表示「未命中任何脚本」,这是**有效结果**
// (缓存它可省掉每次重跑全量正则去确认无命中),故用 get 的返回值区分而非真假值。
import { renderScopedScripts } from './render';
import { buildScriptBlocksKey, scriptBlocksCache } from './renderCache';

/**
 * 缓存键与宏上下文需要的动态输入。declared 为取值函数而非快照:
 * 脚本指纹 / 当前角色 / 用户称呼都会在运行期变化,必须在每次调用时现取。
 */
export interface ScriptBlocksRenderDeps {
  /** 当前脚本指纹(换卡 / 改脚本 → 换键) */
  scriptHash: () => string;
  /** 宏展开用角色名(空串 = 不展开 {{char}}) */
  charName: () => string;
  /** 宏展开用用户称呼(USERPERSONA-1;空串按显示层默认「用户」) */
  userName: () => string;
}

/** 与 renderScopedScripts 同形的渲染器签名(单测可注入替身) */
type ScopedRenderer = typeof renderScopedScripts;

/**
 * 构造「带缓存的脚本块渲染」函数。
 * `render` 可注入(默认 renderScopedScripts),单测据此断言命中缓存不重复渲染。
 */
export function createScriptBlocksRender(
  deps: ScriptBlocksRenderDeps,
  render: ScopedRenderer = renderScopedScripts,
): (
  text: string,
  scripts: Parameters<ScopedRenderer>[1],
  scopeId: string,
  depth: number,
) => ReturnType<ScopedRenderer> {
  return (text, scripts, scopeId, depth) => {
    // depth 参与缓存键:同一条消息在不同楼层深度下脚本适用性不同(实跑问题 7 R2)
    const userName = deps.userName();
    const key = buildScriptBlocksKey(scopeId, deps.scriptHash(), depth, text, userName);
    const hit = scriptBlocksCache.get(key);
    if (hit !== undefined) return hit;
    const scoped = render(text, scripts, scopeId, depth, {
      charName: deps.charName(),
      userName,
    });
    scriptBlocksCache.set(key, scoped ?? null);
    return scoped;
  };
}
