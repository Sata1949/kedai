<script setup lang="ts">
// 设置区:生成参数(温度/Top-P/长度/上下文窗口/工具轮次 + 压缩/记忆/子代理参数)。
// 从 SettingsModal.vue 双模板合并而来:取 standalone 超集版本(embedded 分支缺失
// 压缩模式/记忆蒸馏/子代理等字段,属模板漂移,合并后两模式一致)。
// 数值字段的默认值由 stores/genSettings.ts 的 loadSettings 集中 `?? 默认` 兜底,
// 本组件不再逐个兜(onMounted 里曾只兜 3 个字段,既冗余又误导)。
import { computed } from 'vue';
import { useAppStore } from '../../store';
import { storeToRefs } from 'pinia';
import { useGenerationParams } from '../../composables/useDataManager';

withDefaults(defineProps<{
  /** 是否显示(embedded 模式按 activeSection 切换;standalone 恒 true) */
  show?: boolean;
}>(), {
  show: true,
});

const store = useAppStore();
// 生成参数直接绑定 store(storeToRefs),与 useGenerationParams 内部保存逻辑读写同一 store
const {
  temperature, topP, maxTokens, maxContextTokens, maxToolRounds,
  maxFlowCallDepth, maxFlowCallsPerTask, defaultNodeMaxContext,
  toolHistoryKeepRounds, toolHistoryBudgetTokens,
  sessionTokenBudget, sessionBudgetAction,
  loopGuardSemanticWindow, loopGuardSemanticMinCalls, loopGuardSemanticMaxDistinct,
  taskStepBudgetSecs, taskIdleTimeoutSecs,
  compactionMode, compactionThreshold, compactionKeepRecent, compactionSnipBytes,
  memoryDistillEnabled, memoryInjectLimit, memoryInjectCharBudget, memoryMaxEntries,
  subagentMaxDepth, subagentMaxConcurrency, subagentResultMaxChars,
} = storeToRefs(store);

const {
  tempLabel, topPLabel, ctxLabel, saveParams, paramsMsg, saveParamsNow,
  taskRecSaving, taskRecMsg, applyTaskRecommended,
} = useGenerationParams();

/** 任务模式缺省块(TM-SET-1)仅任务模式显示;角色扮演侧缺省不同(0.8 / 0.9 / 扁平输出上限) */
const isTaskMode = computed(() => store.appMode === 'task');
</script>

<template>
  <div v-show="show" class="sv-field">
    <div class="sv-field-label"><span class="sv-supreme pink" /> 生成参数</div>
    <div class="sv-stack">
      <div>
        <div class="sv-range-label">
          随机性(Temperature) <span class="sv-range-val">{{ tempLabel }}</span>
        </div>
        <div class="sv-range-row">
          <input v-model.number="temperature" type="range" min="0" max="2" step="0.05" class="sv-range" />
          <output>{{ (temperature ?? 0).toFixed(2) }}</output>
        </div>
        <div class="sv-range-hints"><span>稳定 · 严谨</span><span>多样 · 创意</span></div>
      </div>
      <div>
        <div class="sv-range-label">
          核采样(Top-P) <span class="sv-range-val">{{ topPLabel }}</span>
        </div>
        <div class="sv-range-row">
          <input v-model.number="topP" type="range" min="0" max="1" step="0.01" class="sv-range" />
          <output>{{ (topP ?? 0).toFixed(2) }}</output>
        </div>
        <div class="sv-range-hints"><span>严格 · 确定</span><span>宽松 · 多样</span></div>
      </div>
      <div>
        <div class="sv-range-label">最大生成长度</div>
        <div class="sv-range-row">
          <!-- 上限与后端 `default_max_tokens` 校验区间(1..=131072)一致。
               超大区间下拖动精度有限,故同排保留数字输入用于精确录入。 -->
          <input v-model.number="maxTokens" type="range" min="128" max="131072" step="1" class="sv-range" />
          <input
            v-model.number="maxTokens"
            type="number"
            min="1"
            max="131072"
            step="1"
            class="sv-input inject-num"
            title="单次生成的最大输出 token 数(含推理模型的 reasoning 消耗)"
          />
        </div>
        <div class="sv-range-hints"><span>128 · 省钱</span><span>131072 · 上限</span></div>
      </div>
      <div>
        <div class="sv-range-label">
          最大上下文窗口(Token) <span class="sv-range-val">{{ ctxLabel }}</span>
        </div>
        <div class="sv-range-row">
          <input
            v-model.number="maxContextTokens"
            type="range"
            min="65536"
            max="1048576"
            step="1024"
            class="sv-range"
          />
          <output>{{ (maxContextTokens ?? 0).toLocaleString() }}</output>
        </div>
        <div class="sv-range-hints"><span>64k · 最低</span><span>1M · 上限</span></div>
        <p class="sv-note">超出上限时按时间裁剪最旧的历史消息(角色设定始终保留)。</p>
      </div>
      <div class="sv-inp-row">
        <label class="sv-inp-tag">工具轮次上限</label>
        <input
          v-model.number="maxToolRounds"
          type="number"
          min="1"
          max="200"
          step="1"
          class="sv-input inject-num"
          title="AGENT/CUSTOM 模式工具循环轮次上限(每轮可执行多个工具调用;默认 32)"
        />
        <span class="sv-note">AGENT/CUSTOM 工具循环轮次上限(1-200,默认 32)</span>
      </div>
      <!-- 流程调用闸与节点默认上下文(A 批 A3/A4):三者都是**任务侧**设置
           (服务端 `apply!(s, is_task, …)` 落在任务覆盖层),角色扮演侧不受影响。
           缺省值全部等于 A 批之前的行为:深度 2 / 每任务 8 / 不裁剪(0)。 -->
      <div class="sv-inp-row">
        <label class="sv-inp-tag">流程调用深度</label>
        <input
          v-model.number="maxFlowCallDepth"
          type="number"
          min="1"
          max="5"
          step="1"
          class="sv-input inject-num"
          title="任务里节点用 run_flow 工具逐层调用流程的嵌套深度上限(1-5,默认 2);静态子流程(sub_flow_id)的嵌套上限另有 3 层的结构限制"
        />
        <span class="sv-note">任务侧:流程动态调用嵌套深度(1-5,默认 2)</span>
      </div>
      <div class="sv-inp-row">
        <label class="sv-inp-tag">每任务调用上限</label>
        <input
          v-model.number="maxFlowCallsPerTask"
          type="number"
          min="1"
          max="64"
          step="1"
          class="sv-input inject-num"
          title="单个任务内流程调用(run_flow)的次数上限(1-64,默认 8);达到后该任务不再允许新的流程调用"
        />
        <span class="sv-note">任务侧:单个任务的流程调用次数上限(1-64,默认 8)</span>
      </div>
      <div class="sv-inp-row">
        <label class="sv-inp-tag">节点默认上下文</label>
        <input
          v-model.number="defaultNodeMaxContext"
          type="number"
          min="0"
          max="1048576"
          step="1"
          class="sv-input inject-num"
          placeholder="0 = 不裁剪"
          title="节点自己没写「上下文上限」时用的默认 token 上限(0 = 不裁剪,默认;否则 256-1048576);节点级配置优先"
        />
        <span class="sv-note">任务侧:节点未单独设置时的输入 token 上限(0 = 不裁剪,否则 256-1048576)</span>
      </div>
      <div class="sv-inp-row">
        <label class="sv-inp-tag">工具历史保留轮数</label>
        <input
          v-model.number="toolHistoryKeepRounds"
          type="number"
          min="1"
          max="32"
          step="1"
          class="sv-input inject-num"
          title="工具循环历史保留的最近完整轮数;超出后最老轮的 tool 结果原地替换为摘要,防止上下文无界膨胀"
        />
        <span class="sv-note">最近完整保留轮数(1-32,默认 4;超出部分摘要化)</span>
      </div>
      <div class="sv-inp-row">
        <label class="sv-inp-tag">工具历史 token 预算</label>
        <input
          v-model.number="toolHistoryBudgetTokens"
          type="number"
          min="0"
          max="1048576"
          step="1024"
          class="sv-input inject-num"
          title="工具循环历史 token 预算;估算超预算时从最老完整轮起继续摘要,保底最近 1 轮完整。0 = 禁用预算闸门"
        />
        <span class="sv-note">0 = 禁用;否则 1024-1048576(默认 16384)</span>
      </div>
      <div class="sv-inp-row">
        <label class="sv-inp-tag">单次生成 token 预算</label>
        <input
          v-model.number="sessionTokenBudget"
          type="number"
          min="0"
          max="1000000000"
          step="1024"
          class="sv-input inject-num"
          title="单次生成内工具循环累计 token(prompt+completion)上限;0 = 关闭(默认)。达到上限按下一项的动作处置"
        />
        <span class="sv-note">0 = 关闭;否则 1024-1000000000(默认 0)</span>
      </div>
      <div class="sv-inp-row">
        <label class="sv-inp-tag">预算超限动作</label>
        <select
          v-model="sessionBudgetAction"
          class="sv-select"
          title="warn = 只在推理链提示一次并继续;stop = 提示后停止本轮工具循环(已生成正文与用量照常保留)"
        >
          <option value="warn">warn · 只提示</option>
          <option value="stop">stop · 停止工具循环</option>
        </select>
        <span class="sv-note">默认 warn;stop 用于硬性控成本(长工具循环提前收尾)</span>
      </div>
      <div class="sv-inp-row">
        <label class="sv-inp-tag">空转熔断次数下限</label>
        <input
          v-model.number="loopGuardSemanticMinCalls"
          type="number"
          min="0"
          max="64"
          step="1"
          class="sv-input inject-num"
          title="同一工具在窗口内被调用达到该次数、且输出实质无变化时熔断空转;0 = 关闭本闸门"
        />
        <span class="sv-note">0 = 关闭;否则 4-64(默认 12)</span>
      </div>
      <div class="sv-inp-row">
        <label class="sv-inp-tag">空转熔断窗口 / 输出去重上限</label>
        <input
          v-model.number="loopGuardSemanticWindow"
          type="number"
          min="4"
          max="64"
          step="1"
          class="sv-input inject-num"
          title="近多少次工具调用内做空转判定(默认 16)"
        />
        <input
          v-model.number="loopGuardSemanticMaxDistinct"
          type="number"
          min="1"
          max="8"
          step="1"
          class="sv-input inject-num"
          title="窗口内同工具输出指纹去重后不超过该值即判空转(默认 2)"
        />
        <span class="sv-note">窗口 4-64(默认 16);去重上限 1-8(默认 2)</span>
      </div>
      <div class="sv-inp-row">
        <label class="sv-inp-tag">步骤墙钟预算</label>
        <input
          v-model.number="taskStepBudgetSecs"
          type="number"
          min="0"
          max="86400"
          step="1"
          class="sv-input inject-num"
          title="任务模式**单步工具循环**的墙钟上限(秒):到点带着已有产出收尾并记完成,不判失败;0 = 关闭本闸门"
        />
        <span class="sv-note">任务侧:单步工具循环上限(默认 1200;0 = 关;建议 ≥300)</span>
      </div>
      <div class="sv-inp-row">
        <label class="sv-inp-tag">任务空闲超时</label>
        <input
          v-model.number="taskIdleTimeoutSecs"
          type="number"
          min="0"
          max="86400"
          step="1"
          class="sv-input inject-num"
          title="任务模式:运行中的任务连续该秒数既无模型调用也无事件时,由看守自动收尾(与手动停止同源,原因写入任务错误栏);0 = 关闭看守"
        />
        <span class="sv-note">任务侧:无活动自动收尾(默认 900;0 = 关;601-86400)</span>
      </div>
      <div class="sv-inp-row">
        <label class="sv-inp-tag">压缩模式</label>
        <select v-model="compactionMode" class="sv-select" title="上下文压缩模式:off 不压缩 / manual 手动触发 / auto token 超阈值自动压缩">
          <option value="off">off · 不压缩</option>
          <option value="manual">manual · 手动触发</option>
          <option value="auto">auto · 自动压缩</option>
        </select>
        <span class="sv-note">压缩较早对话为摘要(原文保留可恢复);manual 需在优化面板手动触发</span>
      </div>
      <div>
        <div class="sv-range-label">压缩触发阈值 <span class="sv-range-val">{{ Math.round(compactionThreshold * 100) }}%</span></div>
        <div class="sv-range-row">
          <input
            v-model.number="compactionThreshold"
            type="range"
            min="0.5"
            max="0.95"
            step="0.05"
            class="sv-range"
          />
          <output>{{ Math.round(compactionThreshold * 100) }}%</output>
        </div>
        <div class="sv-range-hints"><span>50% · 更早压缩</span><span>95% · 更晚压缩</span></div>
        <p class="sv-note">仅 auto 模式生效:历史 token 达到该占比时自动压缩。</p>
      </div>
      <div class="sv-inp-row">
        <label class="sv-inp-tag">压缩保留条数</label>
        <input
          v-model.number="compactionKeepRecent"
          type="number"
          min="2"
          max="200"
          step="1"
          class="sv-input inject-num"
          title="压缩后仍保留的最近消息条数(2-200;默认 4,越界由后端钳回默认)"
        />
        <span class="sv-note">压缩后保留的最近消息条数(2-200,默认 4)</span>
      </div>
      <div class="sv-inp-row">
        <label class="sv-inp-tag">超长裁剪阈值</label>
        <input
          v-model.number="compactionSnipBytes"
          type="number"
          min="0"
          max="1048576"
          step="1"
          class="sv-input inject-num"
          title="snip 零成本裁剪的超长消息长度阈值(字节;0 = 禁用,上限 1MB;默认 8192)"
        />
        <span class="sv-note">超长消息裁剪阈值(字节;0 = 禁用,默认 8192)</span>
      </div>
      <div class="sv-inp-row">
        <label class="sv-inp-tag">记忆蒸馏</label>
        <label style="display: flex; gap: 6px; align-items: center; cursor: pointer" title="开启后允许在优化面板把当前会话蒸馏为角色跨会话记忆">
          <input v-model="memoryDistillEnabled" type="checkbox" style="flex-shrink: 0" />
          <span class="sv-note">开启跨会话记忆蒸馏(优化面板 → 记忆库)</span>
        </label>
      </div>
      <div class="sv-inp-row">
        <label class="sv-inp-tag">记忆注入条数</label>
        <input
          v-model.number="memoryInjectLimit"
          type="number"
          min="0"
          max="50"
          step="1"
          class="sv-input inject-num"
          title="每次注入提示词的记忆条数上限(0-50;0 = 不注入,默认 8,越界由后端钳回默认)"
        />
        <span class="sv-note">注入提示词的记忆条数上限(0-50,默认 8)</span>
      </div>
      <div class="sv-inp-row">
        <label class="sv-inp-tag">记忆字符预算</label>
        <input
          v-model.number="memoryInjectCharBudget"
          type="number"
          min="0"
          max="20000"
          step="100"
          class="sv-input inject-num"
          title="每次注入的记忆内容总字符预算(0-20000;0 = 不限制,默认 2000):按精选排序累积到预算即停"
        />
        <span class="sv-note">注入记忆的字符预算(0-20000,0 = 不限制,默认 2000)</span>
      </div>
      <div class="sv-inp-row">
        <label class="sv-inp-tag">记忆容量上限</label>
        <input
          v-model.number="memoryMaxEntries"
          type="number"
          min="0"
          max="10000"
          step="10"
          class="sv-input inject-num"
          title="每角色记忆容量上限(0-10000;0 = 不淘汰,默认 200):超出后最低分条目置为已归档(不删除)"
        />
        <span class="sv-note">每角色记忆条数上限(0-10000,0 = 不限制,默认 200)</span>
      </div>
      <div class="sv-inp-row">
        <label class="sv-inp-tag">子代理深度</label>
        <input
          v-model.number="subagentMaxDepth"
          type="number"
          min="1"
          max="4"
          step="1"
          class="sv-input inject-num"
          title="子智能体最大嵌套深度(1-4;默认 2,超限时提示主智能体直接处理)"
        />
        <span class="sv-note">子智能体嵌套深度上限(1-4,默认 2)</span>
      </div>
      <div class="sv-inp-row">
        <label class="sv-inp-tag">子代理并发</label>
        <input
          v-model.number="subagentMaxConcurrency"
          type="number"
          min="1"
          max="16"
          step="1"
          class="sv-input inject-num"
          title="子智能体最大并发数(1-16;默认 6,占满时新派发被拒并提示稍后重试)"
        />
        <span class="sv-note">子智能体并发上限(1-16,默认 6)</span>
      </div>
      <div class="sv-inp-row">
        <label class="sv-inp-tag">子代理结果上限</label>
        <input
          v-model.number="subagentResultMaxChars"
          type="number"
          min="500"
          max="8000"
          step="100"
          class="sv-input inject-num"
          title="子智能体结果最大字符数(500-8000;默认 2000,超出截断并附原长尾注)"
        />
        <span class="sv-note">子智能体结果字符上限(500-8000,默认 2000)</span>
      </div>
      <!-- 任务模式缺省生成配置(TM-SET-1):仅任务模式显示。任务覆盖层未显式配置时,
           任务运行取任务缺省(0.3 / 1.0 / 8192,不沿用角色扮演风味值);
           本页「保存为默认参数」会把滑块当前值写成**任务显式值**并冻结,故给一键写推荐值。 -->
      <div v-if="isTaskMode" class="sv-separator">
        <div class="sv-field-label sub">任务模式缺省</div>
        <p class="sv-note">
          任务未单独设置时使用:温度 0.3 · Top-P 1.0 · 输出上限 8192(输出上限只抬不压:
          更大时保持更大)。显式保存过的值优先于缺省。
        </p>
        <div class="sv-btn-row">
          <button
            class="sv-btn ghost sv-btn-fill"
            :disabled="taskRecSaving"
            title="把温度 0.3 / Top-P 1.0 / 输出上限 8192 显式写入任务模式设置(此前批量保存冻结过旧值的用户可用它一键采用新缺省)"
            @click="applyTaskRecommended"
          >
            {{ taskRecSaving ? '写入中...' : '写入任务推荐值(0.3 / 1.0 / 8192)' }}
          </button>
        </div>
        <div v-if="taskRecMsg" class="sv-feedback ok sv-feedback-flex">{{ taskRecMsg }}</div>
      </div>
      <div class="sv-btn-row">
        <button class="sv-btn ghost sv-btn-fill" :disabled="saveParams" @click="saveParamsNow">
          {{ saveParams ? '保存中...' : '保存为默认参数' }}
        </button>
        <div v-if="paramsMsg" class="sv-feedback ok sv-feedback-flex">{{ paramsMsg }}</div>
      </div>
    </div>
  </div>
</template>
