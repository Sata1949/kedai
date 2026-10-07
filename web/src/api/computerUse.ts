// 电脑操作治理 API(CU-1,2026-10-06):急停开关 + 操作审计。
// 后端端点见 server-rs/src/api/computer_use.rs;审计只含元数据,不含屏幕像素。
import { request } from './client';

/** 急停状态(与后端 { stopped } 响应对齐) */
export interface CuStatus {
  stopped: boolean;
}

/** 一条操作审计(与 Rust services::computer_use::CuAuditEntry 对齐) */
export interface CuAuditEntry {
  id: number;
  ts: string;
  /** chat | task */
  source: string;
  task_id: string | null;
  session_id: string | null;
  /** windows | android */
  platform: string;
  /** read_screen(当前唯一) */
  action: string;
  /** 范围描述(全屏 / 显示器 i / 区域 / 窗口「标题」) */
  target: string;
  /** allowed | denied */
  decision: string;
  /** ok | refused | error */
  result: string;
  /** 稳定错误码:'' | CONTROL_STOPPED | CAPTURE_DISABLED | CAPTURE_FAILED */
  error_code: string;
  /** 图像引用名(frame_id;未落盘为空串) */
  image_ref: string;
  png_bytes: number | null;
  /** PNG sha256(留痕但不落像素) */
  sha256: string;
}

/** GET /api/computer-use/status:当前急停状态 */
export async function getCuStatus(): Promise<CuStatus> {
  return request<CuStatus>('/computer-use/status');
}

/** POST /api/computer-use/stop:置位急停(幂等) */
export async function stopComputerUse(): Promise<CuStatus> {
  return request<CuStatus>('/computer-use/stop', { method: 'POST' });
}

/** POST /api/computer-use/resume:解除急停(仅用户显式操作;幂等) */
export async function resumeComputerUse(): Promise<CuStatus> {
  return request<CuStatus>('/computer-use/resume', { method: 'POST' });
}

/** GET /api/computer-use/audit:操作审计列表(时间倒序,limit 上限 500) */
export async function listCuAudit(limit = 100): Promise<CuAuditEntry[]> {
  const data = await request<{ entries: CuAuditEntry[] }>(
    `/computer-use/audit?limit=${limit}`,
  );
  return data.entries;
}

/** DELETE /api/computer-use/audit:清空审计(返回删除行数) */
export async function clearCuAudit(): Promise<number> {
  const data = await request<{ ok: boolean; deleted: number }>('/computer-use/audit', {
    method: 'DELETE',
  });
  return data.deleted;
}
