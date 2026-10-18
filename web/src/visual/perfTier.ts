/**
 * 前端性能档位:只做保守的能力判定,供 CSS(模糊强度)与 WebGL(帧率/分辨率)共用。
 *
 * 判定不追求精确 GPU 型号——浏览器的硬件信息本身不可靠;这里只把「明显不适合
 * 全屏动态模糊/高分辨率 shader」的设备降到 low:
 *   - 移动端/窄屏:功耗与 GPU 预算优先;
 *   - 逻辑核数 ≤ 4 或 deviceMemory ≤ 4:低端桌面/旧设备。
 * 缺失能力信息时按 high 处理,避免误伤桌面。
 */
export type PerfTier = 'high' | 'low';

export interface PerfTierInput {
  cores?: number;
  /** Chromium 的 navigator.deviceMemory(GB);其它浏览器可能缺失 */
  deviceMemory?: number;
  /** 首帧视口是否为窄屏/移动端(<768px) */
  mobile?: boolean;
}

export function resolvePerfTier(input: PerfTierInput = {}): PerfTier {
  if (input.mobile === true) return 'low';
  if (typeof input.cores === 'number' && input.cores > 0 && input.cores <= 4) return 'low';
  if (typeof input.deviceMemory === 'number' && input.deviceMemory > 0 && input.deviceMemory <= 4) return 'low';
  return 'high';
}

/** 写到 <html> 上,供主题 CSS 的 [data-perf-tier] 规则读取;null 时静默跳过。 */
export function applyPerfTier(root: HTMLElement | null, tier: PerfTier): void {
  if (!root) return;
  root.dataset.perfTier = tier;
}
