// 缓存诊断 API:GET /api/diagnostics/cache(server-rs/src/api/diagnostics.rs)。
// 可选 query:session_id(会话过滤,缺省 = 全部会话合并)、window(统计窗口条数,
// 默认 20,1..=500);entries 按时间正序返回便于前端画趋势。
import { request } from './client';

/** 单条请求的缓存统计(llm_requests 行,时间正序) */
export interface CacheUsageEntry {
  session_id: string;
  created_at: string;
  prompt_tokens: number;
  completion_tokens: number;
  /** 缓存命中 token */
  hit: number;
  /** 未命中 token */
  miss: number;
}

/** 窗口汇总:hit_rate 为 token 加权口径 total_hit/(total_hit+total_miss),
 *  提供商不回报缓存字段时为 null(不得显示为 0%) */
export interface CacheTotals {
  count: number;
  total_prompt: number;
  total_completion: number;
  total_hit: number;
  total_miss: number;
  hit_rate: number | null;
}

/** 每百万 token 单价(元;DeepSeek 参考价 0.27/2/8) */
export interface CachePricing {
  cache_hit_per_m: number;
  input_per_m: number;
  output_per_m: number;
  currency_hint: string;
}

/** 上下文四级水位:level ok/soft/snip/compact/force/unknown(未配置上限),
 *  ratio = input_tokens / max_context_tokens(unknown 时 null) */
export interface CacheWatermark {
  level: string;
  ratio: number | null;
  input_tokens: number;
  max_context_tokens: number;
  next_level: string | null;
  gap_tokens: number | null;
}

/** 诊断响应:命中率汇总 + 逐条明细 + 费用/节省估算(后端已算好,元)+ 水位报告 */
export interface CacheDiagnostics {
  window: number;
  session_id: string | null;
  totals: CacheTotals;
  entries: CacheUsageEntry[];
  pricing: CachePricing;
  cost: number;
  saved: number;
  watermark: CacheWatermark;
}

/**
 * 校验响应形状:后端正常时必带 totals/entries/watermark 三个对象。
 *
 * 存在的理由(实测教训):后端曾在**失败时也返回 200** 并携带 `{"error": ...}`。
 * 由于 `request()` 只按 HTTP 状态判成败,该载荷会被当作 `CacheDiagnostics` 返回,
 * 调用方随即在 `data.totals.hit_rate` 上抛 TypeError(可选链只保护了 `data` 本身,
 * 挡不住「形状不对的真值对象」)。后端已改为失败返回 5xx,这里再加一道形状闸门,
 * 使「载荷不对」表现为可捕获的 Error 而不是下游的随机崩溃。
 */
function assertDiagnostics(value: unknown): CacheDiagnostics {
  const v = value as Partial<CacheDiagnostics> | null | undefined;
  const ok =
    !!v &&
    typeof v === 'object' &&
    typeof v.totals === 'object' &&
    v.totals !== null &&
    Array.isArray(v.entries) &&
    typeof v.watermark === 'object' &&
    v.watermark !== null;
  if (!ok) {
    // 兼容旧版后端:带着 error 文案的 200 响应,把原文透出便于定位
    const err = (v as { error?: unknown } | null)?.error;
    throw new Error(typeof err === 'string' && err.trim() ? err : '缓存统计响应格式异常');
  }
  return v as CacheDiagnostics;
}

/** GET /api/diagnostics/cache:读取近 N 轮缓存命中率、费用估算与上下文水位 */
export async function getCacheDiagnostics(
  opts: { sessionId?: string; window?: number } = {},
): Promise<CacheDiagnostics> {
  const query = new URLSearchParams();
  if (opts.sessionId) query.set('session_id', opts.sessionId);
  if (opts.window !== undefined) query.set('window', String(opts.window));
  const raw = await request<unknown>(`/diagnostics/cache${query.size ? `?${query}` : ''}`);
  return assertDiagnostics(raw);
}

/**
 * 一套「选中的库之外、也有用户数据」的数据目录(DB-7)。
 *
 * 后端判据与启动期告警同源(`config::detect_split_data_dirs`):有 settings.json,
 * 或库里已有非内置角色/任何会话;空壳目录不报。
 */
export interface SplitDataDir {
  /** 另一套库的绝对路径(给用户定位用) */
  path: string;
  /** 判定依据的中文说明,如「settings.json + 数据库」 */
  evidence: string;
}

/** 数据目录分叉报告:当前生效目录 + 检测到的其它库 */
export interface DataDirsReport {
  /** 当前进程实际使用的数据目录 */
  data_dir: string;
  /** 另一套有用户数据的库(空数组 = 无分叉);顺序与后端候选顺序一致 */
  others: SplitDataDir[];
}

/**
 * 形状闸门:后端正常时必带 `data_dir` 与 `others` 数组。
 *
 * 与 `assertDiagnostics` 同一理由——`request()` 只按 HTTP 状态判成败,
 * 载荷形状不对时应在**这里**抛可捕获的 Error,而不是让调用方在
 * `report.others.length` 上抛 TypeError。`others` 缺失尤其危险:它会让
 * 「有无分叉」的判据落到 undefined,静默变成「没分叉」——恰好是 DB-7 要堵的静默。
 */
function assertDataDirs(value: unknown): DataDirsReport {
  const v = value as Partial<DataDirsReport> | null | undefined;
  if (!v || typeof v !== 'object' || typeof v.data_dir !== 'string' || !Array.isArray(v.others)) {
    throw new Error('数据目录检测响应格式异常');
  }
  const others = v.others
    .filter((item): item is SplitDataDir =>
      !!item && typeof item === 'object' && typeof item.path === 'string',
    )
    .map((item) => ({ path: item.path, evidence: typeof item.evidence === 'string' ? item.evidence : '' }));
  return { data_dir: v.data_dir, others };
}

/** GET /api/diagnostics/data-dirs:读取当前数据目录与「另一套有用户数据的库」(DB-7) */
export async function getDataDirs(): Promise<DataDirsReport> {
  const raw = await request<unknown>('/diagnostics/data-dirs');
  return assertDataDirs(raw);
}

