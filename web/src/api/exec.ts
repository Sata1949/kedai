// 命令执行审计与执行器状态 API(阶段 E)。
// 后端端点见 server-rs/src/api/exec.rs(tier / audit 列表 / 清空)。
import { request } from './client';

/** 执行器等级(与 Rust ShellTier serde snake_case 对齐) */
export type ShellTier = 'root' | 'shizuku' | 'sandbox' | 'disabled';

/** 命令风险级别(与 Rust CommandRisk serde snake_case 对齐;CU-1 新增 screen_input) */
export type CommandRiskLevel = 'safe' | 'sensitive' | 'destructive' | 'admin' | 'screen_input';

/** 执行器等级状态(含 Shizuku 环境信息;与 api/exec.rs 的响应对齐) */
export interface ExecTierInfo {
  tier: ShellTier;
  label: string;
  /** Shizuku 应用是否已安装(未安装 → 引导安装,而不是让用户点必然失败的按钮) */
  shizuku_installed: boolean;
  /** Shizuku 是否已获授权 */
  shizuku_granted: boolean;
  /** 本次是否为强制重探(「刷新」按钮走 refresh=1) */
  refreshed: boolean;
}

/**
 * GET /api/exec/tier:当前可用执行器等级。
 * `refresh=true` 时清探测缓存强制重探——用户新装 Shizuku / 刚授权 / 刚装 Magisk 后,
 * 不强制重探永远拿到缓存的过期值(设置页「刷新」按钮即为此用途)。
 */
export async function getExecTier(refresh = false): Promise<ExecTierInfo> {
  const qs = refresh ? '?refresh=1' : '';
  return request<ExecTierInfo>(`/exec/tier${qs}`);
}

/** 审计行(与 Rust services::exec::audit::ExecAuditEntry 对齐) */
export interface ExecAuditEntry {
  id: number;
  ts: string;
  source: string;
  task_id: string | null;
  session_id: string | null;
  command: string;
  shell: string;
  tier: string;
  risk: string;
  decision: string;
  exit_code: number | null;
  stdout_summary: string;
  stderr_summary: string;
  /** 风险标记(D1 审计增强):'' | data_dir_touch | parent_climb;只标记不拦截 */
  risk_flag: string;
}

/** GET /api/exec/audit:审计列表(时间倒序,limit 上限 500) */
export async function listExecAudit(
  opts: { limit?: number; source?: string; risk?: string } = {},
): Promise<ExecAuditEntry[]> {
  const params = new URLSearchParams();
  if (opts.limit) params.set('limit', String(opts.limit));
  if (opts.source) params.set('source', opts.source);
  if (opts.risk) params.set('risk', opts.risk);
  const qs = params.toString();
  const data = await request<{ entries: ExecAuditEntry[] }>(
    `/exec/audit${qs ? `?${qs}` : ''}`,
  );
  return data.entries;
}

/** DELETE /api/exec/audit:清空审计 */
export async function clearExecAudit(): Promise<number> {
  const data = await request<{ ok: boolean; deleted: number }>('/exec/audit', {
    method: 'DELETE',
  });
  return data.deleted;
}
