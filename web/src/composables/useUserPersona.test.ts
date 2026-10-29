// 用户设定 composable 单测(USERPERSONA-1)。
// 关键口径:显示层称呼必须与后端 `UserPersonaConfig::user_name()` 同判据
// (enabled && name 非空),否则关闭开关但保留文本时前后端展开出两种称呼。
import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('../api', async (importOriginal) => {
  const orig = await importOriginal<typeof import('../api')>();
  return {
    ...orig,
    getUserPersona: vi.fn(),
    saveUserPersona: vi.fn(),
  };
});

import * as api from '../api';
import { useUserPersona } from './useUserPersona';

const DEFAULT = { enabled: false, name: '', description: '' };

describe('useUserPersona', () => {
  beforeEach(() => {
    vi.mocked(api.getUserPersona).mockResolvedValue({ ...DEFAULT });
    vi.mocked(api.saveUserPersona).mockImplementation(async (c) => ({ ...c }));
  });

  // 放在首位:模块级 `loaded` 一旦置位,后续 ensureLoaded 都直接返回
  it('ensureLoaded 只真正加载一次(启动懒加载不重复打接口)', async () => {
    const persona = useUserPersona();
    await persona.ensureLoaded();
    await persona.ensureLoaded();
    expect(api.getUserPersona).toHaveBeenCalledTimes(1);
  });

  it('load 成功后 config 与 draft 同步;称呼仅在启用时生效', async () => {
    vi.mocked(api.getUserPersona).mockResolvedValue({
      enabled: true,
      name: ' 林 ',
      description: '退役侦探',
    });
    const persona = useUserPersona();
    await expect(persona.load()).resolves.toBe(true);

    expect(persona.config.value.name).toBe(' 林 ');
    expect(persona.draft.value.description).toBe('退役侦探');
    expect(persona.userName.value).toBe('林');
    expect(persona.userDescription.value).toBe('退役侦探');
  });

  it('关闭开关但保留文本时显示层回退「用户」,且不提供用户角色设定', async () => {
    vi.mocked(api.getUserPersona).mockResolvedValue({
      enabled: false,
      name: '林',
      description: '退役侦探',
    });
    const persona = useUserPersona();
    await persona.load();

    expect(persona.userName.value).toBe('用户');
    expect(persona.userDescription.value).toBe('');
  });

  it('启用但称呼为空时回退「用户」', async () => {
    vi.mocked(api.getUserPersona).mockResolvedValue({
      enabled: true,
      name: '   ',
      description: '退役侦探',
    });
    const persona = useUserPersona();
    await persona.load();

    expect(persona.userName.value).toBe('用户');
  });

  it('保存成功同步 config/draft;失败保留草稿并给出错误', async () => {
    const persona = useUserPersona();
    await persona.load();

    persona.draft.value = { enabled: true, name: '林', description: '侦探' };
    await expect(persona.save()).resolves.toBe(true);
    expect(api.saveUserPersona).toHaveBeenCalledWith({ enabled: true, name: '林', description: '侦探' });
    expect(persona.config.value).toEqual({ enabled: true, name: '林', description: '侦探' });
    expect(persona.userName.value).toBe('林');

    // 失败:不覆盖已保存的 config,草稿与错误保留给用户改
    vi.mocked(api.saveUserPersona).mockRejectedValueOnce(new Error('name 最多 64 个字符'));
    persona.draft.value = { enabled: false, name: '晚', description: '' };
    await expect(persona.save()).resolves.toBe(false);
    expect(persona.error.value).toContain('name 最多 64 个字符');
    expect(persona.draft.value.name).toBe('晚');
    expect(persona.config.value.name).toBe('林');
  });

  it('读取失败不抛错(聊天不受阻),错误落在 error', async () => {
    vi.mocked(api.getUserPersona).mockRejectedValueOnce(new Error('网络断开'));
    const persona = useUserPersona();
    await expect(persona.load()).resolves.toBe(false);
    expect(persona.error.value).toContain('网络断开');
  });

  it('resetDraft 只清草稿,不动已保存配置', async () => {
    const persona = useUserPersona();
    await persona.load();
    persona.draft.value = { enabled: true, name: '林', description: '侦探' };
    persona.resetDraft();
    expect(persona.draft.value).toEqual(DEFAULT);
    expect(persona.config.value).toEqual(DEFAULT);
  });
});
