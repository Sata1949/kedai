// 任务三档显示偏好存储层单测(TMSHOW-3;纯 node,注入内存 StorageLike)。
//
// 这个模块的价值在边界:存储不可用、历史脏值、空任务 id。三种情况都不能抛、
// 不能把「读不到」当成「用户选了某一档」,否则故障会伪装成一个用户偏好。
import { describe, expect, it } from 'vitest';
import {
  DEFAULT_TRANSCRIPT_MODE_KEY,
  readDefaultTaskTranscriptMode,
  readTaskTranscriptOverride,
  transcriptModeKey,
  writeDefaultTaskTranscriptMode,
  writeTaskTranscriptOverride,
  type StorageLike,
} from './taskTranscriptPreference';

/** 内存存储(可预置初始值) */
function memStorage(init: Record<string, string> = {}): StorageLike & { raw: Map<string, string> } {
  const raw = new Map<string, string>(Object.entries(init));
  return {
    raw,
    getItem: (k) => raw.get(k) ?? null,
    setItem: (k, v) => void raw.set(k, v),
  };
}

/** 读写都抛的存储(隐私模式 / 配额满) */
const throwingStorage: StorageLike = {
  getItem() {
    throw new Error('denied');
  },
  setItem() {
    throw new Error('denied');
  },
};

describe('默认档', () => {
  it('未设置时回 full(TMSHOW-2 拍板的默认档,不是 legacy)', () => {
    expect(readDefaultTaskTranscriptMode(memStorage())).toBe('full');
  });

  it('三档写入后各自读回', () => {
    const storage = memStorage();
    for (const mode of ['legacy', 'full', 'condensed'] as const) {
      writeDefaultTaskTranscriptMode(storage, mode);
      expect(readDefaultTaskTranscriptMode(storage)).toBe(mode);
      expect(storage.raw.get(DEFAULT_TRANSCRIPT_MODE_KEY)).toBe(mode);
    }
  });

  it('历史脏值回退 full(白名单校验)', () => {
    for (const junk of ['', 'FULL', 'verbose', '{}', '1']) {
      expect(readDefaultTaskTranscriptMode(memStorage({ [DEFAULT_TRANSCRIPT_MODE_KEY]: junk }))).toBe(
        'full',
      );
    }
  });

  it('存储不可用时不抛,读回 full、写入静默忽略', () => {
    expect(() => readDefaultTaskTranscriptMode(throwingStorage)).not.toThrow();
    expect(readDefaultTaskTranscriptMode(throwingStorage)).toBe('full');
    expect(() => writeDefaultTaskTranscriptMode(throwingStorage, 'condensed')).not.toThrow();
  });
});

describe('单任务覆盖', () => {
  it('键按任务 id 拼接,不同任务互不干扰', () => {
    const storage = memStorage();
    writeTaskTranscriptOverride(storage, 'task-a', 'condensed');
    writeTaskTranscriptOverride(storage, 'task-b', 'legacy');
    expect(readTaskTranscriptOverride(storage, 'task-a')).toBe('condensed');
    expect(readTaskTranscriptOverride(storage, 'task-b')).toBe('legacy');
    expect(transcriptModeKey('task-a')).toBe('kedai.task-transcript-mode.v1.task-a');
  });

  it('未覆盖的任务回 null(与「覆盖值等于默认档」是两件事)', () => {
    const storage = memStorage();
    writeDefaultTaskTranscriptMode(storage, 'condensed');
    expect(readTaskTranscriptOverride(storage, 'task-a')).toBeNull();
    // 覆盖成与默认档相同的值,也不算「未覆盖」
    writeTaskTranscriptOverride(storage, 'task-a', 'condensed');
    expect(readTaskTranscriptOverride(storage, 'task-a')).toBe('condensed');
  });

  it('默认档键不会被当成某个任务的覆盖(前缀不串读)', () => {
    const storage = memStorage();
    writeDefaultTaskTranscriptMode(storage, 'legacy');
    expect(readTaskTranscriptOverride(storage, 'task-a')).toBeNull();
    expect(readDefaultTaskTranscriptMode(storage)).toBe('legacy');
  });

  it('空任务 id 不产生键、不读取', () => {
    const storage = memStorage();
    expect(transcriptModeKey('')).toBe('');
    expect(transcriptModeKey('   ')).toBe('');
    writeTaskTranscriptOverride(storage, '  ', 'legacy');
    expect(storage.raw.size).toBe(0);
    expect(readTaskTranscriptOverride(storage, '')).toBeNull();
  });

  it('历史脏值回退 null(跟随默认,而不是猜一个档位)', () => {
    const storage = memStorage({ 'kedai.task-transcript-mode.v1.task-a': 'verbose' });
    expect(readTaskTranscriptOverride(storage, 'task-a')).toBeNull();
  });

  it('存储不可用时不抛', () => {
    expect(() => readTaskTranscriptOverride(throwingStorage, 'task-a')).not.toThrow();
    expect(readTaskTranscriptOverride(throwingStorage, 'task-a')).toBeNull();
    expect(() => writeTaskTranscriptOverride(throwingStorage, 'task-a', 'legacy')).not.toThrow();
  });
});
