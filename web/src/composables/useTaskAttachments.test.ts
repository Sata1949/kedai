// @vitest-environment jsdom
// 任务附件前端状态(ATTACH-1,2026-11-03)。jsdom 环境(要 File/FormData/DOM)。
//
// 这里测的是**判定与状态机**,不是网络细节:每个用例都 mock 三个 api 函数,
// 关注「队列怎么进、什么时候刷、失败怎么留、清单从哪来」。真实 multipart 线格式
// 由 `api/stream.test.ts` 的既有用例 + 后端集成测试覆盖,不在此重复。
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { TaskAttachment } from '../api';

vi.mock('../api/tasks', () => ({
  uploadTaskAttachments: vi.fn(),
  listTaskAttachments: vi.fn(),
  deleteTaskAttachment: vi.fn(),
}));

import { deleteTaskAttachment, listTaskAttachments, uploadTaskAttachments } from '../api/tasks';
import {
  MAX_ATTACHMENT_BYTES,
  MAX_ATTACHMENTS_PER_TASK,
  resetTaskAttachmentsForTest,
  useTaskAttachments,
} from './useTaskAttachments';

const uploadMock = vi.mocked(uploadTaskAttachments);
const listMock = vi.mocked(listTaskAttachments);
const deleteMock = vi.mocked(deleteTaskAttachment);

function file(name: string, size?: number): File {
  const f = new File(['x'], name, { type: 'text/plain' });
  if (size != null) Object.defineProperty(f, 'size', { value: size });
  return f;
}

function att(id: string, name = `${id}.txt`): TaskAttachment {
  return {
    id,
    task_id: 't1',
    name,
    rel_path: `.kedai/attachments/${id}-${name}`,
    size: 10,
    mime: 'text/plain',
    created_at: '2026-11-03T00:00:00Z',
  };
}

beforeEach(() => {
  resetTaskAttachmentsForTest();
  vi.clearAllMocks();
});

describe('任务附件:队列与校验', () => {
  it('入队保持顺序,并给出本地 id', () => {
    const a = useTaskAttachments();
    a.enqueue([file('b.txt'), file('a.txt')]);
    expect(a.pendingFiles.value.map((p) => p.file.name)).toEqual(['b.txt', 'a.txt']);
    expect(a.pendingFiles.value[0].localId).not.toBe(a.pendingFiles.value[1].localId);
  });

  it('超单文件上限被拒且报出文件名(不静默丢弃)', () => {
    const a = useTaskAttachments();
    const errors = a.enqueue([file('big.bin', MAX_ATTACHMENT_BYTES + 1), file('ok.txt')]);
    expect(errors).toHaveLength(1);
    expect(errors[0]).toContain('big.bin');
    expect(a.pendingFiles.value.map((p) => p.file.name)).toEqual(['ok.txt']);
  });

  it('空文件被拒', () => {
    const a = useTaskAttachments();
    const errors = a.enqueue([file('empty.txt', 0)]);
    expect(errors[0]).toContain('empty.txt');
    expect(a.pendingFiles.value).toHaveLength(0);
  });

  it('超过单任务个数上限的部分被拒', () => {
    const a = useTaskAttachments();
    const files = Array.from({ length: MAX_ATTACHMENTS_PER_TASK + 2 }, (_, i) => file(`f${i}.txt`));
    const errors = a.enqueue(files);
    expect(errors).toHaveLength(2);
    expect(a.pendingFiles.value).toHaveLength(MAX_ATTACHMENTS_PER_TASK);
  });

  it('移除与清空队列', () => {
    const a = useTaskAttachments();
    a.enqueue([file('a.txt'), file('b.txt')]);
    const first = a.pendingFiles.value[0].localId;
    a.removePending(first);
    expect(a.pendingFiles.value.map((p) => p.file.name)).toEqual(['b.txt']);
    a.clearPending();
    expect(a.pendingFiles.value).toHaveLength(0);
  });
});

describe('任务附件:上传与清单', () => {
  it('flushPending 成功后清空队列并把条目并入清单', async () => {
    const a = useTaskAttachments();
    a.enqueue([file('a.txt')]);
    uploadMock.mockResolvedValueOnce([att('1', 'a.txt')]);
    await a.flushPending('t1');
    expect(uploadMock).toHaveBeenCalledWith('t1', [expect.any(File)]);
    expect(a.pendingFiles.value).toHaveLength(0);
    expect(a.attachmentsOf('t1').map((x) => x.name)).toEqual(['a.txt']);
  });

  it('flushPending 失败保留队列并记错误(不静默丢附件)', async () => {
    const a = useTaskAttachments();
    a.enqueue([file('a.txt')]);
    uploadMock.mockRejectedValueOnce(new Error('网络错误'));
    await a.flushPending('t1');
    expect(a.pendingFiles.value).toHaveLength(1);
    expect(a.lastError.value).toContain('网络错误');
  });

  it('队列为空时 flushPending 不发请求', async () => {
    const a = useTaskAttachments();
    await a.flushPending('t1');
    expect(uploadMock).not.toHaveBeenCalled();
  });

  it('uploadTo 把新条目追加到已有清单之后', async () => {
    const a = useTaskAttachments();
    listMock.mockResolvedValueOnce([att('1')]);
    await a.refresh('t1');
    uploadMock.mockResolvedValueOnce([att('2')]);
    await a.uploadTo('t1', [file('b.txt')]);
    expect(a.attachmentsOf('t1').map((x) => x.id)).toEqual(['1', '2']);
  });

  it('refresh 失败保留旧清单(不让面板整块空白)', async () => {
    const a = useTaskAttachments();
    listMock.mockResolvedValueOnce([att('1')]);
    await a.refresh('t1');
    listMock.mockRejectedValueOnce(new Error('读失败'));
    await a.refresh('t1');
    expect(a.attachmentsOf('t1')).toHaveLength(1);
    expect(a.lastError.value).toContain('读失败');
  });

  it('remove 成功后重拉清单(不本地 splice)', async () => {
    const a = useTaskAttachments();
    listMock.mockResolvedValueOnce([att('1'), att('2')]);
    await a.refresh('t1');
    deleteMock.mockResolvedValueOnce(undefined);
    listMock.mockResolvedValueOnce([att('2')]);
    await a.remove('t1', '1');
    expect(deleteMock).toHaveBeenCalledWith('t1', '1');
    expect(listMock).toHaveBeenCalledTimes(2);
    expect(a.attachmentsOf('t1').map((x) => x.id)).toEqual(['2']);
  });

  it('forgetTask 丢弃缓存(任务删除后 id 复用不读到旧清单)', async () => {
    const a = useTaskAttachments();
    listMock.mockResolvedValueOnce([att('1')]);
    await a.refresh('t1');
    a.forgetTask('t1');
    expect(a.attachmentsOf('t1')).toHaveLength(0);
  });

  it('单例:两次调用共享同一份状态', () => {
    const a = useTaskAttachments();
    const b = useTaskAttachments();
    a.enqueue([file('a.txt')]);
    expect(b.pendingFiles.value).toHaveLength(1);
  });
});
