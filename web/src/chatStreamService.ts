import type { ChatStreamPayload, SendOutcome, SseEvent, SseHandler } from './api';

interface StreamController {
  abort(): void;
}

export interface ChatStreamDependencies {
  streamChat(payload: ChatStreamPayload, onEvent: SseHandler): { controller: StreamController; accepted: Promise<SendOutcome> };
  stopChat(sessionId: string): Promise<{ ok: boolean }>;
}

export interface ChatStreamService {
  /** 发起一轮流;返回 HTTP 层受理结果(未受理时调用方保留用户草稿) */
  start(payload: ChatStreamPayload, onEvent: SseHandler): Promise<SendOutcome>;
  /** 请求后端停止生成;本地流保持打开,等服务端 interrupted 终态(不再立即 abort) */
  stop(sessionId?: string | null): Promise<void>;
  /** 兜底中止本地流(服务端终态迟迟未达时由上层收尾逻辑调用) */
  abortLocal(): void;
  isActive(): boolean;
}

/**
 * 管理单条聊天流的浏览器控制器、后端停止通知和终态释放。
 * store 只负责业务状态，不再持有 AbortController 生命周期。
 *
 * 停止语义(2026-10-08 SENDFIX-2):`stop` **只**通知后端,不 abort 本地流——
 * 旧实现「通知后端后立即 abort」会把服务端的 interrupted 终态一起掐断,迫使前端在
 * 本地合成中断事件、立刻复位生成态:由此产生约 2 秒的可发送窗口,窗口内重发会被后端
 * 以 409「该会话正在生成中」拒掉,消息与输入被静默吞掉。现在由服务端终态驱动复位,
 * 服务端异常时再由上层兜底计时器调 `abortLocal` 收尾。
 */
export function createChatStreamService(deps: ChatStreamDependencies): ChatStreamService {
  let activeController: StreamController | null = null;

  function start(payload: ChatStreamPayload, onEvent: SseHandler): Promise<SendOutcome> {
    activeController?.abort();
    let startedController: StreamController | null = null;
    const handleEvent = (event: SseEvent): void => {
      onEvent(event);
      if ((event.type === 'finish' || event.type === 'interrupted' || event.type === 'error')
        && activeController === startedController) {
        activeController = null;
      }
    };
    const handle = deps.streamChat(payload, handleEvent);
    startedController = handle.controller;
    activeController = startedController;
    return handle.accepted;
  }

  async function stop(sessionId?: string | null): Promise<void> {
    if (!sessionId) return;
    // 只做后端通知:本地流继续接收,等 interrupted 终态统一收尾
    await deps.stopChat(sessionId);
  }

  function abortLocal(): void {
    const controller = activeController;
    activeController = null;
    controller?.abort();
  }

  return {
    start,
    stop,
    abortLocal,
    isActive: () => activeController !== null,
  };
}
