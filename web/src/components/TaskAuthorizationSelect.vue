<script setup lang="ts">
// 任务模式「工具授权」三档选择器(2026-11-06):Sidebar「下达目标 → 执行设置」区使用。
//
// 三档语义(后端为准,前端只做选择与提示):
//  - step:写/执行类工具逐条等用户确认(只读工具自动放行);
//  - workspace:工作区内工具与命令自动放行,bash 越界/无法判定时转确认;
//  - auto:任务策略内全部自动执行(破坏性/提权命令逐条写审计;屏幕/输入类仍拒)。
//
// 默认 workspace = 本档上线前的行为(工具自动执行、高危命令直接拒绝),故旧用户不受影响。
import { computed } from 'vue';
import { storeToRefs } from 'pinia';
import { useAppStore } from '../store';
import type { TaskAuthorizationTier } from '../api/types';

const store = useAppStore();
const { taskAuthorizationTier } = storeToRefs(store);

/** 本地可写代理:v-model 走 select 的字符串值,写入回落 store action。 */
const tierModel = computed({
  get: () => taskAuthorizationTier.value,
  set: (v: TaskAuthorizationTier) => store.setTaskAuthorizationTier(v),
});

/** 每档的说明文案(与后端裁决口径逐条对应,不写「完全」这种超出能力的承诺)。 */
const HINTS: Record<TaskAuthorizationTier, string> = {
  step: '写文件、改文件、执行命令前会暂停等待你确认;只读工具不受影响。',
  workspace: '工作区内的读写与命令自动放行;越界或无法判定的命令转确认。未绑定工作区时全部转确认。',
  auto: '任务策略内的工具与命令全部自动执行,破坏性/提权命令逐条记入执行审计;屏幕/输入类仍一律拒绝。',
};

const hint = computed(() => HINTS[tierModel.value]);
</script>

<template>
  <div class="sv-auth-tier">
    <div class="sv-inp-row">
      <select
        v-model="tierModel"
        class="sv-select"
        title="任务模式工具授权:逐步 = 写/执行类工具逐条确认;工作区内 = 区内自动放行、越界转确认;自动 = 任务策略内全部自动执行(屏幕/输入类仍拒)。仅对新建任务生效"
      >
        <option value="step">授权：逐步（写操作前逐条确认）</option>
        <option value="workspace">授权：工作区内（区内自动放行）</option>
        <option value="auto">授权：自动（任务策略内全部放行）</option>
      </select>
    </div>
    <!-- 说明放在行外:桌面上 320px 窄栏里行内提示会与下拉抢宽度 -->
    <p class="sv-note" :class="{ 'flow-tool-warn': tierModel === 'auto' }">{{ hint }}</p>
  </div>
</template>

<style scoped>
/* 样式只做纵向留白与折行;配色/字号沿用全局 token 与 .sv-note */
.sv-auth-tier {
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
}

.sv-auth-tier .sv-note {
  margin: 0;
  overflow-wrap: anywhere;
}
</style>
