<script setup lang="ts">
// 新手教程(首启引导,0.4.0;文案与跳转目标收口见 TUTSYNC 批,2026-10-10):一个组件两种形态。
//   展开态 = 分步模态(三问 + 讲解步骤);点「带我去设置」后 **最小化为右下角浮条**
//   (浮条不渲染遮罩,综合设置照常可操作),浮条上再点「继续教程」回到展开态。
//
// 状态全部在组件内:关掉教程即回到第一问重来(下次启动再弹),不越过 check-arch 的
// 「组件不得直改非模态 store state」纪律;跨组件的两件事只走 action 与白名单内的模态开关
// (openSettingsAt / openSettingsPanel / settingsOpen / onboardingOpen)。
//
// 文案表在 `utils/tutorialCopy.ts`(与关于页速览**同一份**):原先文案内联在本文件、
// 靠注释与 AboutSection 约定同源,结果两处各自腐烂(2026-10-10 盘点实测 12 条硬错而 54 例测试全绿)。
// 本组件仍是懒加载,文案不膨胀首屏 index chunk(共享模块被 Vite hoist 成异步共享 chunk)。
import { computed, ref } from 'vue';
import { useAppStore } from '../store';
import { isAndroidTauri } from '../platform';
import { STEP_COPY, type StepCopy } from '../utils/tutorialCopy';
import {
  PANEL_STORE_FLAG,
  buildTutorialPlan,
  planBranches,
  readDefaultAppMode,
  writeDefaultAppMode,
  writeOnboarding,
  type AppMode,
  type SettingsNavRequest,
  type TutorialPanelKey,
  type TutorialScope,
  type TutorialStepId,
} from '../onboarding';

const store = useAppStore();

/** 一屏 */
type Screen =
  | { kind: 'pref' }
  | { kind: 'scope' }
  | { kind: 'step'; id: TutorialStepId }
  | { kind: 'done' };

const screenIndex = ref(0);
/** 偏好:进组件时预选已设过的偏好(关于页重开场景),未设置过则为 null */
const preference = ref<AppMode | null>(readDefaultAppMode(localStorage));
const scope = ref<TutorialScope | null>(null);
/** 浮条形态(点「带我去设置」后置位;**不渲染遮罩**) */
const minimized = ref(false);
/** 设置弹窗是否由本教程打开(完成/跳过时只关自己开过的那个) */
const openedSettingsByTour = ref(false);
/**
 * 由本教程打开过的独立面板(panel 型落点)。
 * 结束/跳过时逐个关掉——与 `openedSettingsByTour` 对综合设置的处理同一条原则:
 * **教程开的教程关,用户自己开的不碰**。「都需要」一路走下来可能攒下三块面板,
 * 不关的话结束屏后面还叠着它们(2026-10-10 GUI 实测发现)。
 */
const openedPanelsByTour = ref<TutorialPanelKey[]>([]);

/** 讲解步骤文案(单一来源在 utils/tutorialCopy.ts) */
const copy = STEP_COPY;

/** 本次讲解的步骤序列:第二问作答后冻结(教程期间用户切模式不重算,防步骤跳变) */
const plan = computed<TutorialStepId[]>(() =>
  preference.value && scope.value
    ? buildTutorialPlan({
        preference: preference.value,
        scope: scope.value,
        isAndroid: isAndroidTauri,
      })
    : [],
);

const screens = computed<Screen[]>(() => {
  const head: Screen[] = [{ kind: 'pref' }];
  if (!preference.value) return head;
  const withScope: Screen[] = [...head, { kind: 'scope' }];
  if (!scope.value) return withScope;
  return [
    ...withScope,
    ...plan.value.map((id): Screen => ({ kind: 'step', id })),
    { kind: 'done' },
  ];
});

/** 当前屏(下标越界时兜底回第一屏,避免刚作答瞬间渲染空白) */
const current = computed<Screen>(() => screens.value[screenIndex.value] ?? { kind: 'pref' });

/** 当前步骤的进度(第 N / M 步);三问屏与结束屏为 null */
const progress = computed(() => {
  const s = current.value;
  if (s.kind !== 'step') return null;
  const m = plan.value.length;
  const n = plan.value.indexOf(s.id) + 1;
  return n > 0 ? { n, m } : null;
});

/** 当前步骤的文案 */
const stepCopy = computed<StepCopy | null>(() => {
  const s = current.value;
  return s.kind === 'step' ? copy[s.id] : null;
});

/** 浮条上的位置说明(功能域 · 二级项;无 nav 的步骤显示标题) */
const barInfo = computed(() => {
  const c = stepCopy.value;
  const p = progress.value;
  if (!c || !p) return null;
  return { n: p.n, m: p.m, label: c.navLabel ?? c.title };
});

const isLastStep = computed(() => progress.value !== null && progress.value.n === progress.value.m);

/** 第一问:选中即落盘(即使随后跳过教程,偏好也已生效)并进入第二问 */
function pickPreference(mode: AppMode): void {
  preference.value = mode;
  writeDefaultAppMode(localStorage, mode);
  screenIndex.value = 1;
}

/** 第二问:三种答案都写「已完成」标记(含「不需要」—— 用户已明确表态,不再打扰) */
function pickScope(next: TutorialScope): void {
  const pref = preference.value ?? 'roleplay';
  scope.value = next;
  writeOnboarding(localStorage, { preferredMode: pref, tutorials: planBranches(next, pref) });
  // 第二问之后要么是第一步骤屏,要么(不需要时)直接落在结束屏
  screenIndex.value = 2;
}

/** 返回第一问改偏好:范围选择的后果(分支数量)随之作废,重答一次 */
function backToPreference(): void {
  scope.value = null;
  screenIndex.value = 0;
}

function go(delta: number): void {
  const next = screenIndex.value + delta;
  if (next < 0 || next >= screens.value.length) return;
  screenIndex.value = next;
}

/**
 * 「带我去设置」:记下由教程打开的窗口,最小化为浮条,并定位到目标落点。
 *
 * `panel` 型落点由 `SettingsHub` 执行同名 action 项——它把 Hub 关掉、把独立面板打开
 * (综合导入 / 导出、附件管理、记忆库都是独立面板,不是分区)。打开过的记进
 * `openedPanelsByTour`,结束/跳过时逐个关掉(见那段注释)。
 */
function goSettings(nav: SettingsNavRequest): void {
  openedSettingsByTour.value = true;
  minimized.value = true;
  if (nav.kind === 'panel') {
    if (!openedPanelsByTour.value.includes(nav.panel)) openedPanelsByTour.value.push(nav.panel);
    store.openSettingsPanel(nav.panel);
  } else {
    store.openSettingsAt(nav.section);
  }
}

/** 浮条「继续教程」:回到展开态(设置弹窗保持打开,被教程遮罩盖住;结束后再关) */
function resume(): void {
  minimized.value = false;
}

/**
 * 结束教程(「完成」与「跳过」共用):写标记 + 只关**由教程打开**的窗口。
 * 用户自己打开的综合设置与面板不被顺手关掉。
 */
function finish(): void {
  const pref = preference.value ?? 'roleplay';
  writeOnboarding(localStorage, {
    preferredMode: pref,
    tutorials: scope.value ? planBranches(scope.value, pref) : [],
  });
  if (openedSettingsByTour.value) store.settingsOpen = false;
  for (const panel of openedPanelsByTour.value) store[PANEL_STORE_FLAG[panel]] = false;
  store.onboardingOpen = false;
}
</script>

<template>
  <!-- 浮条形态:无遮罩(综合设置照常可点);只有步骤屏会出现此形态 -->
  <div v-if="minimized && barInfo" class="ob-bar" role="status">
    <span class="sv-supreme red ob-bar-mark" />
    <span class="ob-bar-text">
      新手教程 · 第 {{ barInfo.n }} / {{ barInfo.m }} 步 · {{ barInfo.label }}
    </span>
    <button class="sv-btn primary sv-btn-sm" @click="resume">继续教程</button>
    <button class="sv-btn ghost sv-btn-square" title="跳过教程" @click="finish">✕</button>
  </div>

  <!-- 展开形态:遮罩点击不关闭(防误触丢进度),只认显式按钮 -->
  <div v-else class="sv-modal-mask ob-mask">
    <div class="sv-modal md ob-modal" role="dialog" aria-label="新手教程">
      <div class="sv-modal-head">
        <h2><span class="sv-supreme pink" /> 新手教程</h2>
        <button class="sv-btn ghost sv-btn-square" title="跳过教程" @click="finish">✕</button>
      </div>

      <div class="sv-modal-body ob-body">
        <!-- S1 偏好选择 -->
        <template v-if="current.kind === 'pref'">
          <div class="ob-step-label">偏好选择</div>
          <p class="ob-lead">检测到您是第一次使用 Kedai Agent，我们需要了解您的喜好是什么？</p>
          <div class="ob-opts">
            <button
              class="ob-opt"
              :class="{ on: preference === 'roleplay' }"
              @click="pickPreference('roleplay')"
            >
              <span class="ob-opt-title">1. 角色扮演</span>
              <span class="ob-opt-desc">我喜欢在虚拟世界中扮演角色推进剧情</span>
            </button>
            <button
              class="ob-opt"
              :class="{ on: preference === 'task' }"
              @click="pickPreference('task')"
            >
              <span class="ob-opt-title">2. 任务</span>
              <span class="ob-opt-desc">我喜欢通过 agent 执行交互完成现实中的工作任务</span>
            </button>
          </div>
          <p class="ob-note">
            此次选择将决定每次启动 Kedai 后默认进入的模式；后续可在「综合设置 → 界面与工具 · 界面」中修改。
          </p>
        </template>

        <!-- S2 是否开始教学流程 -->
        <template v-else-if="current.kind === 'scope'">
          <div class="ob-step-label">是否开始教学流程</div>
          <p class="ob-lead">
            您第一次使用 Kedai Agent，是否需要开启新手教程以增加对 Kedai Agent 功能的了解程度？
          </p>
          <div class="ob-opts">
            <button class="ob-opt" @click="pickScope('both')">
              <span class="ob-opt-title">1. 都需要，谢谢</span>
              <span class="ob-opt-desc">教程都执行，按您选择的偏好决定先后顺序</span>
            </button>
            <button class="ob-opt" @click="pickScope('preferred')">
              <span class="ob-opt-title">2. 我只需要看我喜欢的</span>
              <span class="ob-opt-desc">只讲解偏好对应的那一部分</span>
            </button>
            <button class="ob-opt" @click="pickScope('none')">
              <span class="ob-opt-title">3. 不需要，谢谢</span>
              <span class="ob-opt-desc">跳过教程（之后可在「综合设置 → 关于 · 关于与教程」重新打开）</span>
            </button>
          </div>
        </template>

        <!-- 讲解步骤 -->
        <template v-else-if="current.kind === 'step' && stepCopy">
          <div class="ob-step-label">
            新手教程 · 第 {{ progress?.n }} / {{ progress?.m }} 步
            <span v-if="stepCopy.navLabel" class="ob-step-nav">{{ stepCopy.navLabel }}</span>
          </div>
          <h3 class="ob-title">{{ stepCopy.title }}</h3>
          <p v-for="(line, i) in stepCopy.lines" :key="i" class="ob-line">{{ line }}</p>
          <p v-if="stepCopy.tourNote" class="ob-line">{{ stepCopy.tourNote }}</p>
          <button
            v-if="stepCopy.nav"
            class="sv-btn primary ob-goto"
            @click="goSettings(stepCopy.nav)"
          >
            带我去设置 →
          </button>
          <p v-if="stepCopy.nav" class="ob-note">
            点击后教程会缩到右下角，综合设置会打开并定位到{{ stepCopy.nav.kind === 'panel' ? '该项对应的独立面板' : '该分区' }}；看完点浮条上的「继续教程」回到这里。
          </p>
        </template>

        <!-- 结束屏 -->
        <template v-else>
          <div class="ob-step-label">完成</div>
          <h3 class="ob-title">教程已结束</h3>
          <p class="ob-line">
            需要时可以重新打开：「综合设置 → 关于 · 关于与教程」里的「重新打开新手引导」。
          </p>
          <p class="ob-line">
            默认模式可随时改：「综合设置 → 界面与工具 · 界面」→「默认模式」。
          </p>
        </template>
      </div>

      <div class="sv-modal-foot ob-foot">
        <button
          v-if="current.kind === 'scope'"
          class="sv-btn ghost"
          @click="backToPreference"
        >
          ← 返回上一步
        </button>
        <template v-if="current.kind === 'step'">
          <button class="sv-btn ghost" :disabled="progress?.n === 1" @click="go(-1)">上一步</button>
          <button class="sv-btn primary" @click="go(1)">
            {{ isLastStep ? '完成教程' : '下一步' }}
          </button>
        </template>
        <button v-if="current.kind === 'done'" class="sv-btn primary" @click="finish">开始使用</button>
        <span v-if="current.kind === 'pref'" class="ob-foot-hint">选择后自动进入下一步</span>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* 教程遮罩与浮条同层:高于综合设置(--z-modal:50)、低于启动动画(--z-splash:100) */
.ob-mask {
  z-index: var(--z-tour);
}
.ob-modal {
  width: 660px;
}
.ob-body {
  display: flex;
  flex-direction: column;
  gap: var(--space-2-5);
}
.ob-step-label {
  font-size: var(--text-xs);
  letter-spacing: 0.08em;
  color: var(--sv-ink-dim);
  display: flex;
  gap: var(--space-2-5);
  align-items: center;
}
.ob-step-nav {
  color: var(--sv-pink-dark);
  font-weight: 700;
}
.ob-lead {
  margin: 0;
  font-size: var(--text-md);
  line-height: 1.6;
}
.ob-title {
  margin: 0;
  font-family: var(--font-display);
  font-size: var(--text-lg);
  letter-spacing: 0.02em;
}
.ob-line {
  margin: 0;
  font-size: var(--text-base);
  line-height: 1.7;
  color: var(--sv-ink-soft);
}
.ob-note {
  margin: 0;
  font-size: var(--text-xs);
  color: var(--sv-ink-faint);
  line-height: 1.6;
}
/* 选项卡:构成主义硬边(直角 + 印刷黑 3px 边 + 硬投影),选中态用粉底回声 */
.ob-opts {
  display: flex;
  flex-direction: column;
  gap: var(--space-2-5);
}
.ob-opt {
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
  width: 100%;
  padding: var(--space-3) var(--space-3-5);
  text-align: left;
  cursor: pointer;
  background: var(--sv-white);
  border: var(--bw) solid var(--sv-ink);
  border-radius: 0;
  box-shadow: var(--shadow-sm);
  font-family: inherit;
}
.ob-opt:hover {
  background: var(--sv-pink-light);
  box-shadow: var(--shadow-card);
}
.ob-opt.on {
  background: var(--sv-pink-light);
  border-color: var(--sv-pink-dark);
}
.ob-opt-title {
  font-weight: 700;
  font-size: var(--text-md);
  color: var(--sv-ink);
}
.ob-opt-desc {
  font-size: var(--text-sm);
  color: var(--sv-ink-dim);
  line-height: 1.55;
}
.ob-goto {
  align-self: flex-start;
  margin-top: var(--space-1);
}
.ob-foot {
  align-items: center;
  gap: var(--space-2-5);
}
.ob-foot .ob-foot-hint {
  margin-right: auto;
  font-size: var(--text-xs);
  color: var(--sv-ink-faint);
}
/* 浮条:右下角常驻(无遮罩);窄屏上移避开底部导航 */
.ob-bar {
  position: fixed;
  right: 20px;
  bottom: 20px;
  z-index: var(--z-tour);
  display: flex;
  align-items: center;
  gap: var(--space-2-5);
  max-width: min(560px, calc(100vw - 40px));
  padding: var(--space-2-5) var(--space-3);
  background: var(--sv-surface-elevated);
  border: var(--bw-heavy) solid var(--sv-ink);
  box-shadow: var(--shadow-modal);
}
.ob-bar-mark {
  width: 12px;
  height: 12px;
  flex: none;
}
.ob-bar-text {
  /* 窄屏(375px)实测:不加这三条时整条浮条会被文字挤成 3 行、按钮折成「继续教/程」。
     改为文字可截断(ellipsis)、按钮不折行。 */
  flex: 1;
  min-width: 0;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  font-size: var(--text-sm);
  line-height: 1.4;
}
.ob-bar > .sv-btn {
  flex: none;
  white-space: nowrap;
}
@media (max-width: 767px) {
  .ob-bar {
    right: 12px;
    left: 12px;
    bottom: calc(var(--mobile-nav-h) + var(--safe-bottom) + 12px);
    max-width: none;
  }
}
</style>
