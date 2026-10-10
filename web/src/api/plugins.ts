// 插件(自定义工具)API。后端契约见 server-rs/src/api/plugins.rs:
//   GET /api/plugins/tools、POST /api/plugins/tools/upload、POST /api/plugins/tools/reload、
//   DELETE /api/plugins/tools/{name}、POST /api/plugins/tools/move、POST /api/plugins/tools/import、
//   GET /api/plugins/tools/export
import { request } from './client';
import { uploadForm } from './stream';
import type { PluginToolsStatus } from './types';

/** 列出已注册工具插件 + 磁盘文件 */
export async function listPluginTools(): Promise<PluginToolsStatus> {
  return request<PluginToolsStatus>('/plugins/tools');
}

/** 导入工具插件 JSON 文件(multipart) */
export async function uploadPluginTool(file: File): Promise<{ ok: boolean; name: string; file: string }> {
  return uploadForm<{ ok: boolean; name: string; file: string }>('/plugins/tools/upload', file);
}

/** 热重载全部工具插件(全量同步:增删都收敛;`removed` = 本次差集注销数) */
export async function reloadPluginTools(): Promise<{ ok: boolean; loaded: number; removed?: number; errors?: string[] }> {
  return request<{ ok: boolean; loaded: number; removed?: number; errors?: string[] }>('/plugins/tools/reload', { method: 'POST' });
}

/** 删除已导入的工具插件文件 */
export async function deletePluginTool(name: string): Promise<{ ok: boolean; removed: string }> {
  return request<{ ok: boolean; removed: string }>(`/plugins/tools/${encodeURIComponent(name)}`, { method: 'DELETE' });
}

/** 移动插件到虚拟文件夹(IMPHUB 附件管理;原地改写插件文件的 folder 字段,不重注册) */
export async function movePluginTool(file: string, folder: string): Promise<{ ok: boolean; file: string; folder: string }> {
  return request<{ ok: boolean; file: string; folder: string }>('/plugins/tools/move', {
    method: 'POST',
    body: JSON.stringify({ file, folder }),
  });
}

/** 批量导入插件(综合导入 ⑦;逐项校验、全部通过才落盘,写完后一次全量同步)。
 *  条目 = `{file, folder?, ...ToolPluginConfig}`(与导出端点同形,可原样回导)。 */
export async function importPluginTools(
  plugins: readonly unknown[],
): Promise<{ ok: boolean; imported: number; loaded: number; removed: number }> {
  return request<{ ok: boolean; imported: number; loaded: number; removed: number }>(
    '/plugins/tools/import',
    { method: 'POST', body: JSON.stringify({ plugins }) },
  );
}

/** 导出全部插件文件(综合导出的 ⑦ 段数据源;解析失败的文件不导出,原因随 errors 回报) */
export async function exportPluginTools(): Promise<{
  plugins: Array<Record<string, unknown>>;
  errors: string[];
}> {
  const data = await request<{ ok: boolean; plugins: Array<Record<string, unknown>>; errors: string[] }>(
    '/plugins/tools/export',
  );
  return { plugins: data.plugins, errors: data.errors ?? [] };
}
