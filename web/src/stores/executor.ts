// 任务执行者 store(2026-09-17:执行者与角色扮演角色卡解耦)。
//
// 代际: L2(中层·干 / Orchestration)——Pinia setup store,状态的唯一变更入口。
//
// 为什么独立成 store 而非并入 task store:
//   执行者库是「配置资产」,与任务列表/详情/SSE 订阅(运行态)生命周期不同——
//   两种模式下都可能需要打开执行者管理面板,而任务态只在任务模式加载。
//   独立 store 也让依赖方向保持单向:本 store 只依赖 api 层,不 import 任何其他 store,
//   因此不参与 store 依赖图成环(见 tools/check-arch.mjs 规则 A)。
import { defineStore } from 'pinia';
import { computed, ref } from 'vue';
import * as api from '../api';

export const useExecutorStore = defineStore('app.executor', () => {
  // ===== 状态 =====
  /** 执行者库(库内顺序;任务创建下拉与管理面板共用) */
  const executors = ref<api.TaskExecutor[]>([]);
  /** 加载失败信息(面板展示;空串 = 无错误) */
  const executorsError = ref('');
  const executorsLoading = ref(false);

  // ===== 派生 =====
  /** 按 id 查执行者(任务详情展示「用了哪个执行者」用;未知 id 返回 undefined,
   *  与后端「查不到即回退通用执行者」语义一致) */
  const executorById = computed(() => {
    const map = new Map(executors.value.map((e) => [e.id, e]));
    return (id: string | null | undefined): api.TaskExecutor | undefined =>
      id ? map.get(id) : undefined;
  });

  // ===== 动作 =====
  /** 拉取执行者库(失败不清空既有数据,只记错误——下拉不至于突然变空) */
  async function loadExecutors(): Promise<void> {
    executorsLoading.value = true;
    executorsError.value = '';
    try {
      executors.value = await api.listExecutors();
    } catch (e) {
      executorsError.value = `读取执行者库失败:${(e as Error).message}`;
    } finally {
      executorsLoading.value = false;
    }
  }

  /** 保存(创建或更新)执行者;后端返回最新列表,直接替换本地(免二次拉取) */
  async function saveExecutor(config: api.TaskExecutorInput): Promise<api.TaskExecutor> {
    const { saved, executors: list } = await api.saveExecutor(config);
    executors.value = list;
    executorsError.value = '';
    return saved;
  }

  /** 删除执行者;引用它的任务不受影响(执行期回退通用执行者) */
  async function deleteExecutor(id: string): Promise<void> {
    executors.value = await api.deleteExecutor(id);
    executorsError.value = '';
  }

  return {
    executors,
    executorsError,
    executorsLoading,
    executorById,
    loadExecutors,
    saveExecutor,
    deleteExecutor,
  };
});
