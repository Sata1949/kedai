// 设置 / 连接 / 模型 / Token 与提示词注入 API
import { request } from './client';
import { uploadForm } from './stream';
import {
  requireArrayField,
  requireNumberField,
  requireObject,
  requireObjectField,
  requireStringField,
} from './shape';
import type {
  ConnectorInfo,
  KedaiImportBundle,
  PromptInjectConfig,
  PromptPreview,
  PromptPresetImportResult,
  RegexScript,
  RuntimeSettings,
  RuntimeSettingsPatch,
} from './types';

/** 逐连接探测参数(2026-10-03 API 设置补全):显式 `connection` 草稿字段级覆盖 >
 *  `connection_id` 命中已存连接(缺省字段与密钥取已存值)> 皆缺省 = 测当前生效连接器 */
export interface ConnectionProbeParams {
  connection_id?: string;
  connection?: {
    connector_type?: string;
    base_url?: string;
    /** 留空/缺省 = 用已存密钥(密钥仅服务端持有);非空 = 显式覆盖 */
    api_key?: string;
    api_style?: string;
    model?: string;
  };
}

export async function testConnect(
  probe?: ConnectionProbeParams,
): Promise<{ ok: boolean; message: string; models: string[] }> {
  return request('/settings/connect', { method: 'POST', body: JSON.stringify(probe ?? {}) });
}

/** POST /api/settings/embedding/test:测试向量化连接(嵌入固定文本,回传实际维度与耗时) */
export async function testEmbedding(): Promise<{
  ok: boolean;
  dim?: number;
  latency_ms?: number;
  message: string;
}> {
  return request('/settings/embedding/test', { method: 'POST', body: '{}' });
}

export async function settingsInfo(): Promise<ConnectorInfo> {
  const data = await request<unknown>('/settings/info');
  // 形状闸门:调用方解构 connector/model/models 回填设置页,空对象会让连接器名恒为空
  return requireObject<ConnectorInfo>(data, '连接器信息');
}

export async function getModel(): Promise<string> {
  const data = await request<unknown>('/settings/model');
  return requireStringField(data, 'model', '模型信息');
}

export async function switchModel(model: string): Promise<{ model: string; changed: boolean }> {
  return request('/settings/model', { method: 'PUT', body: JSON.stringify({ model }) });
}

export async function listModels(): Promise<string[]> {
  const data = await request<unknown>('/settings/models');
  return requireArrayField<string>(data, 'models', '模型列表');
}

/** GET /api/settings:读取运行期设置(API Key 脱敏);mode 指定读取哪个模式的合并值 */
export async function getSettings(mode?: 'roleplay' | 'task'): Promise<RuntimeSettings> {
  const q = mode ? `?mode=${mode}` : '';
  return request(`/settings${q}`);
}

/** POST /api/settings/refresh-models:向已保存的 API 请求可用模型列表;带探测参数时按连接探测 */
export async function refreshModels(
  probe?: ConnectionProbeParams,
): Promise<{ models: string[]; message: string | null }> {
  const data = await request<unknown>('/settings/refresh-models', {
    method: 'POST',
    body: JSON.stringify(probe ?? {}),
  });
  // 形状闸门:models 直接决定下拉框内容,解出 undefined 会让模型列表永久为空
  const models = requireArrayField<string>(data, 'models', '模型列表');
  const message = (data as { message?: unknown }).message;
  return { models, message: typeof message === 'string' ? message : null };
}

/** PUT /api/settings:保存运行期设置(部分字段;Base URL/Key 变更立即重建连接器);mode 指定写入哪个模式 */
export async function saveSettings(patch: RuntimeSettingsPatch, mode?: 'roleplay' | 'task'): Promise<{ ok: boolean; settings: RuntimeSettings }> {
  const q = mode ? `?mode=${mode}` : '';
  return request(`/settings${q}`, { method: 'PUT', body: JSON.stringify(patch) });
}

/** GET /api/settings/agent-prompt:读取运行时主 Agent 提示词(DATA_DIR/AGENTS_RUNTIME.md,注入模型) */
export async function getAgentPromptMd(): Promise<{ path: string; content: string }> {
  return request('/settings/agent-prompt');
}

/** PUT /api/settings/agent-prompt:保存运行时主 Agent 提示词(写回 DATA_DIR/AGENTS_RUNTIME.md) */
export async function saveAgentPromptMd(content: string): Promise<{ ok: boolean; path: string }> {
  return request('/settings/agent-prompt', { method: 'PUT', body: JSON.stringify({ content }) });
}

/** GET /api/settings/prompt-preview：按来源、角色、层级与顺序查看最终提示词；历史正文已脱敏。
 *  mode 指定按哪个模式的合并设置预览(task 走 for_mode 覆盖层并追加三层固定提示词)。 */
export async function getPromptPreview(sessionId?: string, characterId?: string, mode?: 'roleplay' | 'task'): Promise<PromptPreview> {
  const query = new URLSearchParams();
  if (sessionId) query.set('session_id', sessionId);
  if (characterId) query.set('character_id', characterId);
  if (mode) query.set('mode', mode);
  return request(`/settings/prompt-preview${query.size ? `?${query}` : ''}`);
}

// ===== 提示词注入(简单模式 + 楼层系统) =====

/** GET /api/prompt-inject:读取注入配置 */
export async function getPromptInject(): Promise<PromptInjectConfig> {
  const data = await request<unknown>('/prompt-inject');
  // 形状闸门:PromptManager 直接渲染 config.mode/floors,空对象会让面板白屏
  return requireObjectField<PromptInjectConfig>(data, 'config', '提示词注入配置');
}

/** PUT /api/prompt-inject:全量保存注入配置 */
export async function savePromptInject(config: PromptInjectConfig): Promise<PromptInjectConfig> {
  const data = await request<unknown>('/prompt-inject', {
    method: 'PUT',
    body: JSON.stringify({ config }),
  });
  return requireObjectField<PromptInjectConfig>(data, 'config', '提示词注入配置');
}

/** POST /api/prompt-inject/import:导入酒馆(SillyTavern)预设 JSON。
 *  `mode='append'` 追加到现有楼层之后;缺省/`replace` = 替换(后端 multipart mode 字段)。 */
export async function importPromptPreset(
  file: File,
  mode?: 'replace' | 'append',
): Promise<PromptPresetImportResult> {
  return uploadForm<PromptPresetImportResult>(
    '/prompt-inject/import',
    file,
    mode ? { mode } : {},
  );
}

// ===== 全局正则库(IMPHUB ⑥)与综合导出(bundle v1) =====

/** GET /api/regex-scripts:读取全局正则库(整体;跨角色生效的显示层正则) */
export async function getRegexScripts(): Promise<RegexScript[]> {
  const data = await request<{ ok: boolean; scripts: RegexScript[] }>('/regex-scripts');
  return data.scripts;
}

/** PUT /api/regex-scripts:全量替换全局正则库(启停/删除/导入都走整体写)。
 *  条目宽进:取不到查找模式的条目由后端跳过,`skipped` 如实回报。 */
export async function saveRegexScripts(
  scripts: unknown[],
): Promise<{ count: number; skipped: number; scripts: RegexScript[] }> {
  const data = await request<{ ok: boolean; count: number; skipped: number; scripts: RegexScript[] }>(
    '/regex-scripts',
    { method: 'PUT', body: JSON.stringify({ scripts }) },
  );
  return { count: data.count, skipped: data.skipped, scripts: data.scripts };
}

/** GET /api/export/bundle?mode=…:综合导出件(bundle v1;段序与归一由后端锁定)。
 *  `warnings` = 插件目录里解析失败、未导出的文件(不影响其余段)。 */
export async function exportBundle(
  mode: 'roleplay' | 'task',
): Promise<{ bundle: KedaiImportBundle; warnings: string[] }> {
  const data = await request<{ ok: boolean; bundle: KedaiImportBundle; warnings: string[] }>(
    `/export/bundle?mode=${mode}`,
  );
  return { bundle: data.bundle, warnings: data.warnings ?? [] };
}

// ===== Token 计数 =====

export async function countTokens(messages: Array<{ role: string; content: string }>, model?: string): Promise<number> {
  const data = await request<unknown>('/token/count', {
    method: 'POST',
    body: JSON.stringify({ messages, model }),
  });
  // 形状闸门:token 计数直接参与上下文水位计算,undefined 会污染后续算术
  return requireNumberField(data, 'total', 'Token 计数');
}

/** GET /api/token/session-total:当前会话累计 token */
export async function getSessionTotalTokens(sessionId: string): Promise<number> {
  const data = await request<unknown>(`/token/session-total?session_id=${encodeURIComponent(sessionId)}`);
  return requireNumberField(data, 'total_tokens', '会话 Token 累计');
}

/** GET /api/token/global-total:全局累计 token */
export async function getGlobalTotalTokens(): Promise<number> {
  const data = await request<unknown>('/token/global-total');
  return requireNumberField(data, 'total_tokens', '全局 Token 累计');
}
