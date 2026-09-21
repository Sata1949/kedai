<script setup lang="ts">
// 单步编辑表单(二维批次 3 前端)。
//
// 从 AgentFlowSection.vue 的内联编辑区抽出:列表视图的展开行与画布视图的节点 Inspector
// 共用同一份表单,避免两处各写一遍后漂移(SettingsModal 的 embedded/standalone 双模板
// 漂移就是本仓库的历史教训)。就地对 `step` 写入,草稿的所有权仍归调用方。
//
// 上游候选/成果节点/警示都走 utils/agentFlowGraph 的纯函数,与后端图原语同口径;
// 权威校验仍在后端(保存被拒时由调用方展示中文错误)。
import {
  isLinearCompat,
  outputStepName,
  stepInputs,
  stepKind,
  stepKindWarnings,
  setStepKind,
  toggleStepInput,
  toggleStepOutput,
  upstreamCandidates,
  upstreamWarnings,
  type StepKind,
} from '../../utils/agentFlowGraph';
import {
  setStepToolMode,
  setStepToolsText,
  stepToolMode,
  stepToolsText,
  stepToolsWarnings,
  type StepToolMode,
} from '../../utils/agentFlowTools';
import { TOOL_MODE_LABELS } from '../../composables/useAgentFlow';
import type { AgentFlowStep } from '../../api/types';

const props = defineProps<{
  /** 被编辑的步骤(就地修改) */
  step: AgentFlowStep;
  /** 所属流程的全部步骤(上游候选、成果节点选拔、成环判断都要看全量) */
  steps: AgentFlowStep[];
}>();

// 就地对草稿里的步骤写入:草稿所有权在调用方(`useAgentFlow` 持有 flowDraft,保存时才提交),
// 表单只是它的编辑入口。这里取步骤对象的引用再用,与 ConnectionSection(`props.state` 解构)
// 同一写法——避免 vue/no-mutating-props 把「表单写自己的字段」误判为改 prop。
const { step, steps } = props;
</script>

<template>
  <div class="flow-edit">
    <div class="sv-inp-row">
      <label class="sv-inp-tag">目标</label>
      <input v-model="step.goal" type="text" class="sv-input" placeholder="该步骤做什么(进度提示与计划摘要显示)" spellcheck="false" />
    </div>
    <!-- 二维依赖(二维批次 1/2):上游勾选 + 成果标注 -->
    <div class="sv-inp-row">
      <label class="sv-inp-tag">上游步骤</label>
      <div class="flow-upstream-list">
        <label
          v-for="cand in upstreamCandidates(steps, step.id)"
          :key="cand.id"
          class="flow-upstream-item"
          :class="{ off: !cand.enabled }"
        >
          <input
            type="checkbox"
            :checked="stepInputs(step).includes(cand.id)"
            @change="toggleStepInput(step, cand.id)"
          />
          <span>{{ cand.name || cand.id }}{{ cand.enabled ? '' : '(已停用)' }}</span>
        </label>
        <span v-if="!upstreamCandidates(steps, step.id).length" class="sv-note">
          暂无可用上游(先新增其它步骤)
        </span>
      </div>
    </div>
    <p class="sv-note">
      {{ isLinearCompat(steps)
        ? '当前流程为一维线性:所有步骤都不设上游,按列表顺序逐步串联(与旧版行为一致)。'
        : '当前流程为二维:不勾选上游 = 源节点(只给任务目标);勾选多个 = 多路产出按列表顺序拼接为本步输入。' }}
    </p>
    <div class="sv-inp-row">
      <label class="sv-inp-tag">最终成果</label>
      <button
        class="sv-btn ghost"
        :class="{ 'sv-btn-on': step.is_output === true }"
        :title="step.is_output === true ? '点击取消标注' : '点击标注为最终成果节点'"
        @click="toggleStepOutput(step)"
      >
        {{ step.is_output === true ? '本步产出即成果' : '未标注' }}
      </button>
      <span class="sv-note">
        未标注时按「无后继汇点」自动判定;当前成果节点:{{ outputStepName(steps) ?? '无' }}
      </span>
    </div>
    <p
      v-for="(warn, wi) in upstreamWarnings(steps, step)"
      :key="`g${wi}`"
      class="sv-note flow-tool-warn"
    >
      {{ warn }}
    </p>
    <!-- 节点档位(二维批次 6a):严格 = 单次模型调用、不下发工具;宽松 = 允许多轮工具自循环 -->
    <div class="sv-inp-row">
      <label class="sv-inp-tag">档位</label>
      <select
        class="sv-select flow-select-wide"
        :value="stepKind(step)"
        title="严格 = 单次模型调用、不下发任何工具(适合压缩/抽取这类原子步骤);宽松 = 可与工具多轮循环"
        @change="setStepKind(step, ($event.target as HTMLSelectElement).value as StepKind)"
      >
        <option value="loose">宽松（允许工具自循环）</option>
        <option value="strict">严格（单次调用，不下发工具）</option>
      </select>
      <span class="sv-note">
        {{ stepKind(step) === 'strict'
          ? '严格档:一次模型调用即完成;已配置的工具不会下发(配置保留,切回宽松档即生效)。'
          : '宽松档:模型可反复调用工具,直到给出正文。' }}
      </span>
    </div>
    <p
      v-for="(warn, wi) in stepKindWarnings(step)"
      :key="`k${wi}`"
      class="sv-note flow-tool-warn"
    >
      {{ warn }}
    </p>
    <template v-if="step.action === 'direct'">
      <div class="sv-inp-row">
        <label class="sv-inp-tag">系统提示词</label>
        <textarea
          v-model="step.system_prompt"
          rows="3"
          class="sv-input"
          placeholder="步骤级提示词(支持酒馆宏),以 [本步指令] 追加到系统提示词末尾;留空 = 不追加"
          spellcheck="false"
        />
      </div>
      <!-- 工具区(二维批次 6a):严格档不下发工具,整区隐藏——草稿里的工具配置保留,
           切回宽松档即恢复;已配置工具时的提示由上方 stepKindWarnings 给出 -->
      <template v-if="stepKind(step) === 'loose'">
        <div class="sv-inp-row">
          <label class="sv-inp-tag">工具</label>
          <select
            class="sv-select flow-select-wide"
            :value="stepToolMode(step)"
            @change="setStepToolMode(step, ($event.target as HTMLSelectElement).value as StepToolMode)"
          >
            <option v-for="(label, val) in TOOL_MODE_LABELS" :key="val" :value="val">{{ label }}</option>
          </select>
          <input
            v-if="stepToolMode(step) === 'list'"
            class="sv-input"
            :value="stepToolsText(step)"
            placeholder="工具名,逗号分隔(如 read, search, calculator)"
            spellcheck="false"
            @change="setStepToolsText(step, ($event.target as HTMLInputElement).value)"
          />
        </div>
        <!-- F8(2026-09-10 实跑修复):tools 三态语义易误配——「全部工具」会下发
             全部已注册工具(含编排/写类),分析规划类步骤不应选它。
             文案由 stepToolsWarnings 纯函数产出,便于单测覆盖 -->
        <p
          v-for="(warn, wi) in stepToolsWarnings(step)"
          :key="wi"
          class="sv-note flow-tool-warn"
        >
          {{ warn }}
        </p>
        <p v-if="stepToolMode(step) === 'none'" class="sv-note">
          「不使用工具」= 本步骤纯生成,不下发任何工具。
        </p>
        <div class="sv-inp-row">
          <label class="sv-inp-tag">工具策略</label>
          <select v-model="step.tool_choice" class="sv-select flow-select-wide">
            <option value="auto">auto（模型决定）</option>
            <option value="none">none（禁止调用）</option>
            <option value="required">required（至少调用一个）</option>
            <option value="function">function（指定工具）</option>
          </select>
          <input
            v-if="step.tool_choice === 'function'"
            v-model="step.tool_choice_function"
            class="sv-input"
            placeholder="必须是本步骤有效工具名"
            spellcheck="false"
          />
          <label class="sv-inp-tag">并行调用</label>
          <select v-model="step.parallel_tool_calls" class="sv-select flow-select-wide">
            <option :value="null">后端默认</option>
            <option :value="true">允许</option>
            <option :value="false">禁止</option>
          </select>
        </div>
      </template>
      <div class="sv-inp-row">
        <label class="sv-inp-tag">温度</label>
        <input v-model.number="step.temperature" type="number" min="0" max="2" step="0.1" class="sv-input inject-num" placeholder="沿用全局" />
        <label class="sv-inp-tag">输出上限</label>
        <input v-model.number="step.max_tokens" type="number" min="1" max="131072" class="sv-input inject-num" placeholder="沿用全局" title="该步骤的输出上限(1~131072);留空沿用全局最大生成长度" />
      </div>
    </template>
  </div>
</template>
