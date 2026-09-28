// 首启引导机制层单测(纯 node:注入内存 StorageLike,不需要 jsdom)。
// 覆盖四类判定:首启识别、偏好读取、启动模式解析、步骤表生成。
import { describe, expect, it } from 'vitest';
import {
  DEFAULT_APP_MODE_KEY,
  ONBOARDING_KEY,
  buildTutorialPlan,
  planBranches,
  readDefaultAppMode,
  readOnboarding,
  resolveLaunchMode,
  shouldShowOnboarding,
  writeDefaultAppMode,
  writeOnboarding,
  type AppMode,
  type StorageLike,
} from './onboarding';

/** 内存 StorageLike(测试桩;可预置初始值) */
function memStorage(init: Record<string, string> = {}): StorageLike & { raw: Map<string, string> } {
  const raw = new Map<string, string>(Object.entries(init));
  return {
    raw,
    getItem: (k) => raw.get(k) ?? null,
    setItem: (k, v) => void raw.set(k, v),
  };
}

/** 会抛的 StorageLike(模拟隐私模式/配额满:读写都不可用) */
const throwingStorage: StorageLike = {
  getItem() { throw new Error('denied'); },
  setItem() { throw new Error('denied'); },
};

describe('首启识别 shouldShowOnboarding', () => {
  it('无键 = 首次使用,需要弹引导', () => {
    expect(shouldShowOnboarding(memStorage())).toBe(true);
  });

  it('已完成(done: true)= 不再弹', () => {
    const s = memStorage({ [ONBOARDING_KEY]: JSON.stringify({ version: 1, done: true, preferredMode: 'task' }) });
    expect(shouldShowOnboarding(s)).toBe(false);
  });

  it('done 为 false / 缺字段 = 未走过,需要弹', () => {
    expect(shouldShowOnboarding(memStorage({ [ONBOARDING_KEY]: JSON.stringify({ done: false }) }))).toBe(true);
    expect(shouldShowOnboarding(memStorage({ [ONBOARDING_KEY]: JSON.stringify({ version: 1 }) }))).toBe(true);
  });

  it('值损坏(非 JSON / 非对象)按未走过处理,只多弹一次', () => {
    expect(shouldShowOnboarding(memStorage({ [ONBOARDING_KEY]: '{不是 JSON' }))).toBe(true);
    expect(shouldShowOnboarding(memStorage({ [ONBOARDING_KEY]: '"字符串"' }))).toBe(true);
  });

  it('存储不可用(getItem 抛)按未走过处理,不抛异常', () => {
    expect(shouldShowOnboarding(throwingStorage)).toBe(true);
  });
});

describe('引导状态读取 readOnboarding', () => {
  it('preferredMode 非法回退 roleplay;tutorials 过滤非法项并保序去重', () => {
    const s = memStorage({
      [ONBOARDING_KEY]: JSON.stringify({
        version: 1,
        done: true,
        preferredMode: 'unknown-mode',
        tutorials: ['task', 'task', 'bogus', 'roleplay'],
        completedAt: '2026-09-27T00:00:00.000Z',
      }),
    });
    expect(readOnboarding(s)).toEqual({
      version: 1,
      done: true,
      preferredMode: 'roleplay',
      tutorials: ['task', 'roleplay'],
      completedAt: '2026-09-27T00:00:00.000Z',
    });
  });

  it('tutorials 非数组时归一为空数组(不抛)', () => {
    const s = memStorage({
      [ONBOARDING_KEY]: JSON.stringify({ version: 1, done: true, preferredMode: 'task', tutorials: 'task' }),
    });
    expect(readOnboarding(s)?.tutorials).toEqual([]);
  });
});

describe('默认模式偏好', () => {
  it('未设 = null(回退粘性记忆)', () => {
    expect(readDefaultAppMode(memStorage())).toBeNull();
  });

  it('值合法时原样读出', () => {
    expect(readDefaultAppMode(memStorage({ [DEFAULT_APP_MODE_KEY]: 'task' }))).toBe('task');
    expect(readDefaultAppMode(memStorage({ [DEFAULT_APP_MODE_KEY]: 'roleplay' }))).toBe('roleplay');
  });

  it('值非法 = null;存储不可用 = null(均不抛)', () => {
    expect(readDefaultAppMode(memStorage({ [DEFAULT_APP_MODE_KEY]: 'TASK' }))).toBeNull();
    expect(readDefaultAppMode(memStorage({ [DEFAULT_APP_MODE_KEY]: '' }))).toBeNull();
    expect(readDefaultAppMode(throwingStorage)).toBeNull();
  });

  it('写入后可读回', () => {
    const s = memStorage();
    writeDefaultAppMode(s, 'task');
    expect(readDefaultAppMode(s)).toBe('task');
  });
});

describe('启动模式解析 resolveLaunchMode', () => {
  it('偏好优先于粘性记忆', () => {
    expect(resolveLaunchMode('roleplay', 'task')).toBe('roleplay');
    expect(resolveLaunchMode('task', 'roleplay')).toBe('task');
  });

  it('无偏好时回退粘性记忆的既有语义', () => {
    expect(resolveLaunchMode(null, 'task')).toBe('task');
    expect(resolveLaunchMode(null, 'roleplay')).toBe('roleplay');
  });

  it('无偏好且粘性值缺失/异常 = roleplay(与改动前逐字一致)', () => {
    expect(resolveLaunchMode(null, null)).toBe('roleplay');
    expect(resolveLaunchMode(null, '')).toBe('roleplay');
    expect(resolveLaunchMode(null, 'bogus')).toBe('roleplay');
  });
});

describe('教程步骤表 buildTutorialPlan', () => {
  const bothRoleplayFirst = buildTutorialPlan({ preference: 'roleplay', scope: 'both', isAndroid: false });

  it('不需要 = 空表', () => {
    expect(buildTutorialPlan({ preference: 'roleplay', scope: 'none', isAndroid: false })).toEqual([]);
    expect(buildTutorialPlan({ preference: 'task', scope: 'none', isAndroid: true })).toEqual([]);
  });

  it('只看喜欢的 = 单分支', () => {
    const p = buildTutorialPlan({ preference: 'task', scope: 'preferred', isAndroid: false });
    expect(p.every((id) => id.startsWith('t-') || id.startsWith('s-'))).toBe(true);
    expect(p).toContain('s-enter');
    expect(p.some((id) => id.startsWith('r-'))).toBe(false);
  });

  it('都需要时角色扮演分支在前(偏好 = 角色扮演)', () => {
    expect(bothRoleplayFirst.findIndex((id) => id === 'r-intro')).toBeLessThan(
      bothRoleplayFirst.findIndex((id) => id === 't-switch'),
    );
  });

  it('都需要时任务分支在前(偏好 = 任务)', () => {
    const p = buildTutorialPlan({ preference: 'task', scope: 'both', isAndroid: false });
    expect(p.findIndex((id) => id === 't-switch')).toBeLessThan(p.findIndex((id) => id === 'r-intro'));
  });

  it('设置共同段只出现一次(第二条分支不重复讲)', () => {
    expect(bothRoleplayFirst.filter((id) => id === 's-enter')).toHaveLength(1);
    expect(bothRoleplayFirst.filter((id) => id === 's-api')).toHaveLength(1);
    expect(bothRoleplayFirst.filter((id) => id === 's-exec')).toHaveLength(1);
    const taskFirst = buildTutorialPlan({ preference: 'task', scope: 'both', isAndroid: false });
    expect(taskFirst.filter((id) => id === 's-enter')).toHaveLength(1);
  });

  it('Android 专属步骤按平台门控', () => {
    expect(bothRoleplayFirst).not.toContain('s-android-exec');
    expect(buildTutorialPlan({ preference: 'roleplay', scope: 'both', isAndroid: true })).toContain('s-android-exec');
    // 只看喜欢的 + Android 同样释放
    expect(buildTutorialPlan({ preference: 'task', scope: 'preferred', isAndroid: true })).toContain('s-android-exec');
  });

  it('两条分支各自的专属步骤不串门', () => {
    expect(bothRoleplayFirst).toContain('r-prompt');
    expect(bothRoleplayFirst).toContain('t-agent');
    expect(buildTutorialPlan({ preference: 'task', scope: 'preferred', isAndroid: false })).not.toContain('r-prompt');
  });
});

describe('讲解分支记录 planBranches', () => {
  it('不需要 = 空;只看喜欢的 = 偏好那一条;都需要 = 偏好在前', () => {
    expect(planBranches('none', 'roleplay')).toEqual([]);
    expect(planBranches('preferred', 'task')).toEqual(['task']);
    expect(planBranches('both', 'roleplay')).toEqual(['roleplay', 'task']);
    expect(planBranches('both', 'task')).toEqual(['task', 'roleplay']);
  });
});

describe('引导状态写入 writeOnboarding', () => {
  it('写入后即视为已完成', () => {
    const s = memStorage();
    writeOnboarding(s, { preferredMode: 'task', tutorials: ['task'] });
    expect(shouldShowOnboarding(s)).toBe(false);
    expect(readOnboarding(s)?.preferredMode).toBe('task');
  });

  it('tutorials 取并集:重开只讲一条分支不抹掉旧记录', () => {
    const s = memStorage();
    writeOnboarding(s, { preferredMode: 'roleplay', tutorials: ['roleplay', 'task'] });
    writeOnboarding(s, { preferredMode: 'roleplay', tutorials: ['roleplay'] });
    expect(readOnboarding(s)?.tutorials).toEqual(['roleplay', 'task']);
  });

  it('重复写入幂等(同参数两次,结果一致)', () => {
    const s = memStorage();
    writeOnboarding(s, { preferredMode: 'task', tutorials: ['task'], completedAt: '2026-09-27T00:00:00.000Z' });
    const first = s.raw.get(ONBOARDING_KEY);
    writeOnboarding(s, { preferredMode: 'task', tutorials: ['task'], completedAt: '2026-09-27T00:00:00.000Z' });
    expect(s.raw.get(ONBOARDING_KEY)).toBe(first);
  });

  it('存储不可用时静默失败(不抛)', () => {
    expect(() => writeOnboarding(throwingStorage, { preferredMode: 'task', tutorials: [] })).not.toThrow();
    expect(() => writeDefaultAppMode(throwingStorage, 'task')).not.toThrow();
  });

  it('completedAt 缺省时自动补时间戳', () => {
    const s = memStorage();
    writeOnboarding(s, { preferredMode: 'roleplay' as AppMode, tutorials: [] });
    expect(readOnboarding(s)?.completedAt).not.toBe('');
  });
});
