<script setup lang="ts">
// 画布连线组件(2026-09-21 画布交互补完)。
//
// 为什么需要它:画布库的默认边只有一条路径 —— 批次 3 关闭了键盘删除(`:delete-key-code="null"`),
// 于是画布内**没有任何断开入口**,只能删节点重建或切回列表视图取消勾选(`useFlowCanvas.disconnect`
// 一度是无人调用的死代码)。这里在路径中点加一个「×」按钮,断开动作仍交给
// `useFlowCanvas.disconnect`(合法性判定与中文文案的唯一出处)。
//
// 隐式边(线性兼容流程的串行链)不画按钮:它没有对应的 `inputs` 记录,「断开」无从表达;
// 要改串联关系得先给步骤显式设置上游(列表视图的上游勾选,或画布拖一条新连线)。
import { computed } from 'vue';
import { BaseEdge, getBezierPath, type EdgeProps } from '@vue-flow/core';

const props = defineProps<EdgeProps & { data?: { implicit?: boolean } }>();

const emit = defineEmits<{
  /** 请求断开这条连线(source 从 target 的上游里移除) */
  disconnect: [id: string];
}>();

const path = computed(
  () =>
    getBezierPath({
      sourceX: props.sourceX,
      sourceY: props.sourceY,
      sourcePosition: props.sourcePosition,
      targetX: props.targetX,
      targetY: props.targetY,
      targetPosition: props.targetPosition,
    })[0],
);

const implicit = computed(() => props.data?.implicit === true);

/** 断开按钮落在两端点连线的中点(边短且以纵向为主,与曲线中点的视觉差可忽略;
 *  不用 `labelX/labelY`:它们不在 EdgeProps 的公开类型里) */
const midX = computed(() => (props.sourceX + props.targetX) / 2);
const midY = computed(() => (props.sourceY + props.targetY) / 2);
</script>

<template>
  <BaseEdge :path="path" :marker-end="markerEnd" />
  <g
    v-if="!implicit"
    class="flow-edge-del"
    role="button"
    aria-label="断开这条连线"
    :transform="`translate(${midX}, ${midY})`"
    @click.stop="emit('disconnect', id)"
  >
    <title>断开这条连线</title>
    <circle r="8" />
    <text y="3.5" text-anchor="middle">×</text>
  </g>
</template>

<style scoped>
/* 静息态半透明、悬停/聚焦才实心:可发现但不喧宾夺主(窄屏没有 hover,故不用 opacity: 0) */
.flow-edge-del {
  cursor: pointer;
  opacity: 0.45;
}
.flow-edge-del:hover {
  opacity: 1;
}
.flow-edge-del circle {
  fill: var(--sv-white);
  stroke: var(--sv-ink-dim);
  stroke-width: 1;
}
.flow-edge-del text {
  font-size: 11px;
  line-height: 1;
  fill: var(--sv-ink-dim);
  user-select: none;
}
.flow-edge-del:hover circle {
  stroke: var(--sv-pink-dark);
}
.flow-edge-del:hover text {
  fill: var(--sv-pink-dark);
}
</style>
