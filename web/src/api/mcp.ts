// MCP 管理面 API(PLGM 3.2)。后端契约见 server-rs/src/api/mcp.rs:
//   GET  /api/mcp/servers
//   POST /api/mcp/servers/{name}/restart | stop | start
import { request } from './client';

/** 单台 MCP 服务器状态(state 四值:running/failed/stopped/disabled) */
export interface McpServerStatus {
  name: string;
  enabled: boolean;
  state: 'running' | 'failed' | 'stopped' | 'disabled';
  tool_count: number;
  tools: string[];
  last_error?: string | null;
}

export interface McpServersStatus {
  servers: McpServerStatus[];
  mcp_enabled: boolean;
}

/** 列出 MCP 服务器状态(设置快照 × 运行时台账) */
export async function listMcpServers(): Promise<McpServersStatus> {
  return request<McpServersStatus>('/mcp/servers');
}

/** 重启单台(读当前设置快照——改配置后点它即生效,替代「重启应用」) */
export async function restartMcpServer(name: string): Promise<{ ok: boolean; name: string; state: string; tool_count: number }> {
  return request(`/mcp/servers/${encodeURIComponent(name)}/restart`, { method: 'POST' });
}

/** 停止单台(注销其工具,配置保留供重启) */
export async function stopMcpServer(name: string): Promise<{ ok: boolean; name: string; state: string; tool_count: number }> {
  return request(`/mcp/servers/${encodeURIComponent(name)}/stop`, { method: 'POST' });
}

/** 启动单台(显式动作,幂等) */
export async function startMcpServer(name: string): Promise<{ ok: boolean; name: string; state: string; tool_count: number }> {
  return request(`/mcp/servers/${encodeURIComponent(name)}/start`, { method: 'POST' });
}
