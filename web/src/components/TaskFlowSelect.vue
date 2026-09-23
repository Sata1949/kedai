<script setup lang="ts">
// 任务**流程用法 + 绑定流程 + 对比模式名单**选择器(二维批次 5a / 7b):仅「自定义流程」
// 模式下出现。
//
// 为什么独立成组件:与 TaskModeSelect 同款先例——Sidebar 引用的 /logo.png 在 SSR
// 冒烟测试下无法解析,抽出来后选择器可单独挂载测试;选择持久化在 task store
// (taskFlowId / taskFlowMode / taskFlowIds,localStorage),刷新后保持。
//
// 语义(后端为准,这里只做呈现与即时警示):
//  - 流程模式「强制」= 只跑根流程(老行为);「对比」= 根流程照常执行 + **名单内流程**
//    作为 `run_flow` 工具释放给宽松节点,模型在工具循环里自主调用、取回成果;
//  - 根流程:「跟随当前流程」(空串)= 旧客户端行为,执行时按当时的当前流程跑;
//    选中某个流程 = **绑定即冻结**:创建时后端落一份快照,此后改流程/换当前流程都不影响该任务;
//  - 已停用流程不可选/不可勾选(后端也会 400);持久化值已失效时如实显示,不静默改绑;
//  - **根流程自身不可被调用**(它正在执行,调用必然撞调用链环守卫),故勾选区里它只作展示并禁用;
//  - 名单为空时后端 400(空名单 = 名存实亡),故这里给出即时警示而不是让用户撞一次失败。
import { computed, onMounted, watch } from 'vue';
import { storeToRefs } from 'pinia';
import { useAppStore } from '../store';
import { FLOW_MODE_OPTION_LABELS, FLOW_MODE_ORDER } from '../api/labels';

const store = useAppStore();
const { taskRunMode, taskFlowId, taskFlowMode, taskFlowIds, agentFlowLibrary } =
  storeToRefs(store);

/** 仅自定义流程模式需要选流程(其余模式下后端会拒绝 flow_id / flow_ids) */
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
 * 根流程 id:显式绑定优先,否则「跟随当前流程」= 库里的当前流程。
 * 运行期根流程由后端解析(未绑定任务跟随当时的当前流程),这里只是给 UI 一个
 * 「哪个不能被勾选」的展示口径——与后端「可调用集扣除根流程」同源。
 */
const rootId = computed(() => taskFlowId.value || agentFlowLibrary.value?.current_flow_id || '');

/** 勾选行:启用的库内流程(根流程禁用)+ 已失效的持久化项(可取消勾选) */
interface Candidate {
  id: string;
  label: string;
  disabled: boolean;
  stale: boolean;
}
const candidates = computed<Candidate[]>(() => {
  const out: Candidate[] = [];
  for (const f of flows.value) {
    // 库内流程的 id 由后端分配(TaskSend 类型标 optional,故这里显式挡一次脏数据)
    const id = f.id ?? '';
    if (!id) continue;
    const name = f.name || id;
    if (id === rootId.value) {
      out.push({ id, label: `${name}(根流程,不可调用)`, disabled: true, stale: false });
      continue;
    }
    out.push({
      id,
      label: f.enabled ? name : `${name}(已停用)`,
      disabled: !f.enabled,
      stale: false,
    });
  }
  // 持久化名单里库里已不存在的 id:如实展示并允许取消勾选(否则创建必被后端 400)
  for (const id of taskFlowIds.value) {
    if (!flows.value.some((f) => f.id === id)) {
      out.push({ id, label: `(已失效) ${id}`, disabled: false, stale: true });
    }
  }
  return out;
});

/** 勾选态(数组顺序 = 用户勾选顺序,后端工具描述按此列举) */
function isChecked(id: string): boolean {
  return taskFlowIds.value.includes(id);
}

function onToggle(id: string, ev: Event): void {
  const checked = (ev.target as HTMLInputElement).checked;
  const list = taskFlowIds.value.filter((x) => x !== id);
  if (checked) list.push(id);
  taskFlowIds.value = list;
}

/** 名单即时警示(后端为准;这里只把「一定会 400」的两种情形提前说清楚) */
const listWarning = computed(() => {
  if (!flows.value.length) return '流程库为空:请先在设置里添加并启用流程。';
  const usable = candidates.value.filter((c) => !c.disabled && !c.stale).length;
  if (usable === 0) return '没有可勾选的流程(全部停用或只有根流程):对比模式至少需要一个可调用流程。';
  if (taskFlowIds.value.some((id) => candidates.value.some((c) => c.id === id && c.stale)))
    return '名单里有已失效的流程,请取消勾选后再创建。';
  if (taskFlowIds.value.length === 0)
    return '名单为空:对比模式需要至少勾选一个可调用流程,否则创建会被拒绝。';
  return '';
});

/**
 * 默认预选:**进入对比模式时若名单为空**,预选全部启用流程(排除根流程)。
 * 只在「本来就没有选择」时填默认值——用户取消勾选后的空名单不该被反复填回去,
 * 故判据是「名单为空」而不是「模式变化」。
 */
function prefillIfEmpty(): void {
  if (!visible.value || taskFlowMode.value !== 'compare') return;
  if (taskFlowIds.value.length > 0 || flows.value.length === 0) return;
  const ids = flows.value
    .filter((f) => f.enabled && f.id && f.id !== rootId.value)
    .map((f) => f.id as string);
  if (ids.length) taskFlowIds.value = ids;
}

/**
 * 绑定与名单都要用到流程库,而任务模式此前只在设置区打开时拉过。这里在遇到 custom 模式
 * 且库为空时惰性拉一次(失败保持 null:选择器退化成只有「跟随当前流程」,不影响建任务)。
 *
 * 不用 `watch(immediate: true)`:那会在 SSR 期间就发请求,而本组件的 SSR 冒烟测试
 * 依赖「服务端不发请求」这一既有约定。改为挂载时检查一次 + 之后随模式/库变化检查。
 */
function ensureLib(): void {
  if (visible.value && !agentFlowLibrary.value) void store.loadAgentFlow();
}
// 挂载时:拉库 + 补一次默认预选(刷新后持久化的是 compare 而名单为空时,用户看到的
// 应是「已预选默认值」而不是空名单 + 一条警示)
onMounted(() => {
  ensureLib();
  prefillIfEmpty();
});
watch(visible, () => {
  ensureLib();
  prefillIfEmpty();
});
// 库加载完成(或流程增删)后才谈得上预选;模式切换时也补一次
watch([() => flows.value.length, taskFlowMode], prefillIfEmpty);
</script>

<template>
  <div v-if="visible" class="flow-select-group">
    <select
      v-model="taskFlowMode"
      class="sv-select"
      title="流程模式:强制 = 只跑下面这一份流程;对比 = 根流程照常执行,名单内流程额外作为工具释放给节点,由模型自主调用"
    >
      <option v-for="m in FLOW_MODE_ORDER" :key="m" :value="m">
        {{ FLOW_MODE_OPTION_LABELS[m] }}
      </option>
    </select>
    <select
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
    <!-- 对比模式名单(二维批次 7b):勾选即「模型可调用」 -->
    <div v-if="taskFlowMode === 'compare'" class="flow-ids">
      <p class="sv-note">可调用流程(模型在工具循环里自主调用,成果回灌给根流程):</p>
      <label
        v-for="c in candidates"
        :key="c.id"
        class="flow-id-item"
        :class="{ disabled: c.disabled }"
      >
        <input
          type="checkbox"
          class="flow-id-box"
          :checked="isChecked(c.id)"
          :disabled="c.disabled"
          @change="onToggle(c.id, $event)"
        />
        <span>{{ c.label }}</span>
      </label>
      <p v-if="listWarning" class="sv-note flow-tool-warn">{{ listWarning }}</p>
    </div>
  </div>
</template>
