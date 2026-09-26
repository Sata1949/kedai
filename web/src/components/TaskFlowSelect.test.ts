import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createSSRApp, h, type Component } from 'vue';
import { renderToString } from 'vue/server-renderer';
import { createPinia, setActivePinia, type Pinia } from 'pinia';
import { useTaskStore } from '../stores/task';
import { useGenSettingsStore } from '../stores/genSettings';
import TaskFlowSelect from './TaskFlowSelect.vue';
import type { AgentFlowLibrary, TaskRunMode } from '../api';

// 二维批次 5a:任务**绑定流程**选择器的 SSR 冒烟测试(项目无 jsdom/@vue/test-utils,
// 沿用 settingsSections.test.ts / taskModeUi.test.ts 的 SSR 模式)。
// SSR 不触发 onMounted,故组件不发请求;这正好也锁住「服务端不拉流程库」的既有约定。
// 组件抽成独立文件的原因同 TaskModeSelect:Sidebar 引用的 /logo.png 在 vitest 下无法解析。

// node 环境无 localStorage,而 task store 初始化即访问(appMode / taskRunMode / taskFlowId),补内存桩
const memStorage = new Map<string, string>();
vi.stubGlobal('localStorage', {
  getItem: (k: string) => memStorage.get(k) ?? null,
  setItem: (k: string, v: string) => void memStorage.set(k, String(v)),
  removeItem: (k: string) => void memStorage.delete(k),
  clear: () => memStorage.clear(),
  key: (i: number) => [...memStorage.keys()][i] ?? null,
  get length() { return memStorage.size; },
});

const library: AgentFlowLibrary = {
  current_flow_id: 'f1',
  flows: [
    { id: 'f1', name: '主流程', enabled: true, steps: [] },
    { id: 'f2', name: '备用流程', enabled: false, steps: [] },
  ],
};

/** 以 pinia 上下文 SSR 渲染组件为 HTML 字符串;seed 在渲染前播种状态 */
async function render(comp: Component, seed: (pinia: Pinia) => void): Promise<string> {
  const pinia = createPinia();
  setActivePinia(pinia);
  seed(pinia);
  const app = createSSRApp({ render: () => h(comp) });
  app.use(pinia);
  return renderToString(app);
}

/** 播种模式 + 绑定 + 流程库 */
function seed(mode: TaskRunMode, flowId = '', lib: AgentFlowLibrary | null = library) {
  return (p: Pinia): void => {
    void p;
    useTaskStore().taskRunMode = mode;
    useTaskStore().taskFlowId = flowId;
    useGenSettingsStore().agentFlowLibrary = lib;
  };
}

describe('TaskFlowSelect:任务绑定流程选择器(二维批次 5a)', () => {
  beforeEach(() => {
    memStorage.clear();
  });

  it('仅自定义流程模式出现;其余模式整块不渲染(后端也会拒绝非 custom 的 flow_id)', async () => {
    expect(await render(TaskFlowSelect, seed('custom'))).toContain('流程:跟随当前流程');
    for (const mode of ['legacy', 'solo', 'multi', 'plan', 'team'] as TaskRunMode[]) {
      const html = await render(TaskFlowSelect, seed(mode));
      expect(html, `${mode} 模式不应出现绑定选择器`).not.toContain('流程:');
    }
  });

  it('列出「跟随当前流程」+ 库内流程;已停用流程 disabled 并标注', async () => {
    const html = await render(TaskFlowSelect, seed('custom'));
    expect(html).toContain('流程:跟随当前流程');
    expect(html).toContain('流程:主流程');
    expect(html).toContain('流程:备用流程(已停用)');
    // 停用项不可选(选中它会被后端 400;不给用户这条路)
    expect(html).toMatch(/<option[^>]*value="f2"[^>]*disabled|disabled[^>]*value="f2"/);
    // 启用项不带 disabled
    expect(html).not.toMatch(/<option[^>]*value="f1"[^>]*disabled/);
  });

  it('持久化的绑定生效(selected 落在该项上)', async () => {
    const html = await render(TaskFlowSelect, seed('custom', 'f1'));
    expect(html).toMatch(/value="f1"[^>]*selected|selected[^>]*value="f1"/);
  });

  it('库未加载 / 绑定已失效都如实保留:不静默改绑到别的流程', async () => {
    // 库未加载:只剩「跟随当前流程」+ 失效占位(值仍是用户原来选的 id)
    const noLib = await render(TaskFlowSelect, seed('custom', 'f-deleted', null));
    expect(noLib).toContain('流程:(已失效) f-deleted');
    expect(noLib).not.toContain('流程:主流程');

    // 库已加载但流程被删:同样是失效占位,而不是回落到第一个流程
    const staleHtml = await render(TaskFlowSelect, seed('custom', 'f-deleted', library));
    expect(staleHtml).toContain('流程:(已失效) f-deleted');
    expect(staleHtml).toMatch(/value="f-deleted"[^>]*selected|selected[^>]*value="f-deleted"/);

    // 正常绑定(库内命中)时不出现失效项
    const ok = await render(TaskFlowSelect, seed('custom', 'f1', library));
    expect(ok).not.toContain('已失效');
  });
});
