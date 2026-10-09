// chat store 测试:swipeMessage 成功后向卡级脚本沙箱广播 message_swiped(楼层序号)。
// 只 mock api 层与沙箱广播函数,store 内部逻辑(消息加载/替换)走真实代码。
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { nextTick } from 'vue';

// node 环境无 localStorage,补内存桩(与 character.test.ts 同款)
const memStorage = new Map<string, string>();
vi.stubGlobal('localStorage', {
  getItem: (k: string) => memStorage.get(k) ?? null,
  setItem: (k: string, v: string) => void memStorage.set(k, String(v)),
  removeItem: (k: string) => void memStorage.delete(k),
  clear: () => memStorage.clear(),
  key: (i: number) => [...memStorage.keys()][i] ?? null,
  get length() { return memStorage.size; },
});

vi.mock('../api', () => ({
  fetchHistory: vi.fn(),
  fetchAgentTrace: vi.fn(),
  swipeMessage: vi.fn(),
  getSessionTotalTokens: vi.fn(),
  getGlobalTotalTokens: vi.fn(),
  countTokens: vi.fn(),
  streamChat: vi.fn(),
  stopChat: vi.fn(),
}));

vi.mock('../characterScriptSandbox', () => ({
  broadcastMvuUpdate: vi.fn(),
  broadcastCardEvent: vi.fn(),
}));

import * as api from '../api';
import { broadcastCardEvent } from '../characterScriptSandbox';
import { registerCharacterIdProvider } from './storeBridge';
import { useChatStore } from './chat';

const mocked = api as unknown as Record<string, ReturnType<typeof vi.fn>>;
const mockedBroadcast = broadcastCardEvent as unknown as ReturnType<typeof vi.fn>;

const HISTORY = [
  {
    id: 5,
    role: 'assistant',
    content: '开场v0',
    extra: {
      swipes: [
        { swipe_id: 0, content: '开场v0', ts: 1 },
        { swipe_id: 1, content: '开场v1', ts: 2 },
      ],
      swipe_id: 0,
    },
  },
  { id: 6, role: 'user', content: '你好', extra: {} },
];

/** 历史消息最小形状(FE-1 迟到响应测试用) */
type HistoryMsg = { id: number; role: string; content: string; extra: Record<string, unknown> };

/** 手工控制时序的 Promise(FE-1 迟到响应测试用) */
function deferred<T>(): { promise: Promise<T>; resolve: (v: T) => void } {
  let resolveFn: ((v: T) => void) | null = null;
  const promise = new Promise<T>((res) => {
    resolveFn = res;
  });
  return {
    promise,
    resolve: (v: T) => {
      if (resolveFn) resolveFn(v);
    },
  };
}

/** 让 void 发出的异步刷新(loadTokenTotals / updateContextTokens)跑完 */
async function flush(): Promise<void> {
  await new Promise((r) => setTimeout(r, 0));
}

beforeEach(() => {
  memStorage.clear();
  vi.clearAllMocks();
  setActivePinia(createPinia());
  // 角色 id 经 storeBridge 提供(chat 的发送守卫依赖它;未注册时降级为 null,发送会被静默拦下)
  registerCharacterIdProvider(() => 'c1');
  mocked.fetchHistory.mockResolvedValue(HISTORY.map((m) => ({ ...m })));
  mocked.fetchAgentTrace.mockResolvedValue(null);
  mocked.getSessionTotalTokens.mockResolvedValue(0);
  mocked.getGlobalTotalTokens.mockResolvedValue(0);
  mocked.countTokens.mockResolvedValue(0);
  mocked.stopChat.mockResolvedValue({ ok: true });
});

describe('swipeMessage 广播(卡级脚本事件流)', () => {
  it('切换成功后广播 message_swiped,payload 为 0-based 楼层序号', async () => {
    mocked.swipeMessage.mockResolvedValue({ content: '开场v1', swipe_id: 1 });
    const store = useChatStore();
    await store.switchSession('s1');
    expect(store.messages).toHaveLength(2);

    await store.swipeMessage(5, 1);

    // 消息内容已切换
    expect(store.messages[0].content).toBe('开场v1');
    expect(store.messages[0].extra.swipe_id).toBe(1);
    // 广播:事件名 + 楼层序号(消息 id=5 位于数组下标 0;舰娘卡脚本据此判定第一条开场白)
    expect(mockedBroadcast).toHaveBeenCalledTimes(1);
    expect(mockedBroadcast).toHaveBeenCalledWith('message_swiped', 0);
  });

  it('中间楼层广播其数组下标(非消息 id)', async () => {
    mocked.swipeMessage.mockResolvedValue({ content: 'vX', swipe_id: 2 });
    const store = useChatStore();
    await store.switchSession('s1');

    await store.swipeMessage(6, 2);
    expect(mockedBroadcast).toHaveBeenCalledWith('message_swiped', 1);
  });

  it('后端失败不广播', async () => {
    mocked.swipeMessage.mockRejectedValue(new Error('网络错误'));
    const store = useChatStore();
    await store.switchSession('s1');

    await store.swipeMessage(5, 1);
    expect(mockedBroadcast).not.toHaveBeenCalled();
  });
});

describe('FE-1 迟到响应守卫(切会话不丢新选择)', () => {
  it('切会话 A→B:A 的历史迟到到达不覆盖 B', async () => {
    const late = deferred<HistoryMsg[]>();
    mocked.fetchHistory.mockImplementation((sid: string) =>
      sid === 'A' ? late.promise : Promise.resolve([{ id: 9, role: 'user', content: 'B 的消息', extra: {} }]),
    );
    const store = useChatStore();

    const pA = store.switchSession('A'); // 挂起在 fetchHistory('A')
    await store.switchSession('B'); // 切到 B 并完成加载
    expect(store.currentSessionId).toBe('B');

    late.resolve([{ id: 5, role: 'assistant', content: 'A 的旧消息', extra: {} }]); // A 的旧响应迟到
    await pA;

    expect(store.currentSessionId).toBe('B');
    expect(store.messages.map((m) => m.content), '迟到历史不得覆盖当前会话').toEqual(['B 的消息']);
  });

  it('正常路径:单次切换仍刷新历史(守卫不误伤)', async () => {
    mocked.fetchHistory.mockResolvedValue([{ id: 7, role: 'user', content: '正常历史', extra: {} }]);
    const store = useChatStore();
    await store.switchSession('s1');
    expect(store.messages.map((m) => m.content)).toEqual(['正常历史']);
  });

  it('切会话后旧会话的累计 token 迟到到达不覆盖新会话', async () => {
    const late = deferred<number>();
    mocked.getSessionTotalTokens.mockImplementation((sid: string) => (sid === 'A' ? late.promise : Promise.resolve(11)));
    const store = useChatStore();

    await store.switchSession('A');
    await store.switchSession('B');
    await flush();
    expect(store.sessionTotalTokens).toBe(11);

    late.resolve(99); // 旧响应迟到
    await flush();
    expect(store.sessionTotalTokens, '迟到累计值属于旧会话,应丢弃').toBe(11);
  });

  it('切会话后旧会话的上下文计数迟到到达不覆盖新会话', async () => {
    const late = deferred<number>();
    mocked.fetchHistory.mockImplementation((sid: string) =>
      Promise.resolve([{ id: 1, role: 'user', content: sid, extra: {} }]),
    );
    mocked.countTokens.mockImplementation((msgs: HistoryMsg[]) => (msgs[0]?.content === 'A' ? late.promise : Promise.resolve(7)));
    const store = useChatStore();

    await store.switchSession('A');
    await store.switchSession('B');
    await flush();
    expect(store.contextTokens).toBe(7);

    late.resolve(999); // 旧响应迟到
    await flush();
    expect(store.contextTokens, '迟到计数属于旧会话消息集,应丢弃').toBe(7);
  });
});

describe('SENDFIX-1 失败可见性(错误条 + trace 恢复尊重 error 态)', () => {
  it('error 终态落错误条,成功 finish 清除', async () => {
    const store = useChatStore();
    await store.switchSession('s1');

    store.onSseEvent({ type: 'error', code: 'rate_limit', message: '上游限流', retryable: true });
    expect(store.lastError).toMatchObject({ code: 'rate_limit', message: '上游限流', retryable: true, rejected: false });
    expect(store.agent.phase).toBe('error');

    store.onSseEvent({
      type: 'finish',
      usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2, context_tokens: 1, prompt_cache_hit_tokens: 0, prompt_cache_miss_tokens: 0 },
      content: 'ok',
    });
    expect(store.lastError).toBeNull();
  });

  it('历史回拉不得把已落 error 的终态擦成空闲(旧判据:工具列表为空即 idleAgent)', async () => {
    const store = useChatStore();
    await store.switchSession('s1');
    // 本地未落库草稿在场:error 终态会削草稿并请求回拉历史(从而触发 restoreAgentTrace)
    store.messages.push({ id: -1, role: 'user', content: '草稿', extra: {}, streaming: false });
    mocked.fetchAgentTrace.mockResolvedValue({ state: 'error', tool_calls: [], plan: [], step_index: 0 });

    store.onSseEvent({ type: 'error', code: 'rate_limit', message: '上游限流', retryable: true });
    await flush();
    await flush();

    expect(store.agent.phase, '错误态被历史回拉擦除=界面表现为「消息发出后什么都没发生」').toBe('error');
    expect(store.agent.stepText).toBe('执行出错');
  });

  it('在途残留态(executing 且无工具调用)仍空闲化,不出现幽灵「执行中」', async () => {
    mocked.fetchAgentTrace.mockResolvedValue({ state: 'executing', tool_calls: [], plan: [], step_index: 0 });
    const store = useChatStore();
    await store.switchSession('s1');
    await flush();
    await flush();
    expect(store.agent.phase).toBe('idle');
  });

  it('HTTP 未受理(409):sendMessage 返回未受理,错误条标「发送失败」且不给重试按钮', async () => {
    mocked.streamChat.mockImplementation((_p: unknown, handler: (e: api.SseEvent) => void) => {
      handler({ type: 'error', code: 'http_409', message: '该会话正在生成中', retryable: true });
      return { controller: { abort: vi.fn() }, accepted: Promise.resolve({ accepted: false, status: 409, message: '该会话正在生成中' }) };
    });
    const store = useChatStore();
    await store.switchSession('s1');

    const outcome = await store.sendMessage('你好');
    expect(outcome.accepted).toBe(false);
    expect(store.lastError).toMatchObject({ rejected: true, retryable: false, message: '该会话正在生成中' });
  });

  it('受理成功时 sendMessage 返回 accepted,不产生错误条', async () => {
    mocked.streamChat.mockReturnValue({ controller: { abort: vi.fn() }, accepted: Promise.resolve({ accepted: true, status: 200 }) });
    const store = useChatStore();
    await store.switchSession('s1');

    const outcome = await store.sendMessage('你好');
    expect(outcome.accepted).toBe(true);
    expect(store.lastError).toBeNull();
  });
});

describe('RPFLOW 聊天档位持久化(deep/agent/custom 不静默回落 fast)', () => {
  // 键名是**持久化格式契约**(与 store 内 AGENT_MODE_KEY 同源):升级改名即丢档位,
  // 故在测试里钉住字面量,防止单侧漂移。
  const MODE_KEY = 'kedai.agent-mode.v1';

  it('无持久化值时默认 fast', () => {
    expect(useChatStore().agentMode).toBe('fast');
  });

  it('非法持久化值(损坏/旧版本)回退 fast,不抛错', () => {
    memStorage.set(MODE_KEY, 'super-deep');
    expect(useChatStore().agentMode).toBe('fast');
  });

  it('持久化值在 store 创建时还原(重启不回落 fast)', () => {
    memStorage.set(MODE_KEY, 'agent');
    expect(useChatStore().agentMode).toBe('agent');
    memStorage.set(MODE_KEY, 'deep');
    setActivePinia(createPinia());
    expect(useChatStore().agentMode).toBe('deep');
  });

  it('档位变更写回 localStorage(setAgentMode 后重读可还原)', async () => {
    const store = useChatStore();
    store.setAgentMode('custom');
    await nextTick();
    expect(memStorage.get(MODE_KEY)).toBe('custom');

    // 模拟「下次启动」:新 pinia 实例读同一份存储
    setActivePinia(createPinia());
    expect(useChatStore().agentMode).toBe('custom');
  });
});

describe('SENDFIX-2 停止竞态(停止中过渡态)', () => {
  it('stop 后保持生成态直到服务端 interrupted 到达(旧实现立即复位=可发送窗口)', async () => {
    vi.useFakeTimers();
    try {
      mocked.streamChat.mockReturnValue({ controller: { abort: vi.fn() }, accepted: Promise.resolve({ accepted: true, status: 200 }) });
      const store = useChatStore();
      await store.switchSession('s1');
      await store.sendMessage('你好');
      expect(store.generating).toBe(true);

      store.stop();
      expect(store.stopping).toBe(true);
      expect(store.generating, '停止中必须保持生成态,否则窗口期重发被后端 409 静默吞掉').toBe(true);

      store.onSseEvent({ type: 'interrupted' });
      expect(store.stopping).toBe(false);
      expect(store.generating).toBe(false);
    } finally {
      vi.useRealTimers();
    }
  });

  it('服务端终态未达:兜底计时器到点后本地收尾并中止本地流', async () => {
    vi.useFakeTimers();
    const abort = vi.fn();
    try {
      mocked.streamChat.mockReturnValue({ controller: { abort }, accepted: Promise.resolve({ accepted: true, status: 200 }) });
      const store = useChatStore();
      await store.switchSession('s1');
      await store.sendMessage('你好');

      store.stop();
      expect(store.stopping).toBe(true);

      await vi.advanceTimersByTimeAsync(6000);
      expect(abort).toHaveBeenCalled();
      expect(store.stopping).toBe(false);
      expect(store.generating).toBe(false);
    } finally {
      vi.useRealTimers();
    }
  });
});
