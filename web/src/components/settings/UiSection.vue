<script setup lang="ts">
// 设置区:界面(Agent 面板开关 / 安全 HTML 渲染 / 角色卡 JavaScript 授权管理)。
// 从 SettingsModal.vue 双模板合并而来:取 embedded 版本(授权说明与撤销列表更完整)。
import { useAppStore } from '../../store';
import { storeToRefs } from 'pinia';
import type { useDataManager } from '../../composables/useDataManager';

const props = withDefaults(defineProps<{
  /** useDataManager 的返回对象(壳共享实例,本区仅用 characterLabel / 撤销授权) */
  state: ReturnType<typeof useDataManager>;
  /** 是否显示(embedded 模式按 activeSection 切换;standalone 恒 true) */
  show?: boolean;
}>(), {
  show: true,
});

const { characterLabel, confirmRevokeScriptAuthorization } = props.state;

const store = useAppStore();
const { scriptAuthorizations } = storeToRefs(store);
</script>

<template>
  <div v-show="props.show" class="sv-field">
    <div class="sv-field-label"><span class="sv-supreme yellow" /> 界面</div>
    <div class="sv-datalist">
      <!-- 默认模式(2026-09-27 首启引导批次):偏好决定每次启动进入的模式,
           未设置时沿用旧行为「按上次用过的模式进入」。放在本区首行——它比下面几项
           更基础(决定启动后落到哪个工作台),且与顶栏/底部导航的模式切换互为补充。 -->
      <div class="sv-data-row">
        <div class="info">
          <b>默认模式</b>
          <span>决定每次启动 Kedai 后默认进入的模式（未设置时按上次使用过的模式进入）。点击即切换并保存，首次启动的引导里选的也是这一项。</span>
        </div>
        <!-- 两按钮组:shrink-0 + nowrap 是必须的——行内说明文字会占满宽度,不给这两条则按钮被压成
             竖排折行(实测「角色扮演」折成 4 行);选中态用 sv-btn-on(粉底)表示,未设置过时两者都不高亮。 -->
        <div class="flex items-center gap-2 shrink-0 whitespace-nowrap">
          <button
            type="button"
            class="sv-btn ghost"
            :class="{ 'sv-btn-on': store.defaultAppMode === 'roleplay' }"
            :aria-pressed="store.defaultAppMode === 'roleplay'"
            @click="store.setDefaultAppMode('roleplay')"
          >
            角色扮演
          </button>
          <button
            type="button"
            class="sv-btn ghost"
            :class="{ 'sv-btn-on': store.defaultAppMode === 'task' }"
            :aria-pressed="store.defaultAppMode === 'task'"
            @click="store.setDefaultAppMode('task')"
          >
            任务
          </button>
        </div>
      </div>
      <div class="sv-data-row">
        <div class="info">
          <b>Agent 面板</b>
          <span>生成时自动展开,展示步骤、推理链与工具调用</span>
        </div>
        <button class="sv-btn ghost" @click="store.toggleAgentPanel()">
          {{ store.agentPanelOpen ? '展开中' : '已折叠' }}
        </button>
      </div>
      <div class="sv-data-row">
        <div class="info">
          <b>安全 HTML 渲染</b>
          <span>应用角色卡正则替换并清理 script/style 属性、事件处理器、javascript URL 与外部资源；不代表允许 JavaScript。开关按角色卡记忆,未设置的角色卡回退全局默认</span>
        </div>
        <button type="button" class="sv-btn ghost" :aria-pressed="store.renderHtml" @click="store.toggleRenderHtml()">
          {{ store.renderHtml ? '已开启' : '已关闭' }}
        </button>
      </div>
      <div class="sv-data-row" style="align-items: flex-start">
        <div class="info">
          <b>角色卡 JavaScript 授权</b>
          <span>默认禁用；按角色 ID 与脚本哈希授权，内容变化后自动失效。授权记录仅保存在当前浏览器配置中。</span>
          <div v-if="scriptAuthorizations.length" style="display: grid; gap: 8px; margin-top: 10px">
            <div v-for="grant in scriptAuthorizations" :key="grant.characterId" class="flex items-center gap-2">
              <span>{{ characterLabel(grant.characterId) }} · {{ new Date(grant.authorizedAt).toLocaleString('zh-CN', { hour12: false }) }}</span>
              <button type="button" class="sv-btn ghost sv-btn-sm" @click="confirmRevokeScriptAuthorization(grant.characterId)">撤销</button>
            </div>
          </div>
          <span v-else>暂无已授权角色卡。</span>
        </div>
      </div>
      <!-- 退出应用(2026-09-17):此前界面上没有退出入口,只能靠 Android 返回键
           碰运气触发,或桌面关窗口。放在「界面」区末尾——危险操作不与其它设置项混排,
           且点击后走同一个退出确认弹窗(不直接退,误触不丢未保存内容)。 -->
      <div class="sv-data-row">
        <div class="info">
          <b>退出应用</b>
          <span>结束 Kedai 进程(正在生成或执行中的任务会被中断)。Android 上也可连按两次返回键退出。</span>
        </div>
        <button type="button" class="sv-btn danger" @click="store.exitConfirmOpen = true">退出 Kedai</button>
      </div>
    </div>
  </div>
</template>
