<script setup lang="ts">
// 退出确认弹窗(2026-09-16 批次 5 自绘;2026-09-17 视觉重做 + 二次退出提示)。
//
// 为什么自绘:Tauri 原生对话框是 Windows MessageBox,外观不受前端 CSS 影响,做不出
// 应用统一的至上主义/构成主义语言。现在由壳 prevent_close + 下发 kedai://close-requested
// 打开本弹窗,用户确认才发 kedai://exit-app 真正退出。
//
// 三个入口共用本弹窗(2026-09-17 补齐前两个):
//   - 侧栏底部「退出 Kedai」按钮(新增:此前界面上没有退出入口);
//   - 设置 → 界面 → 退出应用(新增);
//   - 桌面:点窗口关闭 → 壳下发事件;
//   - Android:返回键无处可退时由 App.vue 打开。
// 取消时额外发 kedai://close-cancelled:桌面壳据此复位「二次关闭兜底」标记,
// 否则取消一次之后,下一次单击关闭会被兜底逻辑直接放行(不再询问)。
//
// 二次确认语义(两平台一致,2026-09-17 对齐):
//   - 桌面:App 的 ExitGuard —— 已弹过确认时再点关闭 = 直接退;
//   - Android:返回键已打开本弹窗时再按一次 = 直接退(App.vue::handleAndroidBack 第 0 步)。
//   故文案里明确写出「再按一次返回键即退出」,否则用户不知道有这条捷径。
//
// 样式纪律:只复用 style.css 既有 .sv-modal/.sv-btn/.sv-note 等类与 :root token,
// 不往全局样式表新增类(MAINTENANCE D-5);本组件专有的排版写在 <style scoped> 内。
import { computed } from 'vue';
import { useAppStore } from '../store';
import { isAndroidTauri } from '../platform';

const store = useAppStore();

/** 是否展示「再按一次返回键即退出」提示(仅 Android 有返回键语义) */
const showBackHint = computed(() => isAndroidTauri);

/** 取消退出:关弹窗并通知壳复位兜底标记(失败仅记日志,不阻断关闭弹窗) */
async function cancel(): Promise<void> {
  store.exitConfirmOpen = false;
  try {
    const { emit } = await import('@tauri-apps/api/event');
    await emit('kedai://close-cancelled');
  } catch (e) {
    console.debug('[kedai] 取消退出通知未送达(非 Tauri 环境可忽略)', e);
  }
}

/** 确认退出:壳收到后清理端口残留并结束进程(本弹窗随进程一同消失,无需自关) */
async function confirm(): Promise<void> {
  try {
    const { emit } = await import('@tauri-apps/api/event');
    await emit('kedai://exit-app');
  } catch (e) {
    // 事件送不出去时明确告知用户,而不是留一个点了没反应的按钮
    console.warn('[kedai] 退出请求发送失败', e);
    window.alert('退出请求发送失败,请手动关闭窗口');
  }
}
</script>

<template>
  <div class="sv-modal-mask" @click.self="cancel">
    <div class="sv-modal exit-modal">
      <div class="sv-modal-head">
        <h2 class="sv-head-title">
          <span class="sv-supreme red" />
          退出 Kedai
        </h2>
        <button class="sv-btn ghost sv-btn-square" title="取消" @click="cancel">✕</button>
      </div>

      <div class="sv-modal-body">
        <!-- 提问做主标题:比普通说明文字更重,用户一眼看到要决定什么 -->
        <p class="exit-question">确定要退出 Kedai 吗?</p>
        <p class="exit-sub">退出会结束整个应用进程,以下内容会受影响:</p>

        <ul class="exit-warn">
          <li>
            <span class="exit-warn-mark" aria-hidden="true" />
            <span class="exit-warn-text">
              <b>正在生成或执行中的任务会被中断</b>
              <em>已产出的内容保留在任务记录里,可稍后继续追加指令</em>
            </span>
          </li>
          <li>
            <span class="exit-warn-mark" aria-hidden="true" />
            <span class="exit-warn-text">
              <b>未保存的编辑内容将丢失</b>
              <em>输入框草稿、弹窗里未提交的改动均不会自动保存</em>
            </span>
          </li>
        </ul>

        <!-- Android 的二次返回键快捷退出(与桌面二次关闭同语义) -->
        <p v-if="showBackHint" class="exit-tip">
          提示:按返回键打开本确认后,<b>再按一次返回键即直接退出</b>。
        </p>

        <!-- 构成主义签名:红斜线 + 黑方块,与全站装饰语言一致(纯装饰,不参与交互) -->
        <div class="exit-mark" aria-hidden="true">
          <span class="sv-diagonal lg" />
          <span class="sv-supreme" />
        </div>
      </div>

      <div class="sv-modal-foot exit-foot">
        <button class="sv-btn primary exit-cancel" @click="cancel">取消</button>
        <button class="sv-btn danger exit-confirm" @click="confirm">退出 Kedai</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* 卡面比默认 sm(480px)略宽:后果清单是两行「标题 + 说明」的结构,窄了会挤成多行 */
.exit-modal {
  max-width: 520px;
}

.sv-head-title {
  display: flex;
  align-items: center;
  gap: 8px;
}

/* 主体提问:最大字号,作为视觉主标题(用户第一眼要读到的就是它) */
.exit-question {
  margin: 0;
  color: var(--sv-ink);
  font-size: 17px;
  font-weight: 900;
  letter-spacing: 0.02em;
}

.exit-sub {
  margin: 8px 0 0;
  font-size: 12px;
  color: var(--sv-ink-soft);
  line-height: 1.6;
}

/* 后果清单:每项一个黑方块标记 + 「标题 + 说明」两行,层次比纯列表清楚 */
.exit-warn {
  margin: 14px 0 0;
  padding: 0;
  list-style: none;
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.exit-warn li {
  display: flex;
  align-items: flex-start;
  gap: 10px;
  font-size: 12px;
  line-height: 1.6;
}

.exit-warn-mark {
  flex: none;
  width: 8px;
  height: 8px;
  margin-top: 0.45em;
  background: var(--sv-red);
}

.exit-warn-text {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

.exit-warn-text b {
  color: var(--sv-ink);
  font-size: 13px;
}

.exit-warn-text em {
  font-style: normal;
  color: var(--sv-ink-faint);
}

/* 二次返回键提示:描边小条,与后果清单区分层级(这是操作提示,不是后果) */
.exit-tip {
  margin: 14px 0 0;
  padding: 8px 10px;
  border: var(--bw-hair) dashed var(--sv-line-strong);
  font-size: 12px;
  line-height: 1.6;
  color: var(--sv-ink-soft);
}
.exit-tip b { color: var(--sv-ink); }

/* 底部装饰:斜线 + 方块右对齐收尾,呼应至上主义签名 */
.exit-mark {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 10px;
  margin-top: 18px;
}

.exit-mark .sv-supreme {
  width: 8px;
  height: 8px;
}

/* 按钮区:退出(危险)给更大的点击面积并占主位,取消退居次位——
   但取消仍是默认焦点(误触退出代价高,不鼓励盲按回车) */
.exit-foot {
  gap: 10px;
}
.exit-cancel {
  flex: 0 0 auto;
  min-width: 96px;
}
.exit-confirm {
  flex: 1;
  font-weight: 700;
  letter-spacing: 0.04em;
}
</style>
