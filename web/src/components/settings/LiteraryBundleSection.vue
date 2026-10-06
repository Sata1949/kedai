<script setup lang="ts">
// 设置区:文学能力包(默认关;须用户显式开启)。LIT-1 起为双开关骨架,LIT-6/LIT-7 起
// 再挂两个选择型控件:
//   ① 角色扮演侧开关——纯扁平字段(该侧无覆盖层),经任一模式写入都直写扁平;
//   ② 任务侧开关——扁平 + task 覆盖层,对称「编码能力包」;
//   ③ 文风预设(LIT-6)——扁平字段 `literary_style_preset`,开关开且选了档时在位置 0
//      追加一段文风素材段;四档清单与服务端常量一一对应(键是存储值,勿改名);
//   ④ 长程一致性推荐档(LIT-7)——**显式选档才写入**压缩三项数值,可回到「不改变」;
//      该档只写通用字段,故不随开关门控(门控会让「回退」入口不可达)。
//
// 语义(整批交付后的生效面):角色扮演侧开启后,提示词框留空时默认值取「文学增强版」,
// 并在 system 尾与最新用户消息尾追加创作纪律段;任务侧开启后,任务执行者缺省默认词改用
// 文学向变体。开关都**只影响默认值**,用户在提示词框自定义过则自定义值逐字优先。
//
// 形态与体例:照 CodingBundleSection(同一保存通道 queueSettingsSave 串行队列,
// 失败回滚本地值,避免 UI 与服务端分叉);本区有四个控件,故各自独立回滚。
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
const {
  literaryBundleEnabled,
  taskLiteraryBundleEnabled,
  literaryStylePreset,
  literaryRecommendPreset,
} = storeToRefs(store);

/** 文风预设清单(键与服务端 `LITERARY_STYLE_PRESETS` 逐字对齐;值 = settings.json 存储值) */
const STYLE_OPTIONS = [
  { value: '', label: '不注入(默认)' },
  { value: 'plain', label: '白描冷峻' },
  { value: 'classical', label: '古典雅致' },
  { value: 'lightnovel', label: '轻小说' },
  { value: 'hardboiled', label: '悬疑冷硬' },
] as const;

/** 长程一致性推荐档清单(键与服务端 `LITERARY_RECOMMEND_PRESETS` 逐字对齐) */
const RECOMMEND_OPTIONS = [
  { value: '', label: '不改变(默认)' },
  { value: 'medium', label: '中篇 · 阈值 75% / 保留 6 条' },
  { value: 'long', label: '长篇 · 阈值 70% / 保留 8 条' },
] as const;

/** 保存反馈(成功/失败,3 秒后自动清除) */
const msg = ref('');
const msgKind = ref<'ok' | 'err'>('ok');

/**
 * 统一收尾:保存 → 成功提示(并刷新流程库缓存)/ 失败回滚本地值。
 * `rollback` 由调用方给出(各控件回滚各自的值,互不波及)。
 */
async function persistPatch(
  patch: Record<string, string | boolean>,
  rollback: () => void,
  okText: string,
): Promise<void> {
  msg.value = '';
  try {
    await store.queueSettingsSave(patch);
    // 开包会在服务端**并入四条文学流程**(LIT-5)。流程库缓存在 store 里,而各消费点
    // (「绑定流程」下拉 / 流程列表 / 执行流程区)都只在「未加载」时拉一次——不刷新的话,
    // 要重开设置或重启才看得到新流程(与 CODE-5 收尾复核时实测的同类缺陷一致)。
    // 关包不回收已注入副本,故不需要「反向移除」;刷新失败只记日志(loadAgentFlow 内部吞错)。
    await store.loadAgentFlow();
    msgKind.value = 'ok';
    msg.value = okText;
  } catch (e) {
    rollback();
    msgKind.value = 'err';
    msg.value = `保存失败:${(e as Error).message}`;
  } finally {
    setTimeout(() => (msg.value = ''), 3000);
  }
}

/** 角色扮演侧开关:只影响角色扮演模式,不影响任务模式 */
async function onToggleRoleplay(next: boolean): Promise<void> {
  const prev = literaryBundleEnabled.value;
  if (prev === next) return;
  literaryBundleEnabled.value = next;
  await persistPatch(
    { literary_bundle_enabled: next },
    () => (literaryBundleEnabled.value = prev),
    next ? '已开启文学能力包(角色扮演)' : '已关闭文学能力包(角色扮演)',
  );
}

/** 任务侧开关:只影响任务模式执行者/汇总者的缺省提示词 */
async function onToggleTask(next: boolean): Promise<void> {
  const prev = taskLiteraryBundleEnabled.value;
  if (prev === next) return;
  taskLiteraryBundleEnabled.value = next;
  await persistPatch(
    { task_literary_bundle_enabled: next },
    () => (taskLiteraryBundleEnabled.value = prev),
    next ? '已开启文学能力包(任务模式)' : '已关闭文学能力包(任务模式)',
  );
}

/** 文风预设:空 = 不注入;改了会立即保存 */
async function onStyleChange(next: string): Promise<void> {
  const prev = literaryStylePreset.value;
  if (prev === next) return;
  literaryStylePreset.value = next;
  const label = STYLE_OPTIONS.find((o) => o.value === next)?.label ?? next;
  await persistPatch(
    { literary_style_preset: next },
    () => (literaryStylePreset.value = prev),
    next ? `文风预设已设为「${label}」` : '文风预设已关闭(不注入)',
  );
}

/** 推荐档:显式选档才写入压缩三项;选回空串由服务端按写入前快照恢复 */
async function onRecommendChange(next: string): Promise<void> {
  const prev = literaryRecommendPreset.value;
  if (prev === next) return;
  literaryRecommendPreset.value = next;
  const label = RECOMMEND_OPTIONS.find((o) => o.value === next)?.label ?? next;
  await persistPatch(
    { literary_recommend_preset: next },
    () => (literaryRecommendPreset.value = prev),
    next
      ? `已采纳「${label}」(压缩三项已更新,可随时回到「不改变」恢复原值)`
      : '已回到「不改变」(压缩三项按写入前原值恢复)',
  );
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
      <div class="sv-data-row">
        <div class="info">
          <b>文风预设(角色扮演)</b>
          <span>在最新用户消息尾部追加一段文风素材(人称 / 句式 / 正例锚定);需开启上方「文学能力包」开关才会注入,默认不注入</span>
        </div>
        <select
          class="sv-select"
          title="选择文风取向;空 = 不注入。需开启文学能力包(角色扮演)才生效"
          :value="literaryStylePreset"
          @change="onStyleChange(($event.target as HTMLSelectElement).value)"
        >
          <option v-for="o in STYLE_OPTIONS" :key="o.value" :value="o.value">{{ o.label }}</option>
        </select>
      </div>
      <div class="sv-data-row">
        <div class="info">
          <b>长程一致性推荐档</b>
          <span>把「压缩模式 / 压缩阈值 / 压缩保留条数」三项改为该档推荐值(只改这三项);选回「不改变」按<b>写入前原值</b>恢复,不是恢复默认值</span>
        </div>
        <select
          class="sv-select"
          title="仅在显式选档时写入压缩三项;选回「不改变」即恢复写入前原值"
          :value="literaryRecommendPreset"
          @change="onRecommendChange(($event.target as HTMLSelectElement).value)"
        >
          <option v-for="o in RECOMMEND_OPTIONS" :key="o.value" :value="o.value">{{ o.label }}</option>
        </select>
      </div>
    </div>
    <div v-if="msg" class="sv-feedback" :class="msgKind === 'err' ? 'err' : 'ok'">{{ msg }}</div>
    <p class="sv-note">
      两个开关各自按模式生效:角色扮演侧开关<b>只影响角色扮演模式</b>,任务侧开关<b>只影响任务模式</b>
      (与「编码能力包」同开时,任务侧以编码模板优先)。两者都<b>只影响默认值</b>——
      你在提示词框里填过自定义内容时,<b>以自定义值为准</b>;角色扮演侧的创作纪律段对自定义提示词同样生效。
      开启后还会并入四条文学流程预设(章节续写 / 润色去 AI 腔 / 一致性校对 / 人物声音校准)。
      默认关闭。
    </p>
  </div>
</template>
