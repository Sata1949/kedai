<script setup lang="ts">
// 设置区:自定义 Agent 执行流程(custom 模式;流程库管理 + 步骤编辑)。
// 从 SettingsModal.vue 双模板合并而来:取 embedded 超集版本(standalone 分支缺失
// 「工具策略 / 并行调用」行,属模板漂移,合并后 standalone 一并补上)。
//
// 二维批次 3 起本区有两个视图:
//   - **列表(默认)**:一维/二维都能编,WF-10 要求线性列表保持默认视图(WF-10 见 `展望.md`);
//   - **画布(高级)**:二维节点图可视化编辑,异步加载——只有真正切过去才下载画布库。
// 两个视图共用 AgentFlowStepEditor(步骤表单)与 utils/agentFlowGraph(图算法),
// 不各写一份,避免本文件历史上出现过的双模板漂移。
import { computed, defineAsyncComponent, onMounted, ref } from 'vue';
import { storeToRefs } from 'pinia';
import { useAppStore } from '../../store';
import { useAgentFlow } from '../../composables/useAgentFlow';
import { useFlowConnections } from '../../composables/useFlowConnections';
import { graphHint, isLinearCompat, subFlowSummary, upstreamSummary } from '../../utils/agentFlowGraph';
import { connectionSummary } from '../../utils/agentFlowConnections';
import type { ToolPolicyCtx } from '../../utils/agentFlowTools';
import { loadFlowCanvas } from '../../utils/flowCanvasChunk';
import AgentFlowStepEditor from './AgentFlowStepEditor.vue';

// 画布重(@vue-flow 及其 d3/@vueuse 传递依赖),做成异步组件:切到画布视图才下载。
// 与 lazyModal 同一纪律「失败自动重试一次」——静默失败在这里表现为一块空白画布,
// 用户无从判断是加载慢还是坏了。故第二次仍失败时**就地给出提示 + 重试入口**
// (弹窗体系走 asyncModal 写全局错误条;画布是分区内的异步组件,就地提示更贴近出错位置)。
const canvasLoadFailed = ref(false);
/** 换 key 重建异步组件 = 让懒加载重新发起(切视图同理);失败兜底是刷新页面 */
const canvasKey = ref(0);

const AgentFlowCanvas = defineAsyncComponent({
  // 懒加载入口在 utils/flowCanvasChunk(单独成模块 → 失败路径可测,见该文件注释)
  loader: loadFlowCanvas,
  onError(error, retry, fail, attempts) {
    if (attempts <= 1) {
      retry();
      return;
    }
    console.error('[kedai] 流程画布加载失败', error);
    canvasLoadFailed.value = true;
    fail();
  },
});

/** 画布加载失败后的手动重试 */
function retryCanvasLoad(): void {
  canvasLoadFailed.value = false;
  canvasKey.value += 1;
}

const props = withDefaults(defineProps<{
  /** 是否显示(embedded 模式按 activeSection 切换;standalone 恒 true) */
  show?: boolean;
}>(), {
  show: true,
});

const {
  flowDraft, flowSaving, flowMsg, editingStepId, dragStepId, flowId, flowName, flowDesc,
  flowImportInput, flowImporting, flowLibFlows,
  loadFlowConfig, onFlowSelect, newFlow, duplicateFlow, deleteFlowNow, onFlowImport,
  exportFlowNow, exportAllFlows, saveFlowNow, addStep, removeStep, moveStep, onStepActionChange,
  onStepDragStart, onStepDragOver, onStepDrop, onStepDragEnd,
  setFlowParallel,
} = useAgentFlow();

/** 视图模式:列表为默认(WF-10);本地状态,不写 store(组件直改 store state 会被 check-arch 拦) */
const flowView = ref<'list' | 'canvas'>('list');

// 节点级连接候选(二维批次 5b):只读拉一次已落库的连接列表,下传给步骤表单与画布。
// 分区每次可见时重拉,故「先在连接配置区改连接、再切回执行流程」不会用到陈旧列表。
const flowConn = useFlowConnections(computed(() => props.show !== false));

// 任务工具策略(收口批 2026-09-24):步骤白名单会与策略编译集取交,交集为空时该节点
// 不下发任何工具(闸门 fail-closed)。策略值取自设置 store,仅供编辑区本地提示使用
// (只读;保存与执行判定始终在后端)。
const store = useAppStore();
const { taskToolPolicy, taskToolAllowlist } = storeToRefs(store);
const toolPolicyCtx = computed<ToolPolicyCtx>(() => ({
  policy: taskToolPolicy.value,
  allowlist: taskToolAllowlist.value,
}));

const flowSteps = computed(() => flowDraft.value?.steps ?? []);

/** 流程级提示(线性流程返回 null,不增加噪音) */
const graphHintText = computed(() =>
  graphHint(flowDraft.value?.steps ?? [], flowDraft.value?.max_parallel_nodes),
);

onMounted(async () => {
  // 加载自定义 Agent 执行流程(custom 模式)
  await loadFlowConfig();
  // 连接候选(SSR 不触发 onMounted → 测试期不发请求;失败只记提示,不挡流程编辑)
  void flowConn.load();
});
</script>

<template>
  <div v-show="show" class="sv-field">
    <div class="sv-field-label"><span class="sv-supreme orange" /> Agent 执行流程</div>
    <div class="sv-stack">
      <!-- 流程库管理:选择 / 新建 / 复制 / 删除 / 导入 / 导出 -->
      <div class="sv-inp-row">
        <label class="sv-inp-tag">当前流程</label>
        <select
          v-model="flowId"
          class="sv-select flow-select-wide"
          :disabled="!flowLibFlows.length"
          @change="onFlowSelect"
        >
          <option v-for="f in flowLibFlows" :key="f.id" :value="f.id">
            {{ f.name || '未命名流程' }}{{ f.enabled ? '' : '(未启用)' }}
          </option>
        </select>
        <span class="flow-lib-actions">
          <button class="sv-btn ghost sv-btn-square" title="新建流程" @click="newFlow">+</button>
          <button class="sv-btn ghost sv-btn-square" title="复制当前流程" @click="duplicateFlow">⧉</button>
          <button class="sv-btn danger sv-btn-square" title="删除当前流程" @click="deleteFlowNow">✕</button>
        </span>
      </div>
      <div class="sv-inp-row">
        <span class="flow-meta-pair">
          <label class="sv-inp-tag">流程名称</label>
          <input v-model="flowName" type="text" class="sv-input" placeholder="流程名称(保存时生效)" spellcheck="false" />
        </span>
        <span class="flow-meta-pair">
          <label class="sv-inp-tag flow-meta-gap">说明</label>
          <input v-model="flowDesc" type="text" class="sv-input" placeholder="流程说明(可选)" spellcheck="false" />
        </span>
      </div>
      <div class="sv-btn-row">
        <button class="sv-btn ghost sv-btn-fill" :disabled="flowImporting" @click="flowImportInput?.click()">
          {{ flowImporting ? '导入中...' : '导入流程(JSON)' }}
        </button>
        <button class="sv-btn ghost sv-btn-fill" :disabled="!flowDraft" @click="exportFlowNow">
          导出流程(JSON)
        </button>
        <button
          class="sv-btn ghost sv-btn-fill"
          :disabled="!flowLibFlows.length"
          @click="exportAllFlows"
        >
          导出全部流程
        </button>
      </div>
      <div class="sv-note">
        导出会带上该流程挂载的子流程(缺了它们,导入方必然报「引用的子流程不存在」);
        导出文件不含 API 连接与密钥,节点上的连接引用跨机后需重选。
      </div>
      <input
        ref="flowImportInput"
        type="file"
        accept=".json,application/json"
        class="hidden"
        @change="onFlowImport"
      />
      <div class="sv-inp-row">
        <label class="sv-inp-tag">启用</label>
        <button
          v-if="flowDraft"
          class="sv-btn ghost"
          :class="{ 'sv-btn-on': flowDraft.enabled }"
          @click="flowDraft.enabled = !flowDraft.enabled"
        >
          {{ flowDraft.enabled ? '已启用' : '已关闭' }}
        </button>
        <span v-else class="sv-note">加载中...</span>
      </div>
      <div v-if="flowDraft" class="sv-inp-row">
        <label class="sv-inp-tag">并行上限</label>
        <input
          :value="flowDraft.max_parallel_nodes ?? ''"
          type="number"
          min="1"
          max="8"
          class="sv-input inject-num"
          placeholder="默认 2"
          title="同一层最多同时执行的步骤数(1 = 完全串行);并行会成倍消耗 token"
          @change="setFlowParallel(($event.target as HTMLInputElement).value)"
        />
        <span class="sv-note">同一层的步骤最多同时跑这么多个;并行会成倍消耗 token。</span>
      </div>
      <p v-if="graphHintText" class="sv-note">{{ graphHintText }}</p>
      <p class="sv-note">
        自定义流程:输入栏切换到 <b>CUSTOM</b> 模式后,按下方步骤从上到下依次执行
        (设了上游则按依赖顺序执行);每步可独立设置系统提示词(支持酒馆宏,以
        <code v-pre>[本步指令]</code> 追加到系统提示词末尾)、生成参数与工具范围。
        至少需要一步「生成正文」的 direct 步骤。
      </p>
      <!-- 视图切换(二维批次 3):列表默认,画布是高级模式 -->
      <div v-if="flowDraft" class="sv-inp-row">
        <label class="sv-inp-tag">视图</label>
        <button
          class="sv-btn ghost"
          :class="{ 'sv-btn-on': flowView === 'list' }"
          title="列表视图:一维/二维都能编辑,默认视图"
          @click="flowView = 'list'"
        >
          列表
        </button>
        <button
          class="sv-btn ghost"
          :class="{ 'sv-btn-on': flowView === 'canvas' }"
          title="画布视图:可视化编辑节点与依赖连线"
          @click="flowView = 'canvas'"
        >
          画布
        </button>
        <span class="sv-note">画布是高级模式;列表视图同样能编辑二维依赖。</span>
      </div>
      <div v-if="flowDraft && !flowDraft.steps.length" class="sv-note inject-empty">
        尚未添加步骤,点击下方「+ 新增步骤」。
      </div>
      <template v-if="flowView === 'list'">
        <div
          v-for="step in flowSteps"
          :key="step.id"
          class="flow-row"
          :class="{ dragging: dragStepId === step.id }"
          draggable="true"
          @dragstart="onStepDragStart($event, step.id)"
          @dragover="onStepDragOver($event, step.id)"
          @drop="onStepDrop($event, step.id)"
          @dragend="onStepDragEnd"
        >
          <span class="floor-grip" title="拖拽排序">⋮⋮</span>
          <button
            class="sv-btn ghost"
            :class="{ 'sv-btn-on': step.enabled }"
            :title="step.enabled ? '点击停用' : '点击启用'"
            @click="step.enabled = !step.enabled"
          >
            {{ step.enabled ? '开' : '关' }}
          </button>
          <input v-model="step.name" type="text" class="sv-input floor-name" placeholder="步骤名称" spellcheck="false" />
          <select
            v-model="step.action"
            class="sv-select floor-select"
            title="动作:direct=执行/生成,reflect=反思(不生成)"
            @change="onStepActionChange(step)"
          >
            <option value="direct">执行</option>
            <option value="reflect">反思</option>
          </select>
          <button
            v-if="step.action === 'direct'"
            class="sv-btn ghost"
            :class="{ 'sv-btn-on': step.generates }"
            :title="step.generates ? '该步生成正文' : '该步不生成(如理解意图)'"
            @click="step.generates = !step.generates"
          >
            {{ step.generates ? '生成' : '不生成' }}
          </button>
          <div class="floor-actions">
            <button class="sv-btn ghost sv-btn-square" title="上移" @click="moveStep(step.id, -1)">↑</button>
            <button class="sv-btn ghost sv-btn-square" title="下移" @click="moveStep(step.id, 1)">↓</button>
            <button
              class="sv-btn ghost sv-btn-square"
              :title="editingStepId === step.id ? '收起编辑' : '编辑详情'"
              @click="editingStepId = editingStepId === step.id ? null : step.id"
            >
              ✎
            </button>
            <button class="sv-btn danger sv-btn-square" title="删除步骤" @click="removeStep(step.id)">✕</button>
          </div>
          <!-- 二维依赖摘要:层级 + 上游(线性流程不显示,避免给一维用户增加噪音) -->
          <span v-if="!isLinearCompat(flowSteps)" class="flow-graph-note">
            {{ upstreamSummary(flowSteps, step) }}
            <template v-if="subFlowSummary(flowLibFlows, step)">
              · {{ subFlowSummary(flowLibFlows, step) }}
            </template>
            <template v-if="connectionSummary(flowConn.options.value, step)">
              · {{ connectionSummary(flowConn.options.value, step) }}
            </template>
          </span>
          <!-- 挂载子流程的摘要在**线性流程**里也要显示:它是执行语义的一部分,不属于二维依赖
               (节点级连接同理:它改了这一步走哪个 provider,不属二维依赖,但也是执行语义) -->
          <span
            v-else-if="subFlowSummary(flowLibFlows, step) || connectionSummary(flowConn.options.value, step)"
            class="flow-graph-note"
          >
            <template v-if="subFlowSummary(flowLibFlows, step)">
              {{ subFlowSummary(flowLibFlows, step) }}
            </template>
            <template v-if="connectionSummary(flowConn.options.value, step)">
              {{ subFlowSummary(flowLibFlows, step) ? ' · ' : '' }}{{ connectionSummary(flowConn.options.value, step) }}
            </template>
          </span>
          <!-- 展开编辑区:跨整行、纵向堆叠,避免被步骤行 grid 挤压 -->
          <AgentFlowStepEditor
            v-if="editingStepId === step.id"
            :step="step"
            :steps="flowSteps"
            :flows="flowLibFlows"
            :connections="flowConn.options.value"
            :current-flow-id="flowId ?? ''"
            :flow-enabled="flowDraft?.enabled ?? true"
            :tool-policy="toolPolicyCtx"
          />
        </div>
      </template>
      <AgentFlowCanvas
        v-else
        :key="canvasKey"
        :steps="flowSteps"
        :flows="flowLibFlows"
        :connections="flowConn.options.value"
        :current-flow-id="flowId ?? ''"
        :flow-enabled="flowDraft?.enabled ?? true"
        :tool-policy="toolPolicyCtx"
        @remove="removeStep"
      />
      <p v-if="flowView === 'canvas' && canvasLoadFailed" class="sv-note flow-tool-warn">
        画布组件加载失败(多为前端已更新、页面缓存的旧 chunk 失效)。
        可点右侧「重试」;若仍失败,请刷新页面,或先用列表视图编辑(功能一致)。
        <button class="sv-btn ghost" @click="retryCanvasLoad">重试</button>
      </p>
      <div class="sv-btn-row">
        <button v-if="flowDraft" class="sv-btn ghost sv-btn-fill" @click="addStep">+ 新增步骤</button>
      </div>
      <div class="sv-btn-row">
        <button class="sv-btn primary sv-btn-fill" :disabled="flowSaving || !flowDraft" @click="saveFlowNow">
          {{ flowSaving ? '保存中...' : '保存执行流程' }}
        </button>
        <div
          v-if="flowMsg"
          class="sv-feedback-flex"
          :class="flowMsg.includes('失败') ? 'err' : 'ok'"
        >{{ flowMsg }}</div>
      </div>
    </div>
  </div>
</template>
