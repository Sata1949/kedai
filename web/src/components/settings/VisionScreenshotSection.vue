<script setup lang="ts">
// 设置区:视觉与截图(视觉能力包;默认关,须用户显式开启)。
//
// 语义:开启后允许截图工具在此设备取屏(Windows 原生 GDI;安卓无障碍截图见移动端批次),
// 配合「连接配置 → 模型能力」的视觉输入能力位,模型可以「看」屏幕做读屏视觉验证
// (截图 → 图像 → 模型)。连接未开「视觉输入」时截图仍可调用,但图像不会随请求下发。
//
// 隐私:截图内容会经所配置的连接发往模型端点——故默认关闭、须显式开启,并在下方明示。
//
// 形态与体例:单开关分区,照 CodingBundleSection(同一保存通道 queueSettingsSave,
// 失败回滚本地值,避免 UI 与服务端分叉)。
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
const { visionScreenshotEnabled } = storeToRefs(store);

/** 保存反馈(成功/失败,3 秒后自动清除) */
const msg = ref('');
const msgKind = ref<'ok' | 'err'>('ok');

async function onToggle(next: boolean): Promise<void> {
  const prev = visionScreenshotEnabled.value;
  if (prev === next) return;
  visionScreenshotEnabled.value = next;
  msg.value = '';
  try {
    await store.queueSettingsSave({ vision_screenshot_enabled: next });
    msgKind.value = 'ok';
    msg.value = next ? '已开启视觉与截图' : '已关闭视觉与截图';
  } catch (e) {
    visionScreenshotEnabled.value = prev;
    msgKind.value = 'err';
    msg.value = `保存失败:${(e as Error).message}`;
  } finally {
    setTimeout(() => (msg.value = ''), 3000);
  }
}
</script>

<template>
  <div v-show="show" class="sv-field">
    <div class="sv-field-label"><span class="sv-supreme blue" /> 视觉与截图</div>
    <div class="sv-datalist">
      <div class="sv-data-row">
        <div class="info">
          <b>允许截图工具取屏</b>
          <span>截图工具可读取屏幕内容(全屏 / 指定显示器 / 区域 / 窗口);图像交给模型查看</span>
        </div>
        <label
          style="display: flex; gap: 6px; align-items: center; cursor: pointer"
          title="开启后模型可在对话/任务中调用截图工具;截图内容会发往「连接配置」里的模型端点,请确认端点可信"
        >
          <input
            type="checkbox"
            :checked="visionScreenshotEnabled"
            style="flex-shrink: 0"
            @change="onToggle(($event.target as HTMLInputElement).checked)"
          />
          <span class="sv-note">{{ visionScreenshotEnabled ? '已开启' : '已关闭' }}</span>
        </label>
      </div>
      <p class="sv-note">
        开启后,模型可在对话 / 任务中主动调用截图工具读取屏幕(读屏视觉验证:截图 → 图像 → 模型);
        图像要真正随请求发送,还需在「连接配置 → 模型能力」勾选<b>视觉输入</b>。
        <b>隐私提示</b>:截图内容会经所配置的连接发往模型端点,请确认端点可信;
        Windows 下受保护/最小化的窗口可能截为异常画面,该情形会在结果中如实注记。
        默认关闭,须显式开启。
      </p>
    </div>
    <div v-if="msg" class="sv-feedback" :class="msgKind === 'err' ? 'err' : 'ok'">{{ msg }}</div>
  </div>
</template>
