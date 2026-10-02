// 安卓截图 / 无障碍状态 API(移动端视觉能力包 A4)。
// 后端端点见 server-rs/src/api/screen.rs(status / settings)。
import { request } from './client';

/** 截图能力状态(与 api/screen.rs 的响应对齐;字段 snake_case 与全仓 API 同口径) */
export interface ScreenStatus {
  /** 截图通道是否可用(安卓 = 无障碍截图服务已启用;非安卓恒 false,本端点供安卓设置面板使用) */
  available: boolean;
  /** 可读原因/说明(未启用时为可操作指引) */
  reason: string;
  /** 运行平台:android | other */
  platform: 'android' | 'other';
  /** 「视觉与截图」总开关当前值(全局扁平) */
  vision_screenshot_enabled: boolean;
  /** 安卓专属信息 */
  android: {
    /** 无障碍截图服务是否已被系统启用(列表法判定,不依赖服务连接实例) */
    accessibility_enabled: boolean;
  };
}

/**
 * GET /api/screen/status:截图能力状态。
 * `refresh=true` 时清无障碍状态缓存强制重探——用户刚在系统设置里开启/关闭服务后,
 * 不强制重探会一直拿到缓存旧值(设置页「刷新」按钮即为此用途)。
 */
export async function getScreenStatus(refresh = false): Promise<ScreenStatus> {
  const qs = refresh ? '?refresh=1' : '';
  return request<ScreenStatus>(`/screen/status${qs}`);
}

/** POST /api/screen/settings:跳系统「无障碍」设置页(需用户手动开启服务) */
export async function openAccessibilitySettings(): Promise<void> {
  await request<{ ok: boolean }>('/screen/settings', { method: 'POST' });
}
