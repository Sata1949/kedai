// chat store 测试:swipeMessage 成功后向卡级脚本沙箱广播 message_swiped(楼层序号)。
// 只 mock api 层与沙箱广播函数,store 内部逻辑(消息加载/替换)走真实代码。
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';

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
  mocked.fetchHistory.mockResolvedValue(HISTORY.map((m) => ({ ...m })));
  mocked.getSessionTotalTokens.mockResolvedValue(0);
  mocked.getGlobalTotalTokens.mockResolvedValue(0);
  mocked.countTokens.mockResolvedValue(0);
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
