<script setup lang="ts">
// 单步编辑表单(二维批次 3 前端)。
//
// 从 AgentFlowSection.vue 的内联编辑区抽出:列表视图的展开行与画布视图的节点 Inspector
// 共用同一份表单,避免两处各写一遍后漂移(SettingsModal 的 embedded/standalone 双模板
// 漂移就是本仓库的历史教训)。就地对 `step` 写入,草稿的所有权仍归调用方。
//
// 上游候选/成果节点/警示都走 utils/agentFlowGraph 的纯函数,与后端图原语同口径;
// 权威校验仍在后端(保存被拒时由调用方展示中文错误)。
import { toRef } from 'vue';
import {
  MAX_SUB_FLOW_DEPTH,
  flowName,
  isLinearCompat,
  outputStepName,
  setSubFlowId,
  stepInputs,
  stepKind,
  stepKindWarnings,
  stepSubFlowWarnings,
  setStepKind,
  subFlowCandidates,
  subFlowId,
  subFlowMountNote,
  toggleStepInput,
  toggleStepOutput,
  upstreamCandidates,
  upstreamWarnings,
  type StepKind,
} from '../../utils/agentFlowGraph';
import {
  STEP_MAX_CONTEXT_MAX,
  STEP_MAX_CONTEXT_MIN,
  STEP_OUTPUT_TOKENS_MAX,
  STEP_OUTPUT_TOKENS_MIN,
  setStepToolMode,
  setStepToolsText,
  stepToolMode,
  stepToolsText,
  stepToolsWarnings,
  type StepToolMode,
  type ToolPolicyCtx,
} from '../../utils/agentFlowTools';
import {
  MAX_TOOL_ROUNDS_MAX,
  MAX_TOOL_ROUNDS_MIN,
  setStepConnectionId,
  setStepToolRounds,
  staleConnectionLabel,
  stepConnectionId,
  stepConnectionWarnings,
  stepToolRoundsText,
  stepToolRoundsWarnings,
  type FlowConnectionOption,
} from '../../utils/agentFlowConnections';
import {
  STEP_CALL_TIMEOUT_MAX,
  STEP_CALL_TIMEOUT_MIN,
  STEP_MAX_RETRIES_MAX,
  STEP_MAX_RETRIES_MIN,
  setStepCallTimeout,
  setStepMaxRetries,
  stepCallTimeoutText,
  stepCallTimeoutWarnings,
  stepMaxRetriesText,
  stepMaxRetriesWarnings,
} from '../../utils/agentFlowStepLimits';
import { TOOL_MODE_LABELS } from '../../composables/useAgentFlow';
import type { AgentFlowConfig, AgentFlowStep } from '../../api/types';

const props = defineProps<{
  /** 被编辑的步骤(就地修改) */
  step: AgentFlowStep;
  /** 所属流程的全部步骤(上游候选、成果节点选拔、成环判断都要看全量) */
  steps: AgentFlowStep[];
  /** 流程库全部流程(子流程候选;缺省 = 空库,选择器只剩「不挂载」) */
  flows?: AgentFlowConfig[];
  /** 本流程 id(子流程自引用/成环判断;新建未保存时为空串) */
  currentFlowId?: string;
  /**
   * 本流程是否启用(草稿值)。只影响「反思步骤挂子流程」那条警示的口径:
   * 后端 `validate_flow` 对未启用流程直接放行该检查;其余引用链警示由全库校验判,
   * 与流程启停无关(见 `stepSubFlowWarnings`)。缺省按启用处理。
   */
  flowEnabled?: boolean;
  /** 本机已落库的连接选项(二维批次 5b;缺省 = 空列表,选择器只剩「默认连接」) */
  connections?: FlowConnectionOption[];
  /**
   * 任务工具策略上下文(设置 → 工具策略;缺省 = 未知)。
   * 只喂给 `stepToolsWarnings` 的白名单取交提示——后端下发的工具 = 策略集 ∩ 步骤白名单,
   * 交集为空时该节点拿不到任何工具且闸门 fail-closed(收口批 2026-09-24)。
   */
  toolPolicy?: ToolPolicyCtx;
}>();

// 就地对草稿里的步骤写入:草稿所有权在调用方(`useAgentFlow` 持有 flowDraft,保存时才提交),
// 表单只是它的编辑入口。这里取步骤对象的引用再用,与 ConnectionSection(`props.state` 解构)
// 同一写法——避免 vue/no-mutating-props 把「表单写自己的字段」误判为改 prop。
//
// **必须是响应式引用(`toRef`),不能解构成值**:Vue 3.5 的「响应式 props 解构」只对
// 直接 `defineProps()` 生效,`const { step } = props` 拿到的是**一次性快照**。
// 草稿会被整体替换(`useAgentFlow.loadFlowConfig` 用深拷贝重建,保存/切换/新建/导入后
// 都走它),此时编辑区仍指向旧对象——列表行读新对象、编辑区写旧对象,用户看得见地分叉,
// 继续编辑的改动在下次保存时**静默丢失**(遗留.md IFW-7①)。取引用后 `step.value`
// 恒为当前 props 上的那个对象,模板写法不变(顶层 ref 在模板中自动解包)。
const step = toRef(props, 'step');
const steps = toRef(props, 'steps');
const flowLib = () => props.flows ?? [];
const selfFlowId = () => props.currentFlowId ?? '';
const conns = () => props.connections ?? [];

/**
 * 下拉的「当前值不在候选里」占位项(无此情形返回空串)。
 * 两种成因的文案不同:库内不存在 → 引用已失效;因成环被候选过滤 → 会成环。
 * 原生 select 在不命中任何 option 时会显示为空白,挂载状态就没有锚点了。
 */
function staleSubFlowOption(): string {
  const id = subFlowId(step.value);
  if (!id) return '';
  if (subFlowCandidates(flowLib(), selfFlowId()).some((f) => f.id === id)) return '';
  const known = flowLib().some((f) => f.id === id);
  if (!known) return `${id}(引用已失效)`;
  return `${flowName(flowLib(), id)}(会成环)`;
}
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
            @change="toggleStepInput(steps, step, cand.id)"
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
    <!-- 静态子图(二维批次 6b):挂上子流程后本节点不再自己发起模型调用,
         而是把该流程当子图跑一遍,子图成果即本节点产出 -->
    <div class="sv-inp-row">
      <label class="sv-inp-tag">子流程</label>
      <select
        class="sv-select flow-select-wide"
        :value="subFlowId(step) ?? ''"
        title="挂载后本节点把该流程当子图执行,成果即本节点产出;本节点自身的目标/档位/工具/提示词都不参与执行(配置保留)"
        @change="setSubFlowId(step, ($event.target as HTMLSelectElement).value || null)"
      >
        <option value="">不挂载（本节点自己生成）</option>
        <option v-for="f in subFlowCandidates(flowLib(), selfFlowId())" :key="f.id" :value="f.id">
          {{ f.name || f.id }}
        </option>
        <!-- 当前值不在候选里(库内已删除,或因成环被候选过滤):补一个禁用项,
             否则原生 select 会显示成空白,挂载状态失去视觉锚点 -->
        <option v-if="staleSubFlowOption()" value="" disabled>
          {{ staleSubFlowOption() }}
        </option>
      </select>
    </div>
    <template v-if="subFlowId(step)">
      <p class="sv-note">{{ subFlowMountNote(flowLib(), subFlowId(step)) }}</p>
      <p class="sv-note">
        子流程按它自己的并行上限执行(嵌套时并发会按层数相乘,token 消耗随之增加);
        最多嵌套 {{ MAX_SUB_FLOW_DEPTH }} 层。
      </p>
      <p v-if="!step.enabled" class="sv-note">
        本步骤已停用,引用暂不参与校验;启用后会被保存期校验。
      </p>
    </template>
    <p
      v-for="(warn, wi) in stepSubFlowWarnings(flowLib(), selfFlowId(), step, props.flowEnabled ?? true)"
      :key="`s${wi}`"
      class="sv-note flow-tool-warn"
    >
      {{ warn }}
    </p>
    <!-- 以下执行参数在挂载子流程后被旁路(整块隐藏,草稿里的配置保留) -->
    <template v-if="!subFlowId(step)">
      <!-- 节点级连接(二维批次 5b):本步走哪一套 API 连接;provider 与模型随该连接。
           与档位无关(严格档同样生效)——严格只决定「跑几轮」,不决定「用谁跑」 -->
      <div class="sv-inp-row">
        <label class="sv-inp-tag">模型连接</label>
        <select
          class="sv-select flow-select-wide"
          :value="stepConnectionId(step) ?? ''"
          title="该步骤用哪一套 API 连接(连接里的模型一并生效);默认连接 = 设置里的默认那条"
          @change="setStepConnectionId(step, ($event.target as HTMLSelectElement).value)"
        >
          <option value="">默认连接（跟随设置）</option>
          <option v-for="c in conns()" :key="c.id" :value="c.id" :disabled="!c.enabled">
            {{ c.name }}{{ c.model ? `｜${c.model}` : '' }}{{ c.enabled ? '' : '(已停用)' }}
          </option>
          <!-- 当前值不在候选里(本机已删除 / 流程来自别的机器):补一个禁用项保位,
               否则原生 select 会显示成空白,引用状态失去视觉锚点(同子流程口径) -->
          <option v-if="staleConnectionLabel(conns(), stepConnectionId(step))" value="" disabled>
            {{ staleConnectionLabel(conns(), stepConnectionId(step)) }}
          </option>
        </select>
        <span class="sv-note">
          {{
            stepConnectionId(step)
              ? '本步走所选连接(模型随连接);引用失效时该步会明确报错,不会回退默认连接。'
              : '本步走默认连接;改这里可为单步指定别的 provider 或模型。'
          }}
        </span>
      </div>
      <p
        v-for="(warn, wi) in stepConnectionWarnings(step, conns())"
        :key="`c${wi}`"
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
               收口批(2026-09-24):白名单档再补一条「与任务工具策略取交」的提示
               (交集为空 → 本节点不下发任何工具,闸门 fail-closed)。
               文案由 stepToolsWarnings 纯函数产出,便于单测覆盖 -->
          <p
            v-for="(warn, wi) in stepToolsWarnings(step, toolPolicy)"
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
          <!-- 节点级工具轮次上限(二维批次 5b):只影响本步的工具自循环;留空 = 沿用全局设置。
               与工具区同进退(严格档不下发工具 → 本行一并隐藏,配置保留,切回宽松档即生效) -->
          <div class="sv-inp-row">
            <label class="sv-inp-tag">工具轮次上限</label>
            <input
              class="sv-input inject-num"
              type="number"
              :min="MAX_TOOL_ROUNDS_MIN"
              :max="MAX_TOOL_ROUNDS_MAX"
              :value="stepToolRoundsText(step)"
              placeholder="沿用全局"
              title="工具轮次上限:本步最多让模型调用几轮工具(1-200);留空 = 沿用全局设置"
              @change="setStepToolRounds(step, ($event.target as HTMLInputElement).value)"
            />
            <span class="sv-note">
              限制本步的工具循环轮数(1-{{ MAX_TOOL_ROUNDS_MAX }});留空沿用全局。
            </span>
          </div>
          <p
            v-for="(warn, wi) in stepToolRoundsWarnings(step)"
            :key="`r${wi}`"
            class="sv-note flow-tool-warn"
          >
            {{ warn }}
          </p>
        </template>
        <div class="sv-inp-row">
          <label class="sv-inp-tag">温度</label>
          <input v-model.number="step.temperature" type="number" min="0" max="2" step="0.1" class="sv-input inject-num" placeholder="沿用全局" />
          <label class="sv-inp-tag">输出上限</label>
          <input
            v-model.number="step.max_tokens"
            type="number"
            :min="STEP_OUTPUT_TOKENS_MIN"
            :max="STEP_OUTPUT_TOKENS_MAX"
            class="sv-input inject-num"
            placeholder="沿用全局"
            :title="`该步骤的输出上限(${STEP_OUTPUT_TOKENS_MIN}~${STEP_OUTPUT_TOKENS_MAX});留空沿用全局最大生成长度`"
          />
        </div>
        <!-- 节点级上下文上限(二维批次 8):本步**输入**的 token 预算。
             与「输出上限」同排:一个管进、一个管出;留空 = 不裁剪(与旧行为一致) -->
        <div class="sv-inp-row">
          <label class="sv-inp-tag">上下文上限</label>
          <input
            v-model.number="step.max_context"
            type="number"
            :min="STEP_MAX_CONTEXT_MIN"
            :max="STEP_MAX_CONTEXT_MAX"
            class="sv-input inject-num"
            placeholder="不限制"
            :title="`本步输入(系统提示 + 任务目标 + 上游产出)的 token 上限(${STEP_MAX_CONTEXT_MIN}~${STEP_MAX_CONTEXT_MAX});留空 = 不限制`"
          />
          <span class="sv-note">
            本步输入的 token 上限:超出时从「最旧的上游产出」起省略(正文换成
            「(因本节点上下文上限省略)」标记,标签行保留);任务目标恒保留(不会被省略)。
          </span>
        </div>
      </template>
      <!-- 节点级调用预算(A 批 A1/A2):单次调用超时 + 空产出重试。
           放在 direct 参数块**之外**(紧跟其后):反思步骤同样会发起一次模型调用,
           两项对它也生效,塞进 direct 块会让反思节点无从配置(后端校验也不分 action)。
           挂载子流程的节点整块不渲染(子图各节点各自的配置生效),草稿里的配置保留 -->
      <div class="sv-inp-row">
        <label class="sv-inp-tag">单次调用超时</label>
        <input
          class="sv-input inject-num"
          type="number"
          :min="STEP_CALL_TIMEOUT_MIN"
          :max="STEP_CALL_TIMEOUT_MAX"
          :value="stepCallTimeoutText(step)"
          placeholder="沿用缺省 300 秒"
          :title="`本步每次模型调用超过该秒数即失败(${STEP_CALL_TIMEOUT_MIN}-${STEP_CALL_TIMEOUT_MAX} 秒);留空沿用缺省 300 秒;挂载子流程的节点上不生效`"
          @change="setStepCallTimeout(step, ($event.target as HTMLInputElement).value)"
        />
        <span class="sv-note">
          本步「每次」模型调用的时间预算({{ STEP_CALL_TIMEOUT_MIN }}-{{ STEP_CALL_TIMEOUT_MAX }} 秒):
          留空沿用缺省 300 秒;可收紧(如 60)也可放宽(如 900),宽松档工具循环逐轮各按它计;
          超时即本步失败,不自动重试。挂载子流程的节点上本项不生效(配置保留)。
        </span>
      </div>
      <p
        v-for="(warn, wi) in stepCallTimeoutWarnings(step)"
        :key="`t${wi}`"
        class="sv-note flow-tool-warn"
      >
        {{ warn }}
      </p>
      <div class="sv-inp-row">
        <label class="sv-inp-tag">空产出重试</label>
        <input
          class="sv-input inject-num"
          type="number"
          :min="STEP_MAX_RETRIES_MIN"
          :max="STEP_MAX_RETRIES_MAX"
          :value="stepMaxRetriesText(step)"
          placeholder="不重试"
          :title="`只在产出为空时重试(${STEP_MAX_RETRIES_MIN}-${STEP_MAX_RETRIES_MAX} 次 = 额外尝试上限);连接失败/超时等错误不重试;每次重试会翻倍输出预算`"
          @change="setStepMaxRetries(step, ($event.target as HTMLInputElement).value)"
        />
        <span class="sv-note">
          只在模型产出为空时重试({{ STEP_MAX_RETRIES_MIN }}-{{ STEP_MAX_RETRIES_MAX }} 次 = 额外尝试上限,
          总尝试 1 + n);连接失败、超时、解析错误等一律不重试;每次重试会翻倍输出预算(封顶 131072)。
        </span>
      </div>
      <p
        v-for="(warn, wi) in stepMaxRetriesWarnings(step)"
        :key="`x${wi}`"
        class="sv-note flow-tool-warn"
      >
        {{ warn }}
      </p>
    </template>
  </div>
</template>
