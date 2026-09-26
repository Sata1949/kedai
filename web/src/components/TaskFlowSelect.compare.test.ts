// @vitest-environment jsdom
// 二维批次 7b:对比模式(流程用法 + 可调用名单)的**交互**测试。
//
// 为什么与同目录的 TaskFlowSelect.test.ts 分开:那份走 SSR 冒烟(renderToString),
// 而 SSR 不触发 onMounted 与 watch —— 本批新增的行为恰好全在客户端(默认预选、
// 勾选回写、警示随状态变化),只能用 jsdom 真实挂载验证。两份文件各守一半:
// SSR 那份锁「服务端不拉库」与静态渲染,本份锁交互。
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { mount } from '@vue/test-utils';
import { createPinia, setActivePinia } from 'pinia';
import { nextTick } from 'vue';

// jsdom 无 localStorage:piniа store 初始化即访问(与同批其它测试同款内存桩)
const memStorage = new Map<string, string>();
vi.stubGlobal('localStorage', {
  getItem: (k: string) => memStorage.get(k) ?? null,
  setItem: (k: string, v: string) => void memStorage.set(k, String(v)),
  removeItem: (k: string) => void memStorage.delete(k),
  clear: () => memStorage.clear(),
  key: (i: number) => [...memStorage.keys()][i] ?? null,
  get length() {
    return memStorage.size;
  },
});

// 组件在库未加载时会拉库:测试里显式播种库,这里兜底 mock 掉,避免真实请求
vi.mock('../api', async (importOriginal) => {
  const orig = await importOriginal<typeof import('../api')>();
  return { ...orig, getAgentFlow: vi.fn().mockResolvedValue(null) };
});

import { useTaskStore } from '../stores/task';
import { useGenSettingsStore } from '../stores/genSettings';
import TaskFlowSelect from './TaskFlowSelect.vue';
import type { AgentFlowLibrary } from '../api';

const library: AgentFlowLibrary = {
  current_flow_id: 'f1',
  flows: [
    { id: 'f1', name: '主流程', enabled: true, steps: [] },
    { id: 'f2', name: '备用流程', enabled: true, steps: [] },
    { id: 'f3', name: '停用流程', enabled: false, steps: [] },
  ],
};

/** 挂载选择器并播种状态(custom 模式;库固定为上面那份) */
function mountSelect(seed: (store: ReturnType<typeof useTaskStore>) => void) {
  const pinia = createPinia();
  setActivePinia(pinia);
  const store = useTaskStore();
  store.taskRunMode = 'custom';
  store.taskFlowMode = 'force';
  useGenSettingsStore().agentFlowLibrary = library;
  seed(store);
  const wrapper = mount(TaskFlowSelect, { global: { plugins: [pinia] } });
  return { wrapper, store };
}

/** 勾选框(label 文本里带流程名) */
function checkboxOf(wrapper: ReturnType<typeof mountSelect>['wrapper'], label: string) {
  const item = wrapper.findAll('label.flow-id-item').find((l) => l.text().includes(label));
  if (!item) throw new Error(`未找到勾选项:${label}`);
  return item.find('input[type="checkbox"]');
}

describe('TaskFlowSelect:对比模式(二维批次 7b)', () => {
  beforeEach(() => {
    memStorage.clear();
  });

  it('强制模式不渲染名单区;切到对比后出现(流程用法始终在 custom 下可选)', async () => {
    const { wrapper, store } = mountSelect(() => {});
    expect(wrapper.find('select').exists()).toBe(true);
    expect(wrapper.text()).toContain('流程模式:强制');
    expect(wrapper.text()).toContain('流程模式:对比(模型可调用名单内流程)');
    expect(wrapper.find('.flow-ids').exists(), '强制模式不渲染勾选区').toBe(false);

    store.taskFlowMode = 'compare';
    await nextTick();
    expect(wrapper.find('.flow-ids').exists()).toBe(true);
  });

  it('进入对比模式预选全部启用流程,**排除根流程**;停用流程不可勾选', async () => {
    const { wrapper, store } = mountSelect((s) => {
      s.taskFlowMode = 'compare';
    });
    await nextTick();
    // 根流程 = 库当前流程 f1(未显式绑定):它正在执行,不可被调用
    expect(store.taskFlowIds, '默认预选启用且非根的流程').toEqual(['f2']);
    expect(checkboxOf(wrapper, '备用流程').element).toHaveProperty('checked', true);
    expect(checkboxOf(wrapper, '备用流程').element).toHaveProperty('disabled', false);
    // 根流程行:禁用且如实标注原因
    expect(wrapper.text()).toContain('主流程(根流程,不可调用)');
    expect(checkboxOf(wrapper, '主流程').element).toHaveProperty('disabled', true);
    // 停用流程:禁用 + 标注
    expect(wrapper.text()).toContain('停用流程(已停用)');
    expect(checkboxOf(wrapper, '停用流程').element).toHaveProperty('disabled', true);
  });

  it('显式绑定的根流程(flow_id)同样不可被勾选:根判定优先于「当前流程」', async () => {
    const { wrapper, store } = mountSelect((s) => {
      s.taskFlowId = 'f2';
      s.taskFlowMode = 'compare';
    });
    await nextTick();
    expect(store.taskFlowIds, '根换成 f2 → 预选只剩 f1').toEqual(['f1']);
    expect(checkboxOf(wrapper, '备用流程').element).toHaveProperty('disabled', true);
  });

  it('勾选/取消写回 store;清空名单不被立刻填回,并给出「名单为空」警示', async () => {
    const { wrapper, store } = mountSelect((s) => {
      s.taskFlowMode = 'compare';
    });
    await nextTick();

    // 取消唯一的勾选:名单清空 → 警示出现,且**不会**被 watch 立刻填回
    await checkboxOf(wrapper, '备用流程').setValue(false);
    await nextTick();
    expect(store.taskFlowIds).toEqual([]);
    expect(wrapper.text()).toContain('名单为空');

    // 重新勾上:警示消失
    await checkboxOf(wrapper, '备用流程').setValue(true);
    await nextTick();
    expect(store.taskFlowIds).toEqual(['f2']);
    expect(wrapper.text()).not.toContain('名单为空');

    // 勾选顺序即名单顺序(后端工具描述按此列举)
    await checkboxOf(wrapper, '主流程').setValue(true); // 根流程禁用,不应生效
    expect(store.taskFlowIds).toEqual(['f2']);
  });

  it('持久化名单里已失效的流程如实展示、可取消勾选,并给出警示', async () => {
    const { wrapper, store } = mountSelect((s) => {
      s.taskFlowMode = 'compare';
      s.taskFlowIds = ['f2', 'f-已删除'];
    });
    await nextTick();
    expect(wrapper.text()).toContain('(已失效) f-已删除');
    expect(wrapper.text()).toContain('已失效的流程');

    // 取消失效项:警示消失,余下名单保持
    await checkboxOf(wrapper, '(已失效) f-已删除').setValue(false);
    await nextTick();
    expect(store.taskFlowIds).toEqual(['f2']);
    expect(wrapper.text()).not.toContain('已失效的流程');
  });

  it('非 custom 模式整块不渲染(后端也会拒绝 flow_id / flow_ids)', async () => {
    const { wrapper, store } = mountSelect((s) => {
      s.taskFlowMode = 'compare';
    });
    await nextTick();
    expect(wrapper.find('select').exists()).toBe(true);

    store.taskRunMode = 'solo';
    await nextTick();
    expect(wrapper.find('select').exists()).toBe(false);
    expect(wrapper.find('.flow-ids').exists()).toBe(false);
  });
});
