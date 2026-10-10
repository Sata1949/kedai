// 插件 API 的线格式回归(IMPHUB ⑦):路径 / 方法 / 请求体键名与 `errors` 缺省归一。
// 项目无 jsdom,api 层测试统一 mock `./client` 与 `./stream`,只断言「发出去什么、取回什么」。
import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('./client', () => ({ request: vi.fn() }));
vi.mock('./stream', () => ({ uploadForm: vi.fn() }));

import { request } from './client';
import { exportPluginTools, importPluginTools, movePluginTool } from './plugins';

const mockedRequest = vi.mocked(request);

beforeEach(() => {
  vi.clearAllMocks();
});

describe('plugins API 线格式', () => {
  it('批量导入 POST /plugins/tools/import,请求体是 { plugins: [...] }(与导出端点同形,可原样回导)', async () => {
    mockedRequest.mockResolvedValue({ ok: true, imported: 1, loaded: 1, removed: 0 });
    const res = await importPluginTools([{ file: 'a.json', name: 'a' }]);
    expect(res).toEqual({ ok: true, imported: 1, loaded: 1, removed: 0 });
    expect(mockedRequest).toHaveBeenCalledWith('/plugins/tools/import', {
      method: 'POST',
      body: JSON.stringify({ plugins: [{ file: 'a.json', name: 'a' }] }),
    });
  });

  it('移动插件 POST /plugins/tools/move 带 { file, folder }(原地改写 folder,不重注册)', async () => {
    mockedRequest.mockResolvedValue({ ok: true, file: 'a.json', folder: '工具/演示' });
    await movePluginTool('a.json', '工具/演示');
    expect(mockedRequest).toHaveBeenCalledWith('/plugins/tools/move', {
      method: 'POST',
      body: JSON.stringify({ file: 'a.json', folder: '工具/演示' }),
    });
  });

  it('导出端点缺 `errors` 键时归一为空数组(调用方无条件读 .length)', async () => {
    mockedRequest.mockResolvedValue({ ok: true, plugins: [{ file: 'a.json' }] });
    await expect(exportPluginTools()).resolves.toEqual({
      plugins: [{ file: 'a.json' }],
      errors: [],
    });
    expect(mockedRequest).toHaveBeenCalledWith('/plugins/tools/export');
  });
});
