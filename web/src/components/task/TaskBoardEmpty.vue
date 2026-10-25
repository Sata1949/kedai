<script setup lang="ts">
// 任务台空态（C-2A，2026-10-25；原 TaskBoard 1260-1308）。
// 两栏「工作台开场」：左侧说明与模式清单，右侧执行设置摘要；按钮把焦点送回左栏目标输入框。
import { computed, nextTick } from 'vue';
import { storeToRefs } from 'pinia';
import { useAppStore } from '../../store';
import { MODE_LABELS, MODE_ORDER } from '../../api/labels';

const store = useAppStore();
const { agentFlowLibrary } = storeToRefs(store);

/** 空态右侧「执行设置」摘要:模式取当前选择;流程仅在 custom 下有意义。 */
const emptyModeLabel = computed(() => MODE_LABELS[store.taskRunMode]);
const emptyFlowLabel = computed(() => {
  if (store.taskRunMode !== 'custom') return '按模式编排';
  const id = store.taskFlowId;
  if (!id) return '跟随当前流程';
  const flow = agentFlowLibrary.value?.flows.find((f) => f.id === id);
  return flow?.name?.trim() || id;
});

function openTaskComposer(): void {
  store.openSidebar();
  void nextTick(() => {
    const input = document.querySelector<HTMLTextAreaElement>('.sv-task-new textarea');
    input?.focus();
    input?.scrollIntoView({ block: 'center', behavior: 'smooth' });
  });
}
</script>

<template>
  <div class="sv-task-empty fill">
    <div class="sv-task-start">
      <div class="sv-task-start-grid">
        <div class="sv-task-start-copy">
          <div class="sv-task-start-kicker">
            <span class="sq" />任务工作台<span class="sv-task-start-kicker-en">WORKBENCH</span>
          </div>
          <h2 class="sv-task-start-title">把目标交给 Kedai</h2>
          <p class="sv-task-start-desc">
            在左侧输入目标，系统会拆解计划、派子智能体执行并汇总结果。
          </p>
          <ol class="sv-task-start-steps">
            <li><b>01</b><span><strong>下达目标</strong><small>写清楚要交付什么</small></span></li>
            <li><b>02</b><span><strong>拆解与执行</strong><small>选择模式、执行者与工作区</small></span></li>
            <li><b>03</b><span><strong>汇总与交付</strong><small>查看步骤、结果与文件变更</small></span></li>
          </ol>
          <div class="sv-task-start-modes" aria-label="可选执行模式">
            <span
              v-for="m in MODE_ORDER"
              :key="m"
              class="sv-task-start-mode"
              :class="{ active: store.taskRunMode === m }"
            >{{ MODE_LABELS[m] }}</span>
          </div>
          <div class="sv-task-start-actions">
            <button type="button" class="sv-btn primary sv-btn-sm" @click="openTaskComposer">
              打开任务面板
            </button>
          </div>
        </div>

        <aside class="sv-task-start-spec" aria-labelledby="sv-task-spec-head">
          <div id="sv-task-spec-head" class="sv-task-start-spec-head">
            <span>执行设置</span><span>运行前可改</span>
          </div>
          <dl class="sv-task-start-spec-list">
            <div><dt>模式</dt><dd>{{ emptyModeLabel }}</dd></div>
            <div><dt>流程</dt><dd>{{ emptyFlowLabel }}</dd></div>
            <div><dt>执行者</dt><dd>左侧选择 · 通用执行者</dd></div>
            <div><dt>工作区</dt><dd>左侧选择 · 可选</dd></div>
          </dl>
          <div class="sv-task-start-spec-foot">
            <span class="sv-task-start-spec-arrow" aria-hidden="true">←</span>
            在左侧「下达目标」写下交付物，点「创建并执行」。
          </div>
        </aside>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* ===== 空态:两栏「工作台开场」 ===== */
.sv-task-empty.fill {
  height: 100%;
  align-items: stretch;
  justify-content: flex-start;
  padding: 0;
  text-align: left;
  color: var(--sv-ink);
}
.sv-task-start {
  position: relative;
  width: 100%;
  max-width: 1060px;
  margin: clamp(var(--space-6), 6vh, calc(var(--space-6) * 3)) auto 0;
  padding: 0 0 var(--space-5);
  border-top: var(--bw) solid var(--sv-ink);
  background: transparent;
  box-shadow: none;
  text-align: left;
}
.sv-task-start::before {
  content: '';
  position: absolute;
  right: 6px;
  top: 26px;
  width: 18px;
  height: 18px;
  background: var(--sv-ink-supreme);
}
.sv-task-start-grid {
  display: grid;
  grid-template-columns: minmax(0, 1.15fr) minmax(290px, 0.85fr);
  gap: clamp(var(--space-8), 5vw, calc(var(--space-8) * 2));
  align-items: start;
}
.sv-task-start-copy { min-width: 0; }
.sv-task-start-kicker {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  margin-top: var(--space-3);
  font-size: var(--text-2xs);
  letter-spacing: 0.18em;
  color: var(--sv-ink-dim);
  text-transform: uppercase;
}
.sv-task-start-kicker .sq { width: 10px; height: 10px; background: var(--sv-red); display: inline-block; flex: none; }
.sv-task-start-kicker-en { margin-left: var(--space-1); color: var(--sv-ink-faint); letter-spacing: 0.2em; }
.sv-task-start-title {
  max-width: 16ch;
  margin: var(--space-4) 0 var(--space-3);
  font-family: var(--font-display);
  font-size: clamp(28px, 3.4vw, 38px);
  line-height: 1.12;
  letter-spacing: -0.01em;
}
.sv-task-start-desc {
  max-width: 52ch;
  margin: 0;
  font-size: var(--text-base);
  line-height: var(--lh-prose);
  color: var(--sv-ink-dim);
}
.sv-task-start-steps {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  margin: var(--space-5) 0 0;
  padding: 0;
  list-style: none;
  border-top: 1px solid var(--sv-line-strong);
  border-bottom: 1px solid var(--sv-line-strong);
}
.sv-task-start-steps li {
  display: flex;
  gap: var(--space-2);
  padding: var(--space-3) var(--space-3) var(--space-3) 0;
  border-right: 1px solid var(--sv-line);
}
.sv-task-start-steps li:last-child { border-right: 0; }
.sv-task-start-steps b {
  flex: none;
  font-family: var(--font-display);
  font-size: var(--text-lg);
  line-height: var(--lh-tight);
  color: var(--sv-red);
}
.sv-task-start-steps span { display: flex; flex-direction: column; gap: var(--space-1); min-width: 0; }
.sv-task-start-steps strong { font-size: var(--text-xs); }
.sv-task-start-steps small { font-size: var(--text-xs); line-height: var(--lh-body); color: var(--sv-ink-faint); }
.sv-task-start-modes {
  display: flex;
  flex-wrap: wrap;
  gap: var(--space-1-5);
  margin-top: var(--space-4);
}
.sv-task-start-mode {
  padding: var(--space-1) var(--space-2);
  border: 1px solid var(--sv-line-strong);
  font-size: var(--text-2xs);
  color: var(--sv-ink-dim);
  letter-spacing: 0.04em;
}
.sv-task-start-mode.active {
  border-color: var(--sv-ink);
  background: var(--sv-ink);
  color: var(--sv-white);
}
.sv-task-start-actions {
  display: flex;
  flex-wrap: wrap;
  gap: var(--space-2);
  margin-top: var(--space-4);
  padding-top: var(--space-2-5);
  border-top: 1px solid var(--sv-line);
}

/* 右栏:执行设置清单(发丝表) */
.sv-task-start-spec {
  min-width: 0;
  border-top: var(--bw) solid var(--sv-ink);
  padding-top: var(--space-2-5);
}
.sv-task-start-spec-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-3);
  padding-bottom: var(--space-2-5);
  border-bottom: 1px solid var(--sv-line-strong);
  font-size: var(--text-2xs);
  font-weight: 800;
  letter-spacing: 0.14em;
  color: var(--sv-ink-soft);
  text-transform: uppercase;
}
.sv-task-start-spec-head span:last-child { color: var(--sv-ink-faint); letter-spacing: 0.04em; }
.sv-task-start-spec-list { margin: 0; }
.sv-task-start-spec-list > div {
  display: grid;
  grid-template-columns: 70px minmax(0, 1fr);
  gap: var(--space-3);
  padding: var(--space-2-5) 0;
  border-bottom: 1px solid var(--sv-line);
}
.sv-task-start-spec-list dt {
  font-size: var(--text-2xs);
  letter-spacing: 0.08em;
  color: var(--sv-ink-faint);
}
.sv-task-start-spec-list dd {
  margin: 0;
  font-size: var(--text-sm);
  color: var(--sv-ink);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.sv-task-start-spec-foot {
  display: flex;
  align-items: flex-start;
  gap: var(--space-2);
  margin-top: var(--space-3);
  font-size: var(--text-xs);
  line-height: var(--lh-body);
  color: var(--sv-ink-dim);
}
.sv-task-start-spec-arrow {
  flex: none;
  font-family: var(--font-display);
  font-size: var(--text-base);
  color: var(--sv-red);
}

@media (max-width: 1099px) {
  .sv-task-start-grid { grid-template-columns: minmax(0, 1fr); gap: var(--space-6); }
  .sv-task-start-spec { max-width: 560px; }
}
@media (max-width: 900px) {
  .sv-task-start::before { display: none; }
}
@media (max-width: 767px) {
  .sv-task-start { margin: var(--space-3) 0 0; padding-bottom: var(--space-4); }
  .sv-task-start-title { font-size: var(--text-xl); }
  .sv-task-start-steps { grid-template-columns: 1fr; }
  .sv-task-start-steps li {
    border-right: 0;
    border-bottom: 1px solid var(--sv-line);
    padding: var(--space-2) 0;
  }
  .sv-task-start-steps li:last-child { border-bottom: 0; }
  .sv-task-start-spec-list > div { padding: var(--space-2) 0; }
}
</style>
