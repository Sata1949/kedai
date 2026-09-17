<script setup lang="ts">
// 设置区:授权与命令执行(2026-09-17 独立成二级项)。
//
// 为什么独立:此前「授权管理」(工具三档)与「执行器等级(Android)」都嵌在
// 「Agent 设置」的长表单内部,用户(尤其移动端)找不到 root/Shizuku 授权入口。
// 授权是需要被主动发现的设置项,现提升为「Agent 与任务 → 授权与命令执行」二级项。
//
// 分区内容 = 工具三档授权(AuthorizationSection)+ 命令执行与设备档位
// (AndroidExecSection);两者原本就是相邻的两个 sv-separator 块,本次只换外层容器,
// 组件内部逻辑零改动。
import AuthorizationSection from './AuthorizationSection.vue';
import AndroidExecSection from './AndroidExecSection.vue';

withDefaults(defineProps<{ show?: boolean }>(), { show: true });
</script>

<template>
  <div v-show="show" class="sv-field">
    <div class="sv-field-label"><span class="sv-supreme red" /> 授权与命令执行</div>
    <div class="sv-stack">
      <div class="sv-separator">
        <div class="sv-field-label sub">授权管理</div>
        <AuthorizationSection :show="show" />
      </div>

      <div class="sv-separator">
        <AndroidExecSection :show="show" />
      </div>
    </div>
  </div>
</template>
