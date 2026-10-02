<script setup lang="ts">
// 设置区:编码能力包(默认关;须用户显式开启)。
//
// 语义:开启后,任务模式执行者的**默认**系统提示词改用「编码执行者模板」——强调先读后写、
// 遵循既有风格、改完跑验证、最小改动;用户若在提示词框里自定义过,则其自定义值优先,
// 本开关**只影响默认值**。仅任务模式生效,**不影响角色扮演模式**。
//
// 形态与体例:单开关分区,开关行照 DataManagementSection 的「回退快照(undo)」行
// (info 标题+说明 / 右侧开关),保存走 store.queueSettingsSave(既有串行保存队列,
// 与 AndroidExecSection 同一路径),不新造保存通道;失败回滚本地值,避免 UI 与服务端分叉。
import { ref } from 'vue';
import { storeToRefs } from 'pinia';
import { useAppStore } from '../../store';

withDefaults(defineProps<{
  /** 是否显示(embedded 模式按 activeSection 切换;standalone 恒 true) */
  show?: boolean;
}>(), {
  show: true,
});

const store = useAppStore();
const { taskCodingBundleEnabled } = storeToRefs(store);

/** 保存反馈(成功/失败,3 秒后自动清除) */
const msg = ref('');
const msgKind = ref<'ok' | 'err'>('ok');

/**
 * 切换开关并立即持久化。
 * 取值直接读事件目标的 checked(而非依赖 v-model 的更新时机),再把该值写入草稿:
 * 保存成功由服务端响应回填,失败则回滚到切换前的值。
 */
async function onToggle(next: boolean): Promise<void> {
  const prev = taskCodingBundleEnabled.value;
  if (prev === next) return;
  taskCodingBundleEnabled.value = next;
  msg.value = '';
  try {
    await store.queueSettingsSave({ task_coding_bundle_enabled: next });
    // 开包会在服务端**并入两条编码流程**(CODE-5)。流程库缓存在 store 里,而各消费点
    // (`TaskFlowSelect` / `TaskBoard` / 执行流程区)都只在「未加载」时拉一次——不刷新的话,
    // 「绑定流程」下拉与流程列表要重开设置或重启才看得到新流程(实测缺陷,收尾复核时发现)。
    // 关包不回收已注入副本,故这里不需要「反向移除」;刷新失败只记日志(loadAgentFlow 内部吞错)。
    await store.loadAgentFlow();
    msgKind.value = 'ok';
    msg.value = next ? '已开启编码能力包' : '已关闭编码能力包';
  } catch (e) {
    taskCodingBundleEnabled.value = prev;
    msgKind.value = 'err';
    msg.value = `保存失败:${(e as Error).message}`;
  } finally {
    setTimeout(() => (msg.value = ''), 3000);
  }
}
</script>

<template>
  <div v-show="show" class="sv-field">
    <div class="sv-field-label"><span class="sv-supreme blue" /> 编码能力包</div>
    <div class="sv-datalist">
      <div class="sv-data-row">
        <div class="info">
          <b>启用编码能力包</b>
          <span>任务模式执行者的默认系统提示词改用「编码执行者模板」(先读后写 / 遵循既有风格 / 改完跑验证 / 最小改动)</span>
        </div>
        <label
          style="display: flex; gap: var(--space-1-5); align-items: center; cursor: pointer"
          title="开启后任务模式执行者未自定义系统提示词时按编码执行者模板执行;已自定义过则自定义值优先"
        >
          <input
            type="checkbox"
            :checked="taskCodingBundleEnabled"
            style="flex-shrink: 0"
            @change="onToggle(($event.target as HTMLInputElement).checked)"
          />
          <span class="sv-note">{{ taskCodingBundleEnabled ? '已开启' : '已关闭' }}</span>
        </label>
      </div>
    </div>
    <div v-if="msg" class="sv-feedback" :class="msgKind === 'err' ? 'err' : 'ok'">{{ msg }}</div>
    <p class="sv-note">
      仅任务模式生效:开启后,任务模式执行者<b>未自定义</b>系统提示词时,默认值改用「编码执行者模板」,
      强调<b>先读后写</b>、遵循既有风格、改完跑验证、最小改动;你在提示词框里填过自定义内容时,
      <b>以自定义值为准</b>,本开关只影响默认值。<b>不影响角色扮演模式</b>。默认关闭。
    </p>
  </div>
</template>
