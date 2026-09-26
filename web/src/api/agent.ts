// Agent 计划与自定义执行流程(agent-flows)API
import { request } from './client';
import type {
  AgentFlowBundle,
  AgentFlowConfig,
  AgentFlowLibrary,
  AgentMode,
  AgentPlan,
  FlowImportReport,
  ToolPermission,
} from './types';

export async function getToolPermissions(sessionId: string, characterId: string): Promise<ToolPermission[]> {
  const query = new URLSearchParams({ session_id: sessionId, character_id: characterId });
  const data = await request<{ tools: ToolPermission[] }>(`/agent/tool-permissions?${query}`);
  return data.tools;
}

/** 该会话/角色的显式授权(session_grants / role_grants),供授权管理区展示与撤销 */
export async function getGrants(
  sessionId: string,
  characterId: string,
): Promise<{ session: string[]; role: string[] }> {
  const query = new URLSearchParams({ session_id: sessionId, character_id: characterId });
  const data = await request<{ grants?: { session?: string[]; role?: string[] } }>(
    `/agent/tool-permissions?${query}`,
  );
  return { session: data.grants?.session ?? [], role: data.grants?.role ?? [] };
}

export async function authorizeTool(tool: string, scope: 'session' | 'role', sessionId: string): Promise<void> {
  // 10s 超时:防止请求挂起导致前端 authorizing 永久占用、授权按钮全部禁用
  await request('/agent/tool-permissions', {
    method: 'POST',
    body: JSON.stringify({ session_id: sessionId, tool, scope }),
    signal: AbortSignal.timeout(10000),
  });
}

export async function resolveToolAuthorization(
  tool: string,
  decision: 'once' | 'session' | 'role' | 'deny',
  sessionId: string,
  runId: string,
  callId: string,
): Promise<void> {
  await request('/agent/tool-permissions/resolve', {
    method: 'POST',
    body: JSON.stringify({ session_id: sessionId, tool, scope: decision, run_id: runId, call_id: callId }),
    signal: AbortSignal.timeout(10000),
  });
}

export async function revokeTool(tool: string, scope: 'session' | 'role', sessionId: string): Promise<void> {
  await request('/agent/tool-permissions', {
    method: 'DELETE',
    body: JSON.stringify({ session_id: sessionId, tool, scope }),
  });
}

export async function agentPlan(
  message: string,
  agentMode: AgentMode,
  sessionId?: string,
): Promise<AgentPlan> {
  return request('/agent/plan', {
    method: 'POST',
    body: JSON.stringify({ message, agent_mode: agentMode, session_id: sessionId }),
  });
}

/** GET /api/agent-flows:读取流程库 + 当前选中流程 */
export async function getAgentFlow(): Promise<{ library: AgentFlowLibrary; config: AgentFlowConfig | null }> {
  const data = await request<{ ok: boolean; library: AgentFlowLibrary; config: AgentFlowConfig | null }>(
    '/agent-flows',
  );
  return { library: data.library, config: data.config };
}

/** PUT /api/agent-flows:保存(创建或更新)流程并设为当前选中;id 为空 = 新建(校验失败 400) */
export async function saveAgentFlow(config: AgentFlowConfig): Promise<AgentFlowLibrary> {
  const data = await request<{ ok: boolean; library: AgentFlowLibrary }>('/agent-flows', {
    method: 'PUT',
    body: JSON.stringify({ config }),
  });
  return data.library;
}

/** POST /api/agent-flows/select:切换当前选中流程 */
export async function selectAgentFlow(id: string): Promise<AgentFlowLibrary> {
  const data = await request<{ ok: boolean; library: AgentFlowLibrary }>('/agent-flows/select', {
    method: 'POST',
    body: JSON.stringify({ id }),
  });
  return data.library;
}

/** DELETE /api/agent-flows/{id}:删除流程(删除当前流程时回退到第一个) */
export async function deleteAgentFlow(id: string): Promise<AgentFlowLibrary> {
  const data = await request<{ ok: boolean; library: AgentFlowLibrary }>(`/agent-flows/${encodeURIComponent(id)}`, {
    method: 'DELETE',
  });
  return data.library;
}

/** 导出搬运包(二维批次 7a):给 id 导出该流程 + 可达子流程闭包,缺省导出全库 */
export async function exportAgentFlows(id?: string): Promise<AgentFlowBundle> {
  const qs = id ? `?id=${encodeURIComponent(id)}` : '';
  const data = await request<{ ok: boolean; bundle: AgentFlowBundle }>(`/agent-flows/export${qs}`);
  return data.bundle;
}

/**
 * 导入时的**冲突处理**(B 批 B4)。
 * - `rename`(缺省,不传即此):同内容跳过 / 同 id 异内容分配新 id(二维批次 7a 现状);
 * - `replace`:文件里出现的**同 id** 流程若与本库那份内容不同,则**覆盖本库那份**
 *   (id 不变、引用不破);内容相同仍跳过;库中其它流程一律不动。
 * 未登记的取值由后端 400 拦下(这里只做联合类型收窄)。
 */
export type FlowImportConflict = 'rename' | 'replace';

/**
 * 导入搬运包(二维批次 7a):只新增(同内容跳过 / 同 id 异内容分配新 id),校验失败 400 且库不变。
 * B 批 B4 起可传 `onConflict='replace'` 走**覆盖**模式(**不可逆**:同 id 那份被替换)。
 * `onConflict` **仅非空时下发**——缺省不下发该键,旧客户端请求体逐字节不变。
 */
export async function importAgentFlows(
  flows: AgentFlowConfig[],
  rootId?: string,
  onConflict?: FlowImportConflict,
): Promise<{ library: AgentFlowLibrary; report: FlowImportReport }> {
  const data = await request<{
    ok: boolean;
    library: AgentFlowLibrary;
    report: FlowImportReport;
  }>('/agent-flows/import', {
    method: 'POST',
    body: JSON.stringify({
      flows,
      ...(rootId ? { root_id: rootId } : {}),
      ...(onConflict ? { on_conflict: onConflict } : {}),
    }),
  });
  return { library: data.library, report: data.report };
}
