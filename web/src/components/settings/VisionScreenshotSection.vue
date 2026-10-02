<script setup lang="ts">
// 设置区:视觉与截图(视觉能力包;默认关,须用户显式开启)。
//
// 语义:开启后允许截图工具在此设备取屏(Windows 原生 GDI;安卓走无障碍截图服务),
// 配合「连接配置 → 模型能力」的视觉输入能力位,模型可以「看」屏幕做读屏视觉验证
// (截图 → 图像 → 模型)。连接未开「视觉输入」时截图仍可调用,但图像不会随请求下发。
//
// 隐私:截图内容会经所配置的连接发往模型端点——故默认关闭、须显式开启,并在下方明示。
//
// 形态与体例:单开关分区,照 CodingBundleSection(同一保存通道 queueSettingsSave,
// 失败回滚本地值,避免 UI 与服务端分叉)。安卓块(移动端视觉能力包 A4)照
// AndroidExecSection 的 Shizuku 流程形态:状态 tag + 「刷新」强制重探 + 「前往系统设置开启」。
import { onMounted, ref } from 'vue';
import { storeToRefs } from 'pinia';
import { useAppStore } from '../../store';
import { isAndroidTauri } from '../../platform';
import * as screenApi from '../../api/screen';

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

/** 安卓无障碍截图状态(仅 Android Tauri 壳内读取;桌面/浏览器不请求) */
const screenStatus = ref<screenApi.ScreenStatus | null>(null);
const screenLoading = ref(false);
const screenMsg = ref('');

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

/**
 * 读取安卓无障碍截图状态。`refresh=true` 强制重探(清后端缓存)——
 * 用户刚在系统设置里开启/关闭服务后,不重探会一直拿到缓存旧值。
 */
async function loadScreenStatus(refresh = false): Promise<void> {
  screenLoading.value = true;
  screenMsg.value = '';
  try {
    screenStatus.value = await screenApi.getScreenStatus(refresh);
  } catch (e) {
    screenStatus.value = null;
    screenMsg.value = `截图状态读取失败:${(e as Error).message}`;
  } finally {
    screenLoading.value = false;
  }
}

/** 跳系统「无障碍」设置页(开启服务后回应用点「刷新」) */
async function openSystemAccessibility(): Promise<void> {
  screenMsg.value = '';
  try {
    await screenApi.openAccessibilitySettings();
    screenMsg.value = '已跳转系统设置:请找到 Kedai 并开启截图服务,回来后点「刷新」。';
  } catch (e) {
    screenMsg.value = `跳转失败:${(e as Error).message}`;
  }
}

onMounted(() => {
  // 仅 Android 壳内探测;桌面/浏览器无此概念(不发起无意义请求)
  if (isAndroidTauri) void loadScreenStatus();
});
</script>

<template>
  <div v-show="show" class="sv-field">
    <div class="sv-field-label"><span class="sv-supreme blue" /> 视觉与截图</div>
    <div class="sv-datalist">
      <div class="sv-data-row">
        <div class="info">
          <b>允许截图工具取屏</b>
          <span>截图工具可读取屏幕内容(桌面:全屏 / 指定显示器 / 区域 / 窗口;安卓:整屏);图像交给模型查看</span>
        </div>
        <label
          style="display: flex; gap: var(--space-1-5); align-items: center; cursor: pointer"
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

      <template v-if="isAndroidTauri">
        <div class="sv-separator" />
        <div class="sv-field-label sub">安卓无障碍截图(Android)</div>
        <div class="sv-inp-row">
          <label class="sv-inp-tag">截图服务状态</label>
          <span class="sv-tag" :class="screenStatus?.android.accessibility_enabled ? 'sensitive' : ''">
            {{ screenLoading ? '探测中…' : screenStatus ? (screenStatus.android.accessibility_enabled ? '已启用' : '未启用') : '未知' }}
          </span>
          <button class="sv-btn ghost sv-btn-sm" :disabled="screenLoading" @click="loadScreenStatus(true)">
            刷新
          </button>
          <button
            v-if="screenStatus && !screenStatus.android.accessibility_enabled"
            class="sv-btn ghost sv-btn-sm"
            @click="openSystemAccessibility"
          >前往系统设置开启</button>
        </div>
        <p v-if="screenStatus" class="sv-note">{{ screenStatus.reason }}</p>
        <p class="sv-note">
          安卓截图由系统「无障碍」服务提供(Android 11+):仅在应用主动调用截图工具时取屏,
          <b>不读取窗口内容、不执行手势、不注入输入</b>——无障碍仅用于截图。
          图像会经所配置的连接发往模型端点(与上方隐私口径一致);
          受保护内容(FLAG_SECURE,如支付/密码界面)无法被截取。
          Android 13+ 侧载应用开启无障碍时,系统可能要求先在应用信息页允许「受限设置」。
        </p>
      </template>
    </div>
    <div v-if="msg" class="sv-feedback" :class="msgKind === 'err' ? 'err' : 'ok'">{{ msg }}</div>
    <div v-if="screenMsg" class="sv-feedback">{{ screenMsg }}</div>
  </div>
</template>
