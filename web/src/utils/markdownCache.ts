// 任务面板的 markdown 预渲染缓存（C-2A，2026-10-25）。
//
// 原 TaskBoard 里有 7 套小缓存（planRendered / teamGroupsRendered / messageHtmlCache /
// resultMain/Audit/FinalPlan / plannedPlanHtml），各自按数组或消息 id 失效。拆成子组件后
// 统一走这里：键 = 原文本身（内容变了自然换键），容器复用 renderCache.ts 的有界 FIFO。
//
// 边界：
// - 只用于**已落库/稳定的文本**；流式增量仍走纯文本插值（ChatMessageItem 的流式语义不动）。
// - 容量 128 是任务场景的经验值（计划步骤 + 分工结果 + 成果三段 + 历史消息），远小于
//   ChatMessageItem 的 500；避免把超长成果文本无限驻留。
import { renderMarkdown } from '../markdown';
import { createBoundedCache } from '../renderCache';

const cache = createBoundedCache<string>(128);

/** 带缓存的 renderMarkdown；空串直接返回空串，不占缓存位。 */
export function renderMarkdownCached(text: string): string {
  if (!text) return '';
  const hit = cache.get(text);
  if (hit !== undefined) return hit;
  const html = renderMarkdown(text);
  cache.set(text, html);
  return html;
}

/** 测试/诊断用：清空缓存。 */
export function clearMarkdownRenderCache(): void {
  cache.clear();
}

/** 测试/诊断用：当前缓存条目数。 */
export function markdownRenderCacheSize(): number {
  return cache.stats().size;
}
