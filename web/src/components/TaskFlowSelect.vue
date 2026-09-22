<script setup lang="ts">
// 任务**绑定流程**选择器(二维批次 5a):仅「自定义流程」模式下出现。
//
// 为什么独立成组件:与 TaskModeSelect 同款先例——Sidebar 引用的 /logo.png 在 SSR
// 冒烟测试下无法解析,抽出来后选择器可单独挂载测试;选择持久化在 task store
// (taskFlowId,localStorage),刷新后保持。
//
// 语义(后端为准,这里只做呈现与即时警示):
//  - 「跟随当前流程」(空串)= 旧客户端行为,执行时按当时的当前流程跑;
//  - 选中某个流程 = **绑定即冻结**:创建时后端落一份快照,此后改流程/换当前流程都不影响该任务;
//  - 已停用流程不可选(后端也会 400);持久化值已失效时如实显示,不静默改绑。
import { computed, onMounted, watch } from 'vue';
import { storeToRefs } from 'pinia';
import { useAppStore } from '../store';

const store = useAppStore();
const { taskRunMode, taskFlowId, agentFlowLibrary } = storeToRefs(store);

/** 仅自定义流程模式需要选流程(其余模式下后端会拒绝 flow_id) */
const visible = computed(() => taskRunMode.value === 'custom');

/** 库内流程(未加载 = 空数组,只显示「跟随当前流程」) */
const flows = computed(() => agentFlowLibrary.value?.flows ?? []);

/**
 * 持久化的绑定在库里找不到(流程被删 / 库还没加载完)→ 保留一条占位项,
 * 让 v-model 的值仍然可渲染,避免静默改绑到第一个流程。
 * 库加载完若命中,占位项自动消失(computed 依赖 agentFlowLibrary)。
 */
const staleId = computed(() => {
  const id = taskFlowId.value;
  if (!id) return '';
  return flows.value.some((f) => f.id === id) ? '' : id;
});

/**
 * 绑定要用到流程库,而任务模式此前只在设置区打开时拉过。这里在遇到 custom 模式且
 * 库为空时惰性拉一次(失败保持 null:选择器退化成只有「跟随当前流程」,不影响建任务)。
 *
 * 不用 `watch(immediate: true)`:那会在 SSR 期间就发请求,而本组件的 SSR 冒烟测试
 * 依赖「服务端不发请求」这一既有约定。改为挂载时检查一次 + 之后随模式变化检查。
 */
function ensureLib(): void {
  if (visible.value && !agentFlowLibrary.value) void store.loadAgentFlow();
}
onMounted(ensureLib);
watch(visible, ensureLib);
</script>

<template>
  <select
    v-if="visible"
    v-model="taskFlowId"
    class="sv-select"
    title="绑定要执行的自定义流程;绑定后该任务即冻结在这份编排上(改流程/换当前流程都不影响它)"
  >
    <option value="">流程:跟随当前流程</option>
    <option v-if="staleId" :value="staleId">流程:(已失效) {{ staleId }}</option>
    <option v-for="f in flows" :key="f.id" :value="f.id" :disabled="!f.enabled">
      流程:{{ f.name || f.id }}{{ f.enabled ? '' : '(已停用)' }}
    </option>
  </select>
</template>
