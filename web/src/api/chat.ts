// SSE 流式聊天(streamChat)
import { BASE, apiErrorMessage, authorizedFetch, request } from './client';
import { createSseFrameParser } from './sseParser';
import { pumpSseFrames, readErrorParts } from './stream';
import type { AgentMode, SseEvent, TokenUsage } from './types';

export type SseHandler = (event: SseEvent) => void;

/** 一条图像附件(视觉能力包 D2):前端把 File 读成 data URL 提交;
 *  后端落盘后消息 extra 只存引用(`image_refs`),历史渲染走 `GET /api/images/{id}`。 */
export interface ChatAttachment {
  name: string;
  mime: string;
  data_url: string;
}

export interface ChatStreamPayload {
  session_id?: string;
  character_id?: string;
  message: string;
  agent_mode: AgentMode;
  temperature?: number;
  top_p?: number;
  max_tokens?: number;
  /** 重发锚点:服务端严格验证为当前会话最后一条且正文一致的 user 消息。 */
  resend_message_id?: number;
  /** 重生成锚点(阶段六 6f):必须是当前会话最后一条 assistant 消息,生成新版本原地更新该消息。 */
  regenerate_assistant_id?: number;
  /** 手动压缩标记:true 时本轮生成前对较早历史做摘要压缩(仅 compaction_mode=manual 时生效)。 */
  compact?: boolean;
  /** 图像附件(仅常规发送携带;重发/重生成路径服务端拒绝——原图随历史自动携带) */
  attachments?: ChatAttachment[];
}

/** 通知后端中止指定会话的生成任务。 */
export function stopChat(sessionId: string): Promise<{ ok: boolean }> {
  return request('/chat/stop', {
    method: 'POST',
    body: JSON.stringify({ session_id: sessionId }),
  });
}

/** 手动压缩会话历史(阶段借鉴 harness):对较早对话做摘要,原文保留可恢复。 */
export function compactChat(sessionId: string): Promise<{ ok: boolean; compacted: boolean }> {
  return request('/chat/compact', {
    method: 'POST',
    body: JSON.stringify({ session_id: sessionId }),
  });
}

/** 撤销压缩,恢复完整原文历史(阶段借鉴 harness):清除摘要行,原文从未删除。 */
export function clearCompactChat(sessionId: string): Promise<{ ok: boolean; cleared: boolean }> {
  return request('/chat/compact/clear', {
    method: 'POST',
    body: JSON.stringify({ session_id: sessionId }),
  });
}

/** 空 usage(错误分支兜底:保证 finish 事件结构完整,store 能安全复位) */
function emptyUsage(): TokenUsage {
  return { prompt_tokens: 0, completion_tokens: 0, total_tokens: 0, context_tokens: 0, prompt_cache_hit_tokens: 0, prompt_cache_miss_tokens: 0 };
}

/** 发送受理结果(SENDFIX):`accepted=false` = 本轮消息未被服务端受理(HTTP 非 2xx / 网络错误),
 *  调用方应保留用户草稿(输入框不清空)并展示错误条。 */
export interface SendOutcome {
  accepted: boolean;
  status?: number;
  code?: string;
  message?: string;
}

export interface ChatStreamHandle {
  controller: AbortController;
  /** HTTP 层是否受理(2xx 且拿到事件流);在响应到达时 settle,不等待生成结束 */
  accepted: Promise<SendOutcome>;
}

/** 可重试判定(与后端 llm_error 分类同口径的粗判):限流/超时/服务端错误可重试,参数/鉴权类不可 */
function retryableStatus(status: number): boolean {
  return status === 408 || status === 425 || status === 429 || status >= 500;
}

export interface SseParser {
  push(chunk: Uint8Array): void;
  /** 喂入**已解出的 data 文本**(与 push 二选一):供共享读循环 stream.ts 复用同一解析器 */
  pushData(data: string): void;
  finish(): boolean;
}

/**
 * 聊天流 SSE 解析器:帧解析委托 sseParser.ts 共享层(兼容 CRLF/LF、跨 chunk 分隔符和
 * UTF-8,多 data 行按规范以换行拼接),本层只保留聊天专属语义:
 * - data 文本 JSON.parse 为 SseEvent 后回调(单个坏事件不阻断后续);
 * - 终态判定:finish / interrupted / error(error 也是服务端错误终态)收到后,
 *   finish() 返回 true,streamChat 不再补合成 finish。
 */
export function createSseParser(onEvent: SseHandler): SseParser {
  let terminalReceived = false;

  /** 单帧 data 文本 → 聊天事件(坏帧跳过,不阻断后续);终态在此登记 */
  const handleData = (data: string): void => {
    try {
      const event = JSON.parse(data) as SseEvent;
      onEvent(event);
      // error 也是终态事件(服务端错误终态):收到后不再补合成 finish
      if (event.type === 'finish' || event.type === 'interrupted' || event.type === 'error') terminalReceived = true;
    } catch {
      // 单个坏事件不应阻断后续事件。
    }
  };

  const frames = createSseFrameParser(handleData);

  return {
    push: (chunk) => frames.push(chunk),
    pushData: handleData,
    finish: () => {
      frames.finish();
      return terminalReceived;
    },
  };
}

/**
 * 发起 Agent 聊天流;返回中止控制器与「HTTP 层是否受理」的承诺(SENDFIX)。
 *
 * 失败口径(2026-10-08 SENDFIX-1):HTTP 非 2xx / 网络异常 / 流未收终态即结束,
 * 一律发 **error 终态事件**(携带服务端文案与可重试位),不再合成「step + 空 finish」——
 * 旧口径把错误塞进 step 后即被 finish 覆盖,再被历史回拉擦除,界面表现为「什么都没发生」。
 */
export function streamChat(
  payload: ChatStreamPayload,
  onEvent: SseHandler,
): ChatStreamHandle {
  const controller = new AbortController();
  let settleAccepted: (outcome: SendOutcome) => void = () => {};
  const accepted = new Promise<SendOutcome>((resolve) => {
    settleAccepted = resolve;
  });
  void (async () => {
    try {
      const res = await authorizedFetch(`${BASE}/chat/send`, {
        method: 'POST',
        body: JSON.stringify(payload),
        signal: controller.signal,
      });
      if (!res.ok || !res.body) {
        // 错误口径与 request() 一致:按结构化 code 分类(见 client.ts apiErrorMessage),
        // 错误体解析与 tasks 流共用 stream.ts 的单点实现。
        const { status, code, detail } = await readErrorParts(res);
        const message = apiErrorMessage(status, code, detail);
        settleAccepted({ accepted: false, status, code, message });
        onEvent({
          type: 'error',
          code: code || `http_${status}`,
          message,
          retryable: retryableStatus(status),
        });
        return;
      }
      settleAccepted({ accepted: true, status: res.status });
      // 读循环与帧解析走共享底座(stream.ts),终态判定仍由本层的 createSseParser 负责
      const parser = createSseParser(onEvent);
      await pumpSseFrames(res, (data) => parser.pushData(data));
      if (!parser.finish()) {
        // 流在终态前结束:消息可能已受理,但本轮没有结果——必须让用户看见并可重试
        onEvent({
          type: 'error',
          code: 'stream_interrupted',
          message: '连接中断:流在收到终态事件前结束',
          retryable: true,
        });
      }
    } catch (e) {
      // 网络异常:请求可能未送达(accepted 已按未受理处理,调用方保留草稿);
      // AbortError 是用户主动停止/上层切换流,由 interrupted 语义处理,不发事件。
      if ((e as Error).name !== 'AbortError') {
        const message = (e as Error).message;
        settleAccepted({ accepted: false, message });
        onEvent({
          type: 'error',
          code: 'network_error',
          message: `网络错误:${message}`,
          retryable: true,
        });
      }
    }
  })();
  return { controller, accepted };
}
