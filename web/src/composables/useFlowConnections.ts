// 流程编辑器用的**只读连接选项**(二维批次 5b)。
//
// 为什么单独一个 composable 而不是复用 useConnectionProfiles:那个是「连接配置区」的
// 编辑态草稿(自己的 ref、含未保存的新行、含只写密钥),节点选择器只需要**已落库**的
// 连接列表 —— 拿草稿会把「还没保存的连接」也列出来,而用户选中后存进流程的是一个
// 尚不存在的 id。故这里独立拉一次设置(本地服务,开销可忽略)。
//
// 加载时机与既有纪律一致:组件在 `onMounted` 里调 `load()`(SSR 不触发 → 测试期不发请求),
// 之后由 `show` 的 false→true 变化触发重拉 —— 「先开连接配置改连接、再切回执行流程」
// 因此不会用到陈旧列表。
import { ref, watch, type Ref } from 'vue';
import * as api from '../api';
import { useAppStore } from '../store';
import { connectionOptions, type FlowConnectionOption } from '../utils/agentFlowConnections';

export function useFlowConnections(show: Ref<boolean>) {
  const store = useAppStore();
  /** 已落库的连接选项(顺序即设置在库里/设置页的顺序) */
  const options = ref<FlowConnectionOption[]>([]);
  const error = ref('');

  async function load(): Promise<void> {
    try {
      const s = await api.getSettings(store.appMode);
      options.value = connectionOptions(s.connections);
      error.value = '';
    } catch (e) {
      // 拉不到就退化为「没有任何候选」:节点上的引用会显示为『引用已失效』
      // (宁可提示保守,也不静默显示成「没有引用」)
      error.value = `加载连接列表失败:${(e as Error).message}`;
    }
  }

  // 非 immediate:首次加载由组件的 onMounted 负责(SSR 与单测因此不发请求)
  watch(show, (visible) => {
    if (visible) void load();
  });

  return { options, error, load };
}
