// @vitest-environment jsdom
// FileChangesPanel 交互测试(批次 4c):SSR 冒烟只能钉四态文案,点不动按钮——
// 本文件用 jsdom + @vue/test-utils 覆盖「点开 diff / 点回滚」这条真实交互链
// (仓库自 2026-09 起即具备 jsdom 与 @vue/test-utils,ContractModal/AudioPlayer 等已有先例)。
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { mount } from '@vue/test-utils';
import { createPinia, setActivePinia } from 'pinia';

const memStorage = new Map<string, string>();
vi.stubGlobal('localStorage', {
  getItem: (k: string) => memStorage.get(k) ?? null,
  setItem: (k: string, v: string) => void memStorage.set(k, String(v)),
  removeItem: (k: string) => void memStorage.delete(k),
  clear: () => memStorage.clear(),
  key: (i: number) => [...memStorage.keys()][i] ?? null,
  get length() { return memStorage.size; },
});

const h = vi.hoisted(() => ({
  diffCalls: [] as string[],
  rollbackCalls: [] as string[],
  rollbackOk: true,
  confirmOk: true,
  // CODE-2:整任务回滚 + patch 导出
  bulkCalls: 0,
  bulkReject: null as string | null,
  batchRow: { path: 'src/a.rs', result: 'restored', restored_bytes: 3 } as Record<string, unknown>,
  patchCalls: 0,
  patchSaveOk: true,
  patchSaveArgs: [] as Array<[string, string]>,
}));

/** mock 里用的最小 wire 行(字段与后端一致;避免依赖文件后部定义的 makeRow) */
function makeWireRow(op: string) {
  return {
    id: 9,
    task_id: 't1',
    path: 'src/a.rs',
    op,
    source: 'rollback',
    before_hash: 'h1',
    after_hash: 'h1',
    before_bytes: 3,
    after_bytes: 3,
    truncated: false,
    has_baseline: true,
    created_at: '2026-09-30T00:00:01.000Z',
  };
}

vi.mock('../api', async (importOriginal) => {
  const orig = await importOriginal<typeof import('../api')>();
  return {
    ...orig,
    getTaskChanges: vi.fn(async () => ({
      // 回滚发生后重拉:清单里应多出一条 op=rollback 行(组件据此重渲染)
      changes:
        h.rollbackCalls.length || h.bulkCalls ? [makeWireRow('rollback')] : [],
      undected: false,
      undectedReason: null,
    })),
    getTaskChangeDiff: vi.fn(async (_taskId: string, path: string) => {
      h.diffCalls.push(path);
      return { available: true, path, diff: ['--- a/x', '+++ b/x', '+新增行'].join('\n') };
    }),
    rollbackTaskChange: vi.fn(async (_taskId: string, path: string) => {
      h.rollbackCalls.push(path);
      return h.rollbackOk
        ? { ok: true, path, restored_bytes: 3 }
        : { ok: false, path, available: false, reason: '基线不可用(改动前正文超出留存上限)' };
    }),
    // CODE-2:整任务回滚(逐项报告;bulkReject 模拟 409 等失败)
    rollbackAllTaskChanges: vi.fn(async () => {
      h.bulkCalls += 1;
      if (h.bulkReject) throw new Error(h.bulkReject);
      return {
        ok: true,
        restored: 2,
        removed: 0,
        skipped: 1,
        failed: 0,
        results: [
          h.batchRow,
          { path: 'src/b.rs', result: 'restored', restored_bytes: 5 },
          {
            path: 'src/big.rs',
            result: 'skipped',
            reason: '基线不可用(改动前正文超出留存上限或当时读取失败)',
          },
        ],
      };
    }),
    // CODE-2:patch 文本导出(不套 JSON 形状闸门)
    getTaskChangesPatch: vi.fn(async () => {
      h.patchCalls += 1;
      return '# Kedai 任务变更 patch\ndiff --git a/x b/x\n';
    }),
  };
});

// 保存通道整体 mock:测试只关心「内容与文件名进了保存调用」与取消语义,
// 不落真实对话框/下载(jsdom 也没有 URL.createObjectURL 之外的那套能力)。
vi.mock('../exportFile', () => ({
  saveTextFile: vi.fn(async (name: string, content: string) => {
    h.patchSaveArgs.push([name, content]);
    return h.patchSaveOk;
  }),
  saveExportFile: vi.fn(async () => true),
  downloadBlob: vi.fn(),
}));

import { useTaskStore } from '../stores/task';
import FileChangesPanel from './FileChangesPanel.vue';
import type { TaskFileChange } from '../api';

function makeRow(over: Partial<TaskFileChange> = {}): TaskFileChange {
  return {
    id: 1,
    task_id: 't1',
    path: 'src/a.rs',
    op: 'modify',
    source: 'bash',
    before_hash: 'h1',
    after_hash: 'h2',
    before_bytes: 3,
    after_bytes: 12,
    truncated: false,
    has_baseline: true,
    created_at: '2026-09-30T00:00:00.000Z',
    ...over,
  };
}

async function flush(): Promise<void> {
  for (let i = 0; i < 8; i += 1) await Promise.resolve();
}

function mountPanel(rows: TaskFileChange[]): ReturnType<typeof mount> {
  setActivePinia(createPinia());
  const store = useTaskStore();
  store.appMode = 'task';
  store.currentTaskId = 't1';
  store.taskFileChanges = rows;
  return mount(FileChangesPanel);
}

/** 按按钮文案**精确**取按钮(动作行「全部回滚」与行内「回滚」必须分得开);
 *  找不到直接抛错(避免非空断言——前端 lint ratchet 只降不升) */
function findButton(wrapper: ReturnType<typeof mount>, text: string) {
  const btn = wrapper.findAll('button').find((b) => b.text().trim() === text);
  if (!btn) throw new Error(`应存在按钮「${text}」`);
  return btn;
}

function clickByText(wrapper: ReturnType<typeof mount>, text: string): Promise<void> {
  return findButton(wrapper, text)
    .trigger('click')
    .then(() => flush());
}

beforeEach(() => {
  memStorage.clear();
  h.diffCalls = [];
  h.rollbackCalls = [];
  h.rollbackOk = true;
  h.confirmOk = true;
  h.bulkCalls = 0;
  h.bulkReject = null;
  h.patchCalls = 0;
  h.patchSaveOk = true;
  h.patchSaveArgs = [];
  vi.stubGlobal('confirm', () => h.confirmOk);
});

describe('FileChangesPanel 交互(批次 4c)', () => {
  it('点「查看 diff」→ 按 path 请求并渲染 diff;再点收起', async () => {
    const wrapper = mountPanel([makeRow()]);
    await clickByText(wrapper, '查看 diff');
    expect(h.diffCalls).toEqual(['src/a.rs']);
    expect(wrapper.html()).toContain('+新增行');

    await clickByText(wrapper, '收起 diff');
    expect(wrapper.html()).not.toContain('+新增行');
    expect(h.diffCalls, '收起不应重复请求').toEqual(['src/a.rs']);
  });

  it('点「回滚」→ 确认后调接口、显示结果并重拉清单(回滚自身会多一条行)', async () => {
    const wrapper = mountPanel([makeRow()]);

    await clickByText(wrapper, '回滚');
    expect(h.rollbackCalls).toEqual(['src/a.rs']);
    expect(wrapper.html()).toContain('回滚完成');
    // 重拉后的清单里出现 op=rollback 那一行(回滚自身不隐身)
    expect(wrapper.html()).toContain('op-rollback');
  });

  it('确认框取消 → 不发请求', async () => {
    const wrapper = mountPanel([makeRow()]);
    h.confirmOk = false;
    await clickByText(wrapper, '回滚');
    expect(h.rollbackCalls).toEqual([]);
  });

  it('回滚失败(ok:false)→ 显示原因原文,且不触发重拉', async () => {
    const wrapper = mountPanel([makeRow()]);
    h.rollbackOk = false;

    await clickByText(wrapper, '回滚');
    expect(wrapper.html()).toContain('基线不可用(改动前正文超出留存上限)');
    expect(wrapper.html()).toContain('回滚未执行');
    expect(wrapper.html(), '失败不重拉:清单里不应出现 rollback 行').not.toContain('op-rollback');
  });

  it('基线不可用的行:两个入口都不可点(不给死按钮)', async () => {
    const wrapper = mountPanel([makeRow({ truncated: true, has_baseline: false })]);
    const html = wrapper.html();
    expect(html).not.toContain('查看 diff');
    const rollback = findButton(wrapper, '回滚');
    expect(rollback.attributes('disabled')).toBeDefined();
    expect(rollback.attributes('title')).toContain('基线不可用');
  });

  // ===== CODE-2:整任务回滚 + patch 导出 =====

  it('「全部回滚」:确认后调接口、显示逐项汇总(含跳过原因)并重拉清单', async () => {
    const wrapper = mountPanel([makeRow(), makeRow({ id: 2, path: 'src/big.rs' })]);
    await clickByText(wrapper, '全部回滚');
    expect(h.bulkCalls).toBe(1);
    const html = wrapper.html();
    expect(html).toContain('整任务回滚完成');
    expect(html).toContain('成功 2 项');
    expect(html).toContain('跳过 1 项');
    expect(html).toContain('基线不可用(改动前正文超出留存上限或当时读取失败)');
    // 重拉后的清单里出现 op=rollback 那一行(回滚自身不隐身)
    expect(html).toContain('op-rollback');
  });

  it('「全部回滚」:确认框取消 → 不发请求', async () => {
    const wrapper = mountPanel([makeRow()]);
    h.confirmOk = false;
    await clickByText(wrapper, '全部回滚');
    expect(h.bulkCalls).toBe(0);
  });

  it('「全部回滚」:409 失败原文显示(任务进行中)', async () => {
    h.bulkReject = '任务进行中,请先停止再回滚(避免与模型写入互相覆盖)';
    const wrapper = mountPanel([makeRow()]);
    await clickByText(wrapper, '全部回滚');
    const html = wrapper.html();
    expect(html).toContain('整任务回滚失败');
    expect(html).toContain('任务进行中,请先停止再回滚');
  });

  it('「导出 patch」:取文本 + 经保存通道;文件名与内容正确', async () => {
    const wrapper = mountPanel([makeRow()]);
    await clickByText(wrapper, '导出 patch');
    expect(h.patchCalls).toBe(1);
    expect(h.patchSaveArgs.length).toBe(1);
    expect(h.patchSaveArgs[0][0]).toContain('.patch');
    expect(h.patchSaveArgs[0][1]).toContain('Kedai 任务变更 patch');
    expect(wrapper.html()).toContain('已导出');
  });

  it('「导出 patch」:用户取消保存 → 不提示成功(也没报错)', async () => {
    h.patchSaveOk = false;
    const wrapper = mountPanel([makeRow()]);
    await clickByText(wrapper, '导出 patch');
    const html = wrapper.html();
    expect(html).not.toContain('已导出');
    expect(html).not.toContain('导出失败');
  });
});
