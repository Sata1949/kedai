import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createSSRApp, h } from 'vue';
import { renderToString } from 'vue/server-renderer';
import { createPinia, setActivePinia } from 'pinia';
import { useTaskStore } from '../stores/task';
import FileChangesPanel from './FileChangesPanel.vue';
import type { TaskFileChange } from '../api';

// 分批 4c 文件变更卡片冒烟(SSR:createSSRApp + renderToString,与 taskMessagesUi.test.ts 同款——
// 本组件默认不进 jsdom;交互路径(展开 diff / 回滚)由 store 与 api 层用例覆盖,
// SSR 只钉住**四态文案**这件用户直接看到的事)。
// 四态不许合并:「没有变更」「基线不可用」「扫描缺项」「未确认」是四种不同的答案。

// node 环境无 localStorage(store 初始化即访问),补内存桩
const memStorage = new Map<string, string>();
vi.stubGlobal('localStorage', {
  getItem: (k: string) => memStorage.get(k) ?? null,
  setItem: (k: string, v: string) => void memStorage.set(k, String(v)),
  removeItem: (k: string) => void memStorage.delete(k),
  clear: () => memStorage.clear(),
  key: (i: number) => [...memStorage.keys()][i] ?? null,
  get length() { return memStorage.size; },
});

function makeChange(over: Partial<TaskFileChange> = {}): TaskFileChange {
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

/** 渲染组件;seed 在渲染前对 task store 播种(SSR 不跑 onMounted,故不会触发拉取) */
async function render(seed: (store: ReturnType<typeof useTaskStore>) => void): Promise<string> {
  const pinia = createPinia();
  setActivePinia(pinia);
  const store = useTaskStore();
  store.currentTaskId = 't1';
  seed(store);
  const app = createSSRApp({ render: () => h(FileChangesPanel) });
  app.use(pinia);
  return renderToString(app);
}

beforeEach(() => {
  memStorage.clear();
});

describe('FileChangesPanel:四态文案(批次 4c)', () => {
  it('空清单且未缺项 → 「本轮未改动文件」,不出现缺项横幅', async () => {
    const html = await render((store) => {
      store.taskFileChanges = [];
      store.taskChangesUndected = null;
    });
    expect(html).toContain('本轮未改动文件');
    expect(html).not.toContain('未能完整检出');
  });

  it('扫描缺项 → 横幅带**原因原文**;清单为空时明说「不代表没有改动」', async () => {
    const reason = '后扫描未完成(扫到 20000 项即撞预算),已省略删除类改动';
    const html = await render((store) => {
      store.taskFileChanges = [];
      store.taskChangesUndected = reason;
    });
    expect(html).toContain(reason);
    expect(html).toContain('不代表没有改动');
    expect(html, '缺项时不许同时宣称「没有改动」').not.toContain('本轮未改动文件');
  });

  it('有改动 → 路径 / 类型 / 来源 / 字节数 + 「查看 diff」入口', async () => {
    const html = await render((store) => {
      store.taskFileChanges = [makeChange()];
      store.taskChangesUndected = null;
    });
    expect(html).toContain('src/a.rs');
    expect(html).toContain('修改');
    expect(html).toContain('命令检出');
    expect(html).toContain('查看 diff');
    expect(html).toContain('回滚');
  });

  it('基线不可用(truncated)→ 回滚禁用并给出原因,不给可点的死按钮', async () => {
    const html = await render((store) => {
      store.taskFileChanges = [makeChange({ truncated: true, has_baseline: false })];
      store.taskChangesUndected = null;
    });
    expect(html).toContain('回滚');
    expect(html, '禁用态要真的禁用').toContain('disabled');
    expect(html, '不给基线不可用的行留可点按钮').not.toContain('查看 diff');
    expect(html).toContain('基线不可用');
  });
});
