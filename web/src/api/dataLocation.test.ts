import { beforeEach, describe, expect, it, vi } from 'vitest';
import { archiveImportFile, getDataLocation, selectDataLocation } from './dataLocation';
import { resetApiTokenForTest } from './client';

/** 第一次 fetch 是 bootstrap 取 token,第二次才是目标请求 */
function mockBootstrapThen(body: unknown, status = 200): void {
  vi.spyOn(globalThis, 'fetch')
    .mockResolvedValueOnce(new Response(JSON.stringify({ token: 't' }), { status: 200 }))
    .mockResolvedValueOnce(new Response(JSON.stringify(body), { status }));
}

describe('dataLocation API', () => {
  beforeEach(() => {
    resetApiTokenForTest();
    vi.restoreAllMocks();
  });

  it('GET /data-location 返回状态', async () => {
    const payload = {
      managed: true,
      pending: true,
      root: null,
      data_dir: 'C:\\old\\data',
      images_dir: 'C:\\old\\data\\images',
      task_records_dir: 'C:\\old\\task_scratch',
      imports_dir: 'C:\\old\\data\\imports',
      default_drive: 'D',
      drives: [{ letter: 'C' }, { letter: 'D' }],
      legacy_data: false,
      pointer: null,
      error: null,
    };
    mockBootstrapThen(payload);
    const got = await getDataLocation();
    expect(got.default_drive).toBe('D');
    expect(String(vi.mocked(fetch).mock.calls[1][0])).toBe('/api/data-location');
  });

  it('PUT /data-location 发送盘符 JSON', async () => {
    mockBootstrapThen({ root: 'D:\\Kedai-Data', migrated: true, restart_required: true });
    await selectDataLocation('D');
    const req = vi.mocked(fetch).mock.calls[1];
    expect(String(req[0])).toBe('/api/data-location');
    expect((req[1] as RequestInit).method).toBe('PUT');
    expect((req[1] as RequestInit).body).toBe(JSON.stringify({ drive: 'D' }));
  });

  it('POST /imports/archive 用 FormData 上传原始文件', async () => {
    mockBootstrapThen({ ok: true, name: '20260101-x.json' });
    const file = new File(['{}'], 'x.json', { type: 'application/json' });
    await archiveImportFile(file);
    const req = vi.mocked(fetch).mock.calls[1];
    expect(String(req[0])).toBe('/api/imports/archive');
    expect((req[1] as RequestInit).method).toBe('POST');
    expect((req[1] as RequestInit).body).toBeInstanceOf(FormData);
  });
});
