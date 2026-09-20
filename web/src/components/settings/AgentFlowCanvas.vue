<script setup lang="ts">
// 设置区:自定义流程的二维画布视图(二维批次 3)。
//
// **画布是高级模式**:列表视图仍是默认视图(WF-10 产品约束,`展望.md`)——门槛低的列表
// 给所有用户,表达力强的画布给需要的人,存量一维流程进画布也自动排成纵向链、不惊吓用户。
//
// 分工:数据与交互逻辑全在 `composables/useFlowCanvas`(与画布库解耦、可在 node 环境单测),
// 本组件只把它映射成 @vue-flow 的容器/节点/连线,并把画布事件转回逻辑层。
// **打开画布不改动流程数据**:只有用户显式动作(拖拽结束 / 重新布局)才把坐标写回步骤,
// 保存流程时才落盘——保证存量一维流程的 JSON 不会因为「看了一眼画布」而变化。
//
// 本文件是体积敏感点:@vue-flow/* 与它的传递依赖(d3-*、@vueuse/core)必须只被本组件的
// 异步 chunk 引用,在 `vite.config.ts` 的 manualChunks 里单独归组(见该文件注释),
// 否则会被并进首屏的 vue-vendor。
import { computed } from 'vue';
import {
  VueFlow,
  useVueFlow,
  type Connection,
  type NodeDragEvent,
  type NodeMouseEvent,
} from '@vue-flow/core';
import { Background } from '@vue-flow/background';
import { Controls } from '@vue-flow/controls';
import { MiniMap } from '@vue-flow/minimap';
import AgentFlowNodeCard from './AgentFlowNodeCard.vue';
import AgentFlowStepEditor from './AgentFlowStepEditor.vue';
import { useFlowCanvas } from '../../composables/useFlowCanvas';
import { normalizeStepAction } from '../../utils/agentFlowTools';
import type { AgentFlowStep } from '../../api/types';

import '@vue-flow/core/dist/style.css';
import '@vue-flow/core/dist/theme-default.css';
import '@vue-flow/controls/dist/style.css';
import '@vue-flow/minimap/dist/style.css';

const props = defineProps<{
  /** 当前流程的全部步骤(就地编辑;所有权归调用方) */
  steps: AgentFlowStep[];
}>();

const emit = defineEmits<{
  /** 请求删除某个步骤(由调用方走 useAgentFlow.removeStep,连带清理悬空上游引用) */
  remove: [id: string];
}>();

const canvas = useFlowCanvas(() => props.steps);
const { fitView } = useVueFlow();

/** 当前打开 Inspector 的步骤(节点被删掉后自动收起) */
const inspecting = computed(
  () => props.steps.find((s) => s.id === canvas.inspectingId.value) ?? null,
);

/** 只拦自环:自环对任何既有边都不成立,所以放进 is-valid-connection 是安全的
 *  (该回调也参与既有边的过滤,把成环判断塞进来会把合法边一并丢掉)。
 *  重复上游与成环在 connect 里拦,并给中文原因。 */
function isValidConnection(c: Connection): boolean {
  return c.source !== c.target;
}

function onConnect(c: Connection): void {
  canvas.connect(c.source, c.target);
}

function onDragStop(e: NodeDragEvent): void {
  canvas.onNodeDragStop(e.node.id, e.node.computedPosition);
}

function onNodeClick(e: NodeMouseEvent): void {
  canvas.inspect(e.node.id);
}

function onRelayout(): void {
  if (canvas.hasLayout.value && !window.confirm('重新布局会覆盖你手工拖拽的节点位置,确定继续?')) {
    return;
  }
  canvas.relayout();
}
</script>

<template>
  <div class="flow-canvas">
    <div class="flow-canvas-main">
      <div class="flow-canvas-bar">
        <button class="sv-btn ghost" title="按依赖层级重新排列节点" @click="onRelayout">重新布局</button>
        <button class="sv-btn ghost" title="缩放到正好看全整个流程" @click="fitView({ padding: 0.2 })">
          适应视图
        </button>
        <span v-if="canvas.canvasMsg.value" class="flow-canvas-msg">{{ canvas.canvasMsg.value }}</span>
      </div>
      <VueFlow
        :nodes="canvas.nodes.value"
        :edges="canvas.edges.value"
        :min-zoom="0.3"
        :max-zoom="1.6"
        :delete-key-code="null"
        :is-valid-connection="isValidConnection"
        class="flow-canvas-graph"
        @init="fitView({ padding: 0.2 })"
        @connect="onConnect"
        @node-drag-stop="onDragStop"
        @node-click="onNodeClick"
      >
        <template #node-flowStep="nodeProps">
          <AgentFlowNodeCard :data="nodeProps.data" :selected="nodeProps.selected" />
        </template>
        <Background :gap="16" :size="1" pattern-color="var(--sv-line-strong)" />
        <Controls :show-interactive="false" />
        <MiniMap pannable zoomable :node-color="'var(--sv-pink-light)'" />
      </VueFlow>
      <p class="sv-note">
        拖动节点调整位置;从节点<b>底部</b>圆点拖到另一个节点<b>顶部</b>圆点 = 把前者设为后者的上游;
        点节点在右侧逐字段编辑。位置与连线都要点下方「保存执行流程」后才落盘。
      </p>
    </div>
    <aside v-if="inspecting" class="flow-canvas-side">
      <div class="flow-side-head">
        <input v-model="inspecting.name" type="text" class="sv-input" placeholder="步骤名称" spellcheck="false" />
        <button class="sv-btn ghost sv-btn-square" title="关闭编辑" @click="canvas.inspect(null)">✕</button>
      </div>
      <div class="sv-inp-row">
        <label class="sv-inp-tag">动作</label>
        <select
          v-model="inspecting.action"
          class="sv-select"
          title="动作:direct=执行/生成,reflect=反思(不生成)"
          @change="normalizeStepAction(inspecting)"
        >
          <option value="direct">执行</option>
          <option value="reflect">反思</option>
        </select>
        <button
          v-if="inspecting.action === 'direct'"
          class="sv-btn ghost"
          :class="{ 'sv-btn-on': inspecting.generates }"
          :title="inspecting.generates ? '该步生成正文' : '该步不生成(如理解意图)'"
          @click="inspecting.generates = !inspecting.generates"
        >
          {{ inspecting.generates ? '生成' : '不生成' }}
        </button>
      </div>
      <div class="sv-inp-row">
        <label class="sv-inp-tag">启用</label>
        <button
          class="sv-btn ghost"
          :class="{ 'sv-btn-on': inspecting.enabled }"
          :title="inspecting.enabled ? '点击停用' : '点击启用'"
          @click="inspecting.enabled = !inspecting.enabled"
        >
          {{ inspecting.enabled ? '已启用' : '已停用' }}
        </button>
        <button class="sv-btn danger" title="删除该步骤" @click="emit('remove', inspecting.id)">删除步骤</button>
      </div>
      <AgentFlowStepEditor :step="inspecting" :steps="props.steps" />
    </aside>
  </div>
</template>

<style scoped>
.flow-canvas {
  display: flex;
  align-items: stretch;
  gap: 8px;
}
.flow-canvas-main {
  flex: 1 1 auto;
  min-width: 0;
}
.flow-canvas-bar {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
  margin-bottom: 6px;
}
.flow-canvas-msg {
  font-size: 11px;
  line-height: 1.5;
  color: var(--sv-ink-dim);
}
/* Vue Flow 主题接管:只覆盖它的 CSS 变量(它是 :root 级变量),不动它的内部 DOM */
.flow-canvas-graph {
  --vf-node-bg: transparent;
  --vf-node-text: var(--sv-ink);
  --vf-connection-path: var(--sv-ink-dim);
  --vf-handle: var(--sv-ink);
  height: 460px;
  border: var(--bw-thin) solid var(--sv-ink);
  background: var(--sv-paper);
}
.flow-canvas-side {
  flex: 0 0 300px;
  max-height: 520px;
  overflow: auto;
  padding: 8px;
  border: var(--bw-thin) solid var(--sv-ink);
  background: var(--sv-white);
}
.flow-side-head {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-bottom: 6px;
}
/* 窄屏(手机/分栏窄)改为上下叠放,画布不再被 Inspector 挤扁 */
@media (max-width: 900px) {
  .flow-canvas {
    flex-direction: column;
  }
  .flow-canvas-side {
    flex: 1 1 auto;
    max-height: none;
  }
}
</style>
