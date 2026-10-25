// 聊天流 / SSE 事件域类型(对齐后端 SseEvent 线格式)。

import type { TaskEvent } from './task';
import type { ToolRisk } from './tools';

export interface TokenUsage {
  prompt_tokens: number;
  completion_tokens: number;
  total_tokens: number;
  context_tokens: number;
  /** prompt 缓存命中 token(DeepSeek 等提供商;无缓存字段时为 0),用于计算当前命中率 */
  prompt_cache_hit_tokens: number;
  /** prompt 缓存未命中 token(命中 + 未命中 = prompt_tokens) */
  prompt_cache_miss_tokens: number;
}

export type SseEvent =
  | { type: 'token'; text: string }
  /** 隐藏草稿(deep/agent 流程第 1 步产物,RPFLOW):不进消息气泡,仅 Agent 面板折叠展示 */
  | { type: 'draft'; text: string }
  | { type: 'step'; step: string; detail?: string; index?: number; total?: number }
  | { type: 'tool_call'; name: string; input: unknown; call_id?: string; render_kind?: string; origin?: string }
  | { type: 'tool_authorization_required'; name: string; risk: ToolRisk; reason: string; run_id: string; call_id: string; origin?: string }
  | { type: 'tool_result'; name: string; output: unknown; call_id?: string; render_kind?: string; origin?: string; snapshot_id?: string }
  | { type: 'vars'; stat_data: Record<string, unknown> }
  | { type: 'interrupted' }
  /** 上游请求重试中(HB-4):非终态提示事件,成功后照常继续 token/finish */
  | { type: 'retry'; attempt: number; max: number; reason: string }
  | { type: 'error'; code: string; message: string; retryable: boolean }
  | { type: 'finish'; usage: TokenUsage; content: string; finish_reason?: string }
  | TaskEvent;
