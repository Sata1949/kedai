<script setup lang="ts">
// 画布节点卡片(二维批次 3 前端)。
//
// 只负责把一个步骤画成卡片:层级、名称、动作/生成/成果/停用四个徽标。
// 上下各一个连线端口(Handle):**上游在上、下游在下**——自动布局是自上而下的链,
// 从某个节点底部拖到另一个节点顶部,语义就是「把上面那步接成下面那步的上游」。
//
// 样式写在本组件 `<style scoped>`(MAINTENANCE.md 明令禁止再往 style.css 追加),
// 只覆盖 Vue Flow 的 CSS 变量来接管主题,不用 :deep 穿透它的内部 DOM。
import { Handle, Position } from '@vue-flow/core';
import type { FlowCanvasNodeData } from '../../composables/useFlowCanvas';

const props = defineProps<{
  data: FlowCanvasNodeData;
  selected?: boolean;
}>();
</script>

<template>
  <div class="flow-card" :class="{ off: !props.data.step.enabled, on: props.selected }">
    <Handle type="target" :position="Position.Top" />
    <div class="flow-card-head">
      <span class="flow-card-name">{{ props.data.step.name || '未命名步骤' }}</span>
      <span class="flow-card-level">
        {{ props.data.level === null ? '层级未定' : `第 ${props.data.level} 层` }}
      </span>
    </div>
    <div class="flow-card-tags">
      <span class="flow-tag">{{ props.data.step.action === 'reflect' ? '反思' : '执行' }}</span>
      <span v-if="props.data.step.action === 'direct' && props.data.step.generates" class="flow-tag">
        生成
      </span>
      <!-- 节点档位(二维批次 6a):严格节点单次调用、不下发工具 -->
      <span v-if="props.data.step.kind === 'strict'" class="flow-tag kind">严格</span>
      <span v-if="props.data.isOutput" class="flow-tag out">成果</span>
      <span v-if="!props.data.step.enabled" class="flow-tag off">已停用</span>
    </div>
    <Handle type="source" :position="Position.Bottom" />
  </div>
</template>

<style scoped>
.flow-card {
  width: 196px;
  box-sizing: border-box;
  padding: 8px 10px;
  border: var(--bw-thin) solid var(--sv-ink);
  background: var(--sv-white);
  font-size: 12px;
  line-height: 1.4;
}
.flow-card.on {
  border-color: var(--sv-pink-dark);
  box-shadow: var(--shadow-pink);
}
/* 停用步骤用虚线边框 + 降透明度:画布上仍要看得见(它还在流程里),但一眼可辨不参与执行 */
.flow-card.off {
  border-style: dashed;
  opacity: 0.62;
}
.flow-card-head {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 6px;
}
.flow-card-name {
  font-weight: 700;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.flow-card-level {
  flex: 0 0 auto;
  font-size: 10px;
  color: var(--sv-ink-faint);
}
.flow-card-tags {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
  margin-top: 6px;
}
.flow-tag {
  padding: 0 4px;
  border: 1px solid var(--sv-line-strong);
  font-size: 10px;
  color: var(--sv-ink-dim);
}
.flow-tag.out {
  border-color: var(--sv-pink-dark);
  color: var(--sv-pink-dark);
  font-weight: 700;
}
/* 档位徽标用虚线:与「停用」的实线灰、成果的粉线区分开,一眼看出是「模式」而非状态 */
.flow-tag.kind {
  border-style: dashed;
  color: var(--sv-ink-dim);
}
.flow-tag.off {
  border-color: var(--sv-ink-faint);
  color: var(--sv-ink-faint);
}
</style>
