// @vitest-environment jsdom
// 任务附件清单面板(ATTACH-1)。jsdom 环境(要 DOM 与 confirm)。
//
// 关注三件事:清单从哪来(缓存 vs 详情顶层回落)、无附件时不渲染整块、
// 删除走「确认 → 服务端 → 重拉」而不是本地 splice。
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import { createPinia, setActivePinia } from 'pinia';
import type { TaskAttachment, TaskDetail } from '../../api';

vi.mock('../../api/tasks', () => ({
  uploadTaskAttachments: vi.fn(),
  listTaskAttachments: vi.fn(),
  deleteTaskAttachment: vi.fn(),
}));

import { deleteTaskAttachment, listTaskAttachments } from '../../api/tasks';
import { useAppStore } from '../../store';
import { resetTaskAttachmentsForTest, useTaskAttachments } from '../../composables/useTaskAttachments';
import TaskAttachmentList from './TaskAttachmentList.vue';

// store 在 setup 里读 localStorage;jsdom 未提供(与其它面板测试同款桩)。
const memStorage = new Map<string, string>();
vi.stubGlobal('localStorage', {
  getItem: (k: string) => memStorage.get(k) ?? null,
  setItem: (k: string, v: string) => void memStorage.set(k, String(v)),
  removeItem: (k: string) => void memStorage.delete(k),
  clear: () => memStorage.clear(),
  key: (i: number) => [...memStorage.keys()][i] ?? null,
  get length() { return memStorage.size; },
});

const listMock = vi.mocked(listTaskAttachments);
const deleteMock = vi.mocked(deleteTaskAttachment);

function att(overrides: Partial<TaskAttachment> = {}): TaskAttachment {
  return {
    id: 'a1',
    task_id: 't1',
    name: '规格说明.txt',
    rel_path: '.kedai/attachments/1-spec.txt',
    size: 2048,
    mime: 'text/plain',
    created_at: '2026-11-03T00:00:00Z',
    ...overrides,
  };
}

function makeDetail(attachments: TaskAttachment[]): TaskDetail {
  return {
    task: {
      id: 't1',
      title: '写周报',
      status: 'done',
      plan: [],
      result: '',
      error: '',
      created_at: '2026-11-03T00:00:00Z',
      updated_at: '2026-11-03T00:00:00Z',
    },
    subtasks: [],
    usage_total: { prompt_tokens: 0, completion_tokens: 0, reasoning_tokens: 0 },
    attachments,
  };
}

function seed(detail: TaskDetail | null): ReturnType<typeof useAppStore> {
  setActivePinia(createPinia());
  const store = useAppStore();
  store.currentTaskId = 't1';
  store.currentTask = detail;
  return store;
}

beforeEach(() => {
  resetTaskAttachmentsForTest();
  vi.clearAllMocks();
  // 默认「清单端点返回空」：对应真实链路里详情也没有附件的任务。
  // 详情带附件的用例必须显式让清单端点返回同样的内容——清单端点是权威来源
  // （与详情读同一张表），两者不一致时以清单为准（ATTFIX-4）。
  listMock.mockResolvedValue([]);
});

describe('任务附件清单面板', () => {
  it('无附件时不渲染整块(附件是可选输入,不占版面)', async () => {
    seed(makeDetail([]));
    const wrapper = mount(TaskAttachmentList);
    await flushPromises();
    expect(wrapper.text()).not.toContain('任务附件');
    expect(wrapper.findAll('li')).toHaveLength(0);
  });

  it('回落详情顶层的 attachments(首屏/换设备靠它)', async () => {
    seed(makeDetail([att()]));
    // 清单端点与详情读同一张表，故返回同一份清单
    listMock.mockResolvedValue([att()]);
    const wrapper = mount(TaskAttachmentList);
    await flushPromises();
    expect(wrapper.text()).toContain('任务附件');
    expect(wrapper.text()).toContain('规格说明.txt');
    expect(wrapper.text()).toContain('.kedai/attachments/1-spec.txt');
    expect(wrapper.text()).toContain('2.0 KB');
  });

  it('图像附件标「图」,文本标「文」', async () => {
    const list = [
      att({ id: 'a1', name: 'shot.png', mime: 'image/png' }),
      att({ id: 'a2', name: 'n.txt', mime: 'text/plain' }),
    ];
    seed(makeDetail(list));
    listMock.mockResolvedValue(list);
    const wrapper = mount(TaskAttachmentList);
    await flushPromises();
    const kinds = wrapper.findAll('.sv-attachment-kind').map((n) => n.text());
    expect(kinds).toEqual(['图', '文']);
  });

  it('缓存优先于详情顶层(刚上传完即时可见)', async () => {
    const store = seed(makeDetail([att({ id: 'old', name: '旧文件.txt' })]));
    void store;
    // 先塞缓存
    const a = useTaskAttachments();
    listMock.mockResolvedValueOnce([att({ id: 'new', name: '新文件.txt' })]);
    await a.refresh('t1');
    const wrapper = mount(TaskAttachmentList);
    await flushPromises();
    expect(wrapper.text()).toContain('新文件.txt');
    expect(wrapper.text()).not.toContain('旧文件.txt');
  });

  it('删除要确认;确认后调服务端并重拉清单', async () => {
    seed(makeDetail([att()]));
    listMock.mockResolvedValue([att()]);
    const wrapper = mount(TaskAttachmentList);
    await flushPromises();
    const confirmSpy = vi.spyOn(globalThis, 'confirm').mockReturnValue(true);
    deleteMock.mockResolvedValueOnce(undefined);
    listMock.mockResolvedValueOnce([att()]);
    await wrapper.find('button').trigger('click');
    await flushPromises();
    expect(confirmSpy).toHaveBeenCalled();
    expect(deleteMock).toHaveBeenCalledWith('t1', 'a1');
    expect(listMock).toHaveBeenCalledTimes(2);
    confirmSpy.mockRestore();
  });

  it('取消确认时不发请求', async () => {
    seed(makeDetail([att()]));
    listMock.mockResolvedValue([att()]);
    const wrapper = mount(TaskAttachmentList);
    await flushPromises();
    const confirmSpy = vi.spyOn(globalThis, 'confirm').mockReturnValue(false);
    await wrapper.find('button').trigger('click');
    await flushPromises();
    expect(deleteMock).not.toHaveBeenCalled();
    confirmSpy.mockRestore();
  });

  // ATTFIX-4(2026-11-04):`cached.length > 0` 判「有没有缓存」无法区分
  // 「已加载且为空」与「从未加载」。删掉最后一个附件后缓存是空数组 → 判为没加载过
  // → 回落 `currentTask.attachments`(删之前的快照)→ 已删的附件又冒出来。
  it('删除唯一附件后不再显示(缓存已加载且为空,不得回落详情快照)', async () => {
    const store = seed(makeDetail([att()]));
    listMock.mockResolvedValueOnce([att()]);
    const wrapper = mount(TaskAttachmentList);
    await flushPromises();
    expect(wrapper.findAll('li')).toHaveLength(1);

    const confirmSpy = vi.spyOn(globalThis, 'confirm').mockReturnValue(true);
    deleteMock.mockResolvedValueOnce(undefined);
    // 服务端删除后清单归零
    listMock.mockResolvedValue([]);
    await wrapper.find('button').trigger('click');
    await flushPromises();

    expect(wrapper.findAll('li')).toHaveLength(0);
    expect(wrapper.text()).not.toContain('任务附件');
    // 详情快照故意保持旧值:真实链路里 store 不会因为删附件就重拉详情
    expect(store.currentTask?.attachments).toHaveLength(1);
    confirmSpy.mockRestore();
  });

  // 反面对照:从未加载过时仍应回落详情顶层(首屏靠它)
  it('从未加载过缓存时仍回落详情顶层', async () => {
    seed(makeDetail([att()]));
    // listMock 保持挂起不返回 → 缓存始终未建立
    listMock.mockReturnValueOnce(new Promise(() => {}));
    const wrapper = mount(TaskAttachmentList);
    await flushPromises();
    expect(wrapper.findAll('li')).toHaveLength(1);
  });
});
