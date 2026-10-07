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
import * as cuApi from '../../api/computerUse';

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

// ===== CU-1:电脑操作治理(急停 + 操作审计) =====
// 与 Agent 面板头的急停按钮写同一后端状态(services::computer_use::ComputerUseControl);
// 本区是设置侧的常驻入口 + 审计查看(每次截图尝试恰一条,只记元数据不存画面)。
const cuStopped = ref(false);
const cuBusy = ref(false);
const cuLoading = ref(false);
const cuMsg = ref('');
const cuEntries = ref<cuApi.CuAuditEntry[]>([]);

/** 审计结果 → 展示标签(错误码优先;成功显示「已截图」) */
const CU_RESULT_LABEL: Record<string, string> = {
  '': '已截图',
  CONTROL_STOPPED: '急停拒绝',
  CAPTURE_DISABLED: '未启用',
  CAPTURE_FAILED: '执行失败',
};

/** 读取急停状态 + 最近审计(进设置区时加载;失败就近提示,不清空已有列表) */
async function loadCu(): Promise<void> {
  cuLoading.value = true;
  cuMsg.value = '';
  try {
    cuStopped.value = (await cuApi.getCuStatus()).stopped;
    cuEntries.value = await cuApi.listCuAudit(100);
  } catch (e) {
    cuMsg.value = `读取失败:${(e as Error).message}`;
  } finally {
    cuLoading.value = false;
  }
}

/** 置位/解除急停(与 Agent 面板同一后端状态;成功后刷新审计) */
async function toggleCu(): Promise<void> {
  if (cuBusy.value) return;
  cuBusy.value = true;
  cuMsg.value = '';
  try {
    const r = cuStopped.value ? await cuApi.resumeComputerUse() : await cuApi.stopComputerUse();
    cuStopped.value = r.stopped;
    cuMsg.value = r.stopped ? '已停止电脑操作:截图/读屏类工具在恢复前一律拒绝执行' : '已恢复电脑操作';
  } catch (e) {
    cuMsg.value = `操作失败:${(e as Error).message}`;
  } finally {
    cuBusy.value = false;
  }
}

/** 清空审计(仅删行,不影响急停状态) */
async function clearCu(): Promise<void> {
  cuBusy.value = true;
  cuMsg.value = '';
  try {
    const n = await cuApi.clearCuAudit();
    cuEntries.value = [];
    cuMsg.value = `已清空 ${n} 条记录`;
  } catch (e) {
    cuMsg.value = `清空失败:${(e as Error).message}`;
  } finally {
    cuBusy.value = false;
  }
}

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
  // CU-1:急停状态与操作审计两平台都有(桌面 GDI 截图 / 安卓无障碍截图同一审计面)
  void loadCu();
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

      <!-- CU-1:操作审计与急停(与 Agent 面板头按钮同一后端状态;两平台都有) -->
      <div class="sv-separator" />
      <div class="sv-field-label sub">操作审计与急停</div>
      <div class="sv-inp-row">
        <label class="sv-inp-tag">急停</label>
        <span class="sv-tag" :class="cuStopped ? 'sensitive' : ''">
          {{ cuLoading ? '读取中…' : cuStopped ? '已停止' : '未停止' }}
        </span>
        <button class="sv-btn ghost sv-btn-sm" :disabled="cuBusy" @click="toggleCu">
          {{ cuStopped ? '恢复操作电脑' : '停止操作电脑' }}
        </button>
        <button class="sv-btn ghost sv-btn-sm" :disabled="cuLoading" @click="loadCu">刷新</button>
      </div>
      <p class="sv-note">
        急停后截图/读屏类工具一律拒绝执行(结果码 CONTROL_STOPPED),直到点「恢复操作电脑」;
        Agent 也内置了同名的自停能力,在你表示「别看我屏幕」时它会主动置位。
        每次截图尝试(成功 / 被急停 / 未启用 / 执行失败)<b>都会</b>在此留一条记录:
        <b>只记范围、尺寸与图像哈希,不保存屏幕画面</b>。
      </p>
      <div class="sv-inp-row">
        <label class="sv-inp-tag">最近记录({{ cuEntries.length }})</label>
        <button
          class="sv-btn ghost sv-btn-sm"
          :disabled="cuBusy || !cuEntries.length"
          @click="clearCu"
        >清空</button>
      </div>
      <div v-if="cuEntries.length" class="sv-cu-audit-list">
        <div
          v-for="e in cuEntries"
          :key="e.id"
          class="sv-cu-audit-row"
          :class="{ denied: e.decision !== 'allowed' }"
        >
          <div class="sv-cu-audit-head">
            <span class="sv-badge" :class="e.error_code ? 'destructive' : 'safe'">
              {{ CU_RESULT_LABEL[e.error_code] ?? e.error_code }}
            </span>
            <span class="sv-tag sm">{{ e.platform }}</span>
            <span class="sv-tag sm">{{ e.source === 'task' ? '任务' : '聊天' }}</span>
            <span class="sv-note">{{ e.target }}</span>
            <span class="sv-note">{{ e.ts }}</span>
          </div>
          <code v-if="e.image_ref" class="sv-cu-audit-ref">
            图像 {{ e.image_ref }} · {{ e.png_bytes ?? 0 }} 字节 · sha256 {{ e.sha256.slice(0, 12) }}…
          </code>
        </div>
      </div>
      <p v-else class="sv-note">暂无电脑操作记录</p>
      <p v-if="cuMsg" class="sv-note">{{ cuMsg }}</p>
    </div>
    <div v-if="msg" class="sv-feedback" :class="msgKind === 'err' ? 'err' : 'ok'">{{ msg }}</div>
    <div v-if="screenMsg" class="sv-feedback">{{ screenMsg }}</div>
  </div>
</template>

<style scoped>
/* CU-1 操作审计列表(形态照 AndroidExecSection 的审计行;不引裸间距值) */
.sv-cu-audit-list {
  max-height: 320px;
  overflow: auto;
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
}
.sv-cu-audit-row {
  border: 1px solid var(--sv-line, #d8d8d8);
  padding: var(--space-1-5) var(--space-2);
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
  font-size: 12px;
}
.sv-cu-audit-row.denied {
  border-color: var(--sv-red, #c0392b);
  opacity: 0.85;
}
.sv-cu-audit-head {
  display: flex;
  align-items: center;
  gap: var(--space-1-5);
  flex-wrap: wrap;
}
.sv-cu-audit-ref {
  display: block;
  white-space: pre-wrap;
  word-break: break-all;
  background: rgba(0, 0, 0, 0.04);
  padding: 2px var(--space-1);
}
</style>
