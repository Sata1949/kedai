// 显示层正则的**合并口径**(IMPHUB ⑥,2026-10-09):全局库 + 角色卡级正则。
//
// 顺序契约(与后端 services/regex_service.rs 模块头、docs/契约-协议与配置.md 同源):
// **全局在前、卡级在后**——卡级更具体,后应用覆盖全局同型替换。
//
// 单独成 L1 纯函数模块的两个理由:
//   ① 合并被两处消费(stores/character 的授权哈希 + components/ChatWindow 的渲染),
//      复制实现必然漂移;
//   ② 缓存键闸门:授权哈希(scriptAuthorization.hashRegexScripts)与渲染缓存键
//      (renderCache.buildScriptBlocksKey 经 currentScriptHash)都必须覆盖全局库,
//      否则「改了全局正则不重渲染」——合并点收敛成一个函数,闸门才只有一处。
import type { RegexScript } from '../api';

/**
 * 合并全局正则与卡级正则(全局在前、卡级在后)。
 *
 * 无全局库时**返回原数组引用**(与历史行为逐字节一致:空全局不引入新的哈希与缓存键
 * 抖动);两处都空返回空数组。
 */
export function mergedRegexScripts(
  global: readonly RegexScript[] | null | undefined,
  card: readonly RegexScript[] | null | undefined,
): RegexScript[] {
  const g = global ?? [];
  const c = card ?? [];
  if (g.length === 0) return c as RegexScript[];
  if (c.length === 0) return [...g];
  return [...g, ...c];
}
