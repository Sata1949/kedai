<script setup lang="ts">
// 设置区:文学能力包(默认关;须用户显式开启)。LIT-1 起为双开关骨架:
//   ① 角色扮演侧开关——纯扁平字段(该侧无覆盖层),经任一模式写入都直写扁平;
//   ② 任务侧开关——扁平 + task 覆盖层,对称「编码能力包」。
//
// 语义(整批交付后的生效面):角色扮演侧开启后,提示词框留空时默认值取「文学增强版」,
// 并在 system 尾与最新用户消息尾追加两段文学创作纪律(反 AI 腔 / 视角锚定 / 防复读代答 /
// 篇幅纪律);任务侧开启后,任务执行者缺省默认词改用文学向变体。两开关都**只影响默认值**,
// 用户在提示词框自定义过则自定义值逐字优先。
//
// 形态与体例:照 CodingBundleSection(同一保存通道 queueSettingsSave 串行队列,
// 失败回滚本地值,避免 UI 与服务端分叉);本区有两个开关,故各自独立 onToggle/回滚。
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
const { literaryBundleEnabled, taskLiteraryBundleEnabled } = storeToRefs(store);

/** 保存反馈(成功/失败,3 秒后自动清除) */
const msg = ref('');
const msgKind = ref<'ok' | 'err'>('ok');

/** 统一收尾:成功提示 / 失败回滚(prev 为切换前的本地值) */
async function persist(
  next: boolean,
  prev: boolean,
  rollback: (v: boolean) => void,
  patch: Record<string, boolean>,
  okText: string,
): Promise<void> {
  msg.value = '';
  try {
    await store.queueSettingsSave(patch);
    msgKind.value = 'ok';
    msg.value = okText;
  } catch (e) {
    rollback(prev);
    msgKind.value = 'err';
    msg.value = `保存失败:${(e as Error).message}`;
    void next;
  } finally {
    setTimeout(() => (msg.value = ''), 3000);
  }
}

/** 角色扮演侧开关:只影响角色扮演模式,不影响任务模式 */
async function onToggleRoleplay(next: boolean): Promise<void> {
  const prev = literaryBundleEnabled.value;
  if (prev === next) return;
  literaryBundleEnabled.value = next;
  await persist(next, prev, (v) => (literaryBundleEnabled.value = v), { literary_bundle_enabled: next },
    next ? '已开启文学能力包(角色扮演)' : '已关闭文学能力包(角色扮演)');
}

/** 任务侧开关:只影响任务模式执行者/汇总者的缺省提示词 */
async function onToggleTask(next: boolean): Promise<void> {
  const prev = taskLiteraryBundleEnabled.value;
  if (prev === next) return;
  taskLiteraryBundleEnabled.value = next;
  await persist(next, prev, (v) => (taskLiteraryBundleEnabled.value = v), { task_literary_bundle_enabled: next },
    next ? '已开启文学能力包(任务模式)' : '已关闭文学能力包(任务模式)');
}
</script>

<template>
  <div v-show="show" class="sv-field">
    <div class="sv-field-label"><span class="sv-supreme blue" /> 文学能力包</div>
    <div class="sv-datalist">
      <div class="sv-data-row">
        <div class="info">
          <b>启用文学能力包(角色扮演)</b>
          <span>角色扮演模式:提示词框留空时默认值取「文学增强版」,并在 system 尾与最新用户消息尾追加两段创作纪律(反 AI 腔 / 视角锚定 / 防复读代答 / 篇幅纪律)</span>
        </div>
        <label
          style="display: flex; gap: var(--space-1-5); align-items: center; cursor: pointer"
          title="仅角色扮演模式生效;未自定义提示词时默认值取文学增强版,自定义值优先"
        >
          <input
            type="checkbox"
            :checked="literaryBundleEnabled"
            style="flex-shrink: 0"
            @change="onToggleRoleplay(($event.target as HTMLInputElement).checked)"
          />
          <span class="sv-note">{{ literaryBundleEnabled ? '已开启' : '已关闭' }}</span>
        </label>
      </div>
      <div class="sv-data-row">
        <div class="info">
          <b>任务模式启用文学能力包</b>
          <span>任务模式执行者的默认系统提示词改用「文学向变体」(创作视角 / 事实一致 / 篇幅纪律);仅影响文学写作类任务</span>
        </div>
        <label
          style="display: flex; gap: var(--space-1-5); align-items: center; cursor: pointer"
          title="仅任务模式生效;未自定义提示词时默认值改用文学向变体,自定义值优先"
        >
          <input
            type="checkbox"
            :checked="taskLiteraryBundleEnabled"
            style="flex-shrink: 0"
            @change="onToggleTask(($event.target as HTMLInputElement).checked)"
          />
          <span class="sv-note">{{ taskLiteraryBundleEnabled ? '已开启' : '已关闭' }}</span>
        </label>
      </div>
    </div>
    <div v-if="msg" class="sv-feedback" :class="msgKind === 'err' ? 'err' : 'ok'">{{ msg }}</div>
    <p class="sv-note">
      两个开关各自按模式生效:角色扮演侧开关<b>只影响角色扮演模式</b>,任务侧开关<b>只影响任务模式</b>
      (与「编码能力包」同开时,任务侧以编码模板优先)。两者都<b>只影响默认值</b>——
      你在提示词框里填过自定义内容时,<b>以自定义值为准</b>;角色扮演侧的创作纪律段对自定义提示词同样生效。
      默认关闭。
    </p>
  </div>
</template>
