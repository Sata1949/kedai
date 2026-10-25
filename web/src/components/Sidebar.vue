<script setup lang="ts">
// 左侧功能区壳（C-2A，2026-10-25 拆分后）。
// 只保留布局与模式分支；具体内容见 components/sidebar/：
//   SidebarTop（品牌/统计/模式切换） · CharacterListPanel（搜索/列表/右键与长按/提示词编辑）
//   TaskComposer（下达目标 + 执行设置 + 工作区画像） · TaskHistoryList（任务历史）
//   SidebarFooter（导入/综合设置/退出）。
import { useAppStore } from '../store';
import { storeToRefs } from 'pinia';
import SidebarTop from './sidebar/SidebarTop.vue';
import CharacterListPanel from './sidebar/CharacterListPanel.vue';
import TaskComposer from './sidebar/TaskComposer.vue';
import TaskHistoryList from './sidebar/TaskHistoryList.vue';
import SidebarFooter from './sidebar/SidebarFooter.vue';

const store = useAppStore();
const { appMode } = storeToRefs(store);
</script>

<template>
  <!-- open 类仅移动端生效(窄屏下 Sidebar 为覆盖式抽屉,见 style.css 移动端区块);
       桌面端该断点不匹配,布局与此前完全一致。 -->
  <aside class="sv-sidebar" :class="{ wide: appMode === 'task', open: store.sidebarOpen }">
    <SidebarTop />

    <!-- 中部:角色扮演模式 = 搜索 + 角色列表;任务模式 = 下达目标 + 任务历史。
         ANDUI-4:任务模式挂 sv-side-body-task(grow-不缩,见 mobile.css 窄屏区)——
         否则 wrapper 与 .sv-task-scroll 双重可缩会把任务列表压成 12px 细条;
         角色扮演分支不挂钩子,行为与改前逐字一致。 -->
    <div class="flex min-h-0 flex-1 flex-col px-3 pb-2" :class="{ 'sv-side-body-task': appMode === 'task' }">
      <Transition name="sv-fade" mode="out-in">
        <CharacterListPanel v-if="appMode === 'roleplay'" key="roleplay" />
        <!-- 任务模式:下达目标 + 任务历史(承接任务工作台左栏,单列布局)。
             sv-task-scroll(UIP-4):表单+历史合并为单一滚动区,页脚恒固定;
             自定义流程表单超高时不再压住「综合设置」(规则在 styles/task.css)。 -->
        <div v-else key="task" class="sv-task-scroll flex min-h-0 flex-1 flex-col">
          <TaskComposer />
          <TaskHistoryList />
        </div>
      </Transition>
    </div>

    <SidebarFooter />
  </aside>
</template>
