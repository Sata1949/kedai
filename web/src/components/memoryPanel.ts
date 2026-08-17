// 记忆库面板展示纯函数(优化面板「记忆库」分区):
// kind 标签映射与配色、last_usage 格式化、列表行映射排序、蒸馏反馈文案、
// 删除两段式确认状态机。与 cacheHealth.ts 同风格:纯函数供组件渲染与 Vitest 共用。
import type { MemoryEntry } from '../api';

/** kind → 中文标签(未知 kind 原样展示,不吞异常数据) */
export function kindLabel(kind: string): string {
  if (kind === 'distilled') return '蒸馏';
  if (kind === 'tool') return '工具';
  if (kind === 'manual') return '手动';
  return kind;
}

/** kind → 标签配色 class(蓝=蒸馏 / 黄=工具 / 绿=手动;未知中性灰) */
export function kindClass(kind: string): string {
  if (kind === 'distilled' || kind === 'tool' || kind === 'manual') return `kind-${kind}`;
  return 'kind-unknown';
}

/** last_usage 展示:null(从未注入)→ 未使用;ISO → 截断到分(与 cacheHealth 的 formatEntryTime 同口径) */
export function formatMemoryTime(iso: string | null): string {
  if (!iso) return '未使用';
  return iso.replace('T', ' ').slice(0, 16);
}

/** 列表展示行(kind 已映射标签与配色,时间已格式化) */
export interface MemoryRow {
  id: number;
  content: string;
  kind: string;
  kindLabel: string;
  kindClass: string;
  usage: number;
  lastUsage: string;
  selected: boolean;
}

/** 列表行映射:按 id 降序(最新在前,与后端 ORDER BY id DESC 一致,幂等保证展示稳定) */
export function memoryRows(entries: MemoryEntry[]): MemoryRow[] {
  return [...entries]
    .sort((a, b) => b.id - a.id)
    .map((e) => ({
      id: e.id,
      content: e.content,
      kind: e.kind,
      kindLabel: kindLabel(e.kind),
      kindClass: kindClass(e.kind),
      usage: e.usage_count,
      lastUsage: formatMemoryTime(e.last_usage),
      selected: e.selected,
    }));
}

/** 蒸馏成功文案:inserted>0 → 新增 N 条;0(空历史,后端未调模型)→ 无可提取提示 */
export function distillSuccessText(inserted: number): string {
  return inserted > 0
    ? `蒸馏完成:新增 ${inserted} 条记忆`
    : '蒸馏完成:当前会话没有可提取的记忆';
}

/** 蒸馏失败文案:未开启(后端 400 指引)→ 追加去设置开启的路径;其余错误原样带前缀 */
export function distillErrorText(message: string): string {
  if (message.includes('未开启')) {
    return `蒸馏失败:${message}。可到 设置 → 生成参数 打开「记忆蒸馏」后重试`;
  }
  return `蒸馏失败:${message}`;
}

/** 删除两段式确认状态:点「删除」→ 该行进入确认态;「确认删除」/「取消」→ 退出 */
export function nextPendingDelete(
  current: number | null,
  action: 'request' | 'confirm' | 'cancel',
  id: number,
): number | null {
  if (action === 'request') return id;
  return null;
}
