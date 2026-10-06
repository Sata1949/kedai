import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';

// node 环境无 localStorage,补内存桩(与 storeFacade.test.ts 同款)
const memStorage = new Map<string, string>();
vi.stubGlobal('localStorage', {
  getItem: (k: string) => memStorage.get(k) ?? null,
  setItem: (k: string, v: string) => void memStorage.set(k, String(v)),
  removeItem: (k: string) => void memStorage.delete(k),
  clear: () => memStorage.clear(),
  key: (i: number) => [...memStorage.keys()][i] ?? null,
  get length() { return memStorage.size; },
});

// mock 整个 api 层:本测试只关心「重进时恢复上次角色/会话」的选择逻辑
vi.mock('../api', () => ({
  listCharacters: vi.fn(),
  fetchInitVars: vi.fn(),
  listSessions: vi.fn(),
  fetchHistory: vi.fn(),
  createSession: vi.fn(),
  getSessionTotalTokens: vi.fn(),
  getGlobalTotalTokens: vi.fn(),
  countTokens: vi.fn(),
  deleteCharacter: vi.fn(),
  uploadCharacter: vi.fn(),
  updateCharacter: vi.fn(),
  streamChat: vi.fn(),
  stopChat: vi.fn(),
  // 卡级脚本收集/授权哈希/initvar 兜底经 fetchCharacterDetail 拉详情;拒绝=无详情(不缓存,下
  // 次重试),selectCharacter 后台预拉静默退化为纯 regex 哈希
  getCharacter: vi.fn().mockRejectedValue(new Error('测试环境无详情')),
}));

import * as api from '../api';
import { useCharacterStore } from './character';
import { useChatStore } from './chat';
import { writeLastCharacterId, writeLastSessionId, readLastSessionId, readLastCharacterId } from '../lastPosition';

const mocked = api as unknown as Record<string, ReturnType<typeof vi.fn>>;

const CHAR_1 = { id: 'c1', chara_name: '最新角色', name: 'new', description: '', first_mes: '', alternate_greetings: [], regex_scripts: [] };
const CHAR_2 = { id: 'c2', chara_name: '上次角色', name: 'old', description: '', first_mes: '', alternate_greetings: [], regex_scripts: [] };

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

/** 让微任务跑完(FE-1 时序控制用) */
async function flush(): Promise<void> {
  await new Promise((r) => setTimeout(r, 0));
}

beforeEach(() => {
  memStorage.clear();
  vi.clearAllMocks();
  setActivePinia(createPinia());
  mocked.listCharacters.mockResolvedValue([CHAR_1, CHAR_2]); // 列表按 created_at DESC:c1 最新
  mocked.fetchInitVars.mockResolvedValue({});
  mocked.listSessions.mockImplementation(async (cid: string) =>
    cid === 'c2'
      ? [{ id: 's8', title: '较近会话' }, { id: 's9', title: '上次会话' }]
      : [{ id: 's1', title: 'c1 会话' }],
  );
  mocked.fetchHistory.mockResolvedValue([]);
  mocked.getSessionTotalTokens.mockResolvedValue(0);
  mocked.getGlobalTotalTokens.mockResolvedValue(0);
  mocked.countTokens.mockResolvedValue(0);
});

describe('重进恢复上次浏览位置(修复「重进后聊天记录丢失」UX 主因)', () => {
  it('loadCharacters 恢复上次选中的角色与该角色的上次会话,而非落在最新角色', async () => {
    writeLastCharacterId('c2');
    writeLastSessionId('c2', 's9');

    const character = useCharacterStore();
    await character.loadCharacters();

    expect(character.currentCharacterId).toBe('c2');
    expect(useChatStore().currentSessionId).toBe('s9');
    expect(mocked.fetchHistory).toHaveBeenCalledWith('s9');
  });

  it('上次角色已删除:回退到列表首位并刷新记忆', async () => {
    writeLastCharacterId('deleted-char');

    const character = useCharacterStore();
    await character.loadCharacters();

    expect(character.currentCharacterId).toBe('c1');
    expect(readLastCharacterId()).toBe('c1');
  });

  it('该角色上次会话已删除:回退到最近会话并刷新记忆', async () => {
    writeLastCharacterId('c2');
    writeLastSessionId('c2', 'deleted-session');

    const character = useCharacterStore();
    await character.loadCharacters();

    expect(useChatStore().currentSessionId).toBe('s8'); // sessions[0](最近)
    expect(readLastSessionId('c2')).toBe('s8');
  });

  it('切换会话时写入记忆,供下次重进恢复', async () => {
    const character = useCharacterStore();
    await character.loadCharacters(); // 无记忆 → 落在 c1
    await character.selectCharacter('c2');
    const chat = useChatStore();
    expect(chat.currentSessionId).toBe('s8');

    await chat.switchSession('s9');
    expect(readLastSessionId('c2')).toBe('s9');
    expect(readLastCharacterId()).toBe('c2');
  });
});

describe('FE-1 迟到响应守卫(切角色不丢新选择)', () => {
  it('切角色 c1→c2:c1 的会话列表迟到到达不覆盖 c2', async () => {
    const late = deferred<Array<{ id: string; title: string }>>();
    mocked.listSessions.mockImplementation((cid: string) =>
      cid === 'c1' ? late.promise : Promise.resolve([{ id: 's2', title: 'c2 会话' }]),
    );
    const character = useCharacterStore();

    const p1 = character.selectCharacter('c1');
    await flush(); // 让 c1 的加载推进到 listSessions('c1') 挂起
    await character.selectCharacter('c2'); // 切走并完成 c2 的加载
    const chat = useChatStore();
    expect(chat.currentSessionId).toBe('s2');
    expect(chat.sessions.map((s) => s.id)).toEqual(['s2']);

    late.resolve([{ id: 's1', title: 'c1 会话' }]); // c1 的旧响应迟到
    await p1;

    expect(character.currentCharacterId).toBe('c2');
    expect(chat.sessions.map((s) => s.id), '迟到会话列表不得覆盖新角色').toEqual(['s2']);
    expect(chat.currentSessionId, '迟到加载不得改写当前会话').toBe('s2');
    expect(readLastSessionId('c2'), '记忆不得被旧角色改写').toBe('s2');
  });

  it('切角色 c1→c2:c1 的初始变量迟到到达不污染 c2', async () => {
    const late = deferred<Record<string, string>>();
    mocked.fetchInitVars.mockImplementation((cid: string) => (cid === 'c1' ? late.promise : Promise.resolve({})));
    const character = useCharacterStore();

    const p1 = character.selectCharacter('c1');
    await character.selectCharacter('c2');
    late.resolve({ '[InitVar]旧角色变量': 'x' }); // c1 的旧响应迟到
    await p1;

    expect(useChatStore().initVarEntries, '迟到初始变量不得覆盖新角色').toEqual({});
  });

  it('正常路径:selectCharacter 仍恢复该角色上次会话(守卫不误伤)', async () => {
    writeLastSessionId('c2', 's9');
    const character = useCharacterStore();
    await character.loadCharacters(); // 无角色记忆 → 落在 c1
    expect(character.currentCharacterId).toBe('c1');

    await character.selectCharacter('c2');
    expect(useChatStore().currentSessionId).toBe('s9');
    expect(mocked.fetchHistory).toHaveBeenCalledWith('s9');
    expect(readLastSessionId('c2')).toBe('s9');
  });
});
