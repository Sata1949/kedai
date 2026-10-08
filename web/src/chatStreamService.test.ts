import { describe, expect, it, vi } from 'vitest';
import { createChatStreamService } from './chatStreamService';
import type { ChatStreamPayload, SendOutcome, SseEvent, SseHandler } from './api';

/** 假流句柄:控制器 + 事件发射口 + 受理承诺 settle 口(供各用例按需编排) */
function fakeHandle() {
  const abort = vi.fn();
  let emit: SseHandler | undefined;
  let settle: ((outcome: SendOutcome) => void) | undefined;
  const accepted = new Promise<SendOutcome>((resolve) => {
    settle = resolve;
  });
  const handle = { controller: { abort }, accepted };
  const streamChat = vi.fn((_payload: ChatStreamPayload, handler: SseHandler) => {
    emit = handler;
    return handle;
  });
  return {
    abort,
    handle,
    streamChat,
    emit: (e: SseEvent) => emit?.(e),
    settle: (o: SendOutcome) => settle?.(o),
  };
}

describe('createChatStreamService', () => {
  it('start 透传 HTTP 层受理结果(SENDFIX:调用方据此决定是否保留草稿)', async () => {
    const f = fakeHandle();
    f.settle({ accepted: false, status: 409, message: '该会话正在生成中' });
    const service = createChatStreamService({
      streamChat: f.streamChat,
      stopChat: vi.fn().mockResolvedValue({ ok: true }),
    });

    const outcome = await service.start({ message: '你好', agent_mode: 'fast' }, vi.fn());
    expect(outcome).toEqual({ accepted: false, status: 409, message: '该会话正在生成中' });
  });

  it('stop 只通知后端,不中止本地流(SENDFIX-2:等服务端 interrupted 终态,不制造可发送窗口)', async () => {
    const f = fakeHandle();
    const stopChat = vi.fn().mockResolvedValue({ ok: true });
    const service = createChatStreamService({ streamChat: f.streamChat, stopChat });

    service.start({ session_id: 'session-1', message: '你好', agent_mode: 'fast' }, vi.fn());
    await service.stop('session-1');

    expect(stopChat).toHaveBeenCalledWith('session-1');
    expect(f.abort).not.toHaveBeenCalled();
    expect(service.isActive()).toBe(true);
  });

  it('无 sessionId 的 stop 不发后端通知', async () => {
    const f = fakeHandle();
    const stopChat = vi.fn().mockResolvedValue({ ok: true });
    const service = createChatStreamService({ streamChat: f.streamChat, stopChat });

    service.start({ message: '你好', agent_mode: 'fast' }, vi.fn());
    await service.stop(null);

    expect(stopChat).not.toHaveBeenCalled();
  });

  it('abortLocal 中止当前本地流并释放控制器(服务端终态未达时的兜底路径)', () => {
    const f = fakeHandle();
    const service = createChatStreamService({
      streamChat: f.streamChat,
      stopChat: vi.fn().mockResolvedValue({ ok: true }),
    });

    service.start({ message: '你好', agent_mode: 'fast' }, vi.fn());
    service.abortLocal();

    expect(f.abort).toHaveBeenCalledOnce();
    expect(service.isActive()).toBe(false);
  });

  it('收到终态事件后释放控制器(error 同样是终态),后续 stop 不再持有旧流', () => {
    const f = fakeHandle();
    const onEvent = vi.fn();
    const service = createChatStreamService({
      streamChat: f.streamChat,
      stopChat: vi.fn().mockResolvedValue({ ok: true }),
    });

    service.start({ message: '你好', agent_mode: 'fast' }, onEvent);
    f.emit({ type: 'error', code: 'rate_limit', message: '上游限流', retryable: true });

    expect(onEvent).toHaveBeenCalledWith({ type: 'error', code: 'rate_limit', message: '上游限流', retryable: true });
    expect(service.isActive()).toBe(false);
  });

  it('旧流迟到终态不会清理当前新流', () => {
    const first = fakeHandle();
    const second = fakeHandle();
    const handles = [first, second];
    const handlers: SseHandler[] = [];
    const service = createChatStreamService({
      streamChat: vi.fn((_payload: ChatStreamPayload, handler: SseHandler) => {
        handlers.push(handler);
        return handles[handlers.length - 1].handle;
      }),
      stopChat: vi.fn().mockResolvedValue({ ok: true }),
    });

    service.start({ message: '旧', agent_mode: 'fast' }, vi.fn());
    service.start({ message: '新', agent_mode: 'fast' }, vi.fn());
    // 旧流迟到终态:不得清掉当前(新)流的控制器
    handlers[0]({ type: 'interrupted' });

    expect(service.isActive()).toBe(true);
    service.abortLocal();
    expect(second.abort).toHaveBeenCalledOnce();
    expect(first.abort).toHaveBeenCalledOnce(); // 新流 start 时中止了旧流
  });
});
