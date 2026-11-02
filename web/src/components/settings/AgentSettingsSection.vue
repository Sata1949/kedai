<script setup lang="ts">
// arch-allow-file: 设置表单以 v-model 直写 store 草稿,保存统一走 queueSettingsSave;逐字段 action 只增加转发层。
// 设置区:Agent 设置(系统提示词/搜索端点/变量注入位置/反思提示词 + 主 Agent 提示词 + 最终提示词预览)。
// 从 SettingsModal.vue 双模板合并而来:取 embedded 超集版本(standalone 分支缺失
// 「最终提示词预览」块,属模板漂移,合并后 standalone 一并补上)。
import { computed, onMounted, watch } from 'vue';
import { useAppStore } from '../../store';
import { storeToRefs } from 'pinia';
import { useAgentSettings } from '../../composables/useAgentSettings';
import { useAgentPromptEditor } from '../../composables/useAgentPromptEditor';
import { promptWarningText } from '../../utils/promptWarnings';

withDefaults(defineProps<{
  /** 是否显示(embedded 模式按 activeSection 切换;standalone 恒 true) */
  show?: boolean;
}>(), {
  show: true,
});

const store = useAppStore();
// Agent 设置直接绑定 store(storeToRefs),与 useAgentSettings 保存逻辑读写同一 store;
// appMode 用于「系统提示词按模式独立存储」的 UI 标注(徽标/说明/按钮文案随模式即时切换)
const { agentSystemPrompt, searchEndpoint, mvuVarsPosition, reflectPrompt, appMode, mvuModel, mvuTemperatureInput, model, models, settingsMode, settingsError } = storeToRefs(store);

/** 变量生成模型下拉的候选:当前正文模型 + 已拉取到的可用模型列表(HB-7) */
const modelOptions = computed(() => {
  const list = [...new Set([model.value, ...models.value])].filter(Boolean) as string[];
  return list;
});

/** 模式徽标文案:标注当前编辑的是哪个模式的提示词,避免误以为两模式共用一份 */
const modeBadgeText = computed(() => (appMode.value === 'task' ? '任务模式专属' : '角色扮演专属'));
/** 占位符按模式区分空值回退语义:task 模式回退内置任务默认词,roleplay 回退内置角色扮演人设词 */
const promptPlaceholder = computed(() =>
  appMode.value === 'task'
    ? '留空使用内置默认任务提示词。'
    : '留空使用内置默认角色扮演提示词。支持占位符:{{character_name}} {{character_description}} {{world_info}}',
);
/** 「恢复默认」按钮文案:task 模式点明恢复的是任务模式默认词(与角色扮演默认词不同) */
const resetPromptLabel = computed(() => (appMode.value === 'task' ? '恢复任务模式默认提示词' : '恢复默认提示词'));
/** 任务模式:变量组与反思提示词(聊天专属消费)隐藏(TM-SET-3) */
const isTaskMode = computed(() => appMode.value === 'task');
/**
 * 正在切换模式:store 里已回填的设置仍属于上一个模式(RTSPLIT-2)。
 * 请求在飞或刚失败时置位,这期间文本框里是旧模式的词——不提示的话,用户看到的
 * 正是「切了模式但提示词没变」。
 */
const switchingMode = computed(() => settingsMode.value !== null && settingsMode.value !== appMode.value);
/** 目标模式名(切换提示条文案用) */
const targetModeLabel = computed(() => (appMode.value === 'task' ? '任务模式' : '角色扮演模式'));

/** 运行时主提示词的占位与说明按模式区分(RTSPLIT-1:两份文件,任务侧缺省为空) */
const runtimePromptPlaceholder = computed(() =>
  appMode.value === 'task'
    ? '留空 = 任务模式不额外注入运行时主提示词。'
    : '加载中...',
);
const runtimePromptBadge = computed(() => (appMode.value === 'task' ? '任务模式专属' : '角色扮演专属'));

const { agentSaving, agentMsg, resetAgentPrompt, saveAgentNow } = useAgentSettings();

const {
  agentPromptMd, promptPreview, previewLoading, previewError, mdLoading, mdSaving, mdMsg,
  loadPromptPreview, loadAgentPromptMd, saveAgentPromptMd,
} = useAgentPromptEditor();

/** 保存 Agent 设置成功后重拉预览(系统提示词是预览的一层,不重拉会停在旧文本) */
async function onSaveAgent(): Promise<void> {
  if (await saveAgentNow()) await loadPromptPreview();
}

/** 恢复默认同理:清空提示词会直接改变预览里的模板层 */
async function onResetAgent(): Promise<void> {
  if (await resetAgentPrompt()) await loadPromptPreview();
}

onMounted(async () => {
  // 加载当前模式对应的运行时主 Agent 提示词与脱敏后的最终分层预览
  await loadAgentPromptMd();
  await loadPromptPreview();
});

// 切模式要同时换两样东西(RTSPLIT-1/2):
//   ① 系统提示词框由 genSettings.reloadSettings 回填(见 setAppMode);
//   ② 运行时主提示词是**两份文件**,必须显式重载——此前只重拉预览,正文停在旧模式那份,
//      这就是「切模式后运行提示词一字不变」的直接原因之一。
watch(appMode, async () => {
  await loadAgentPromptMd();
  await loadPromptPreview();
});
</script>

<template>
  <div v-show="show" class="sv-field">
    <div class="sv-field-label"><span class="sv-supreme blue" /> Agent</div>
    <div class="sv-stack">
      <!-- 切换中提示(RTSPLIT-2):回填的设置仍属于上一个模式时明确告知,
           否则文本框里的旧模式文本会被读成「提示词不随模式切换」 -->
      <div v-if="switchingMode" class="sv-feedback info">
        正在切换为{{ targetModeLabel }}…当前显示的可能仍是上一模式的提示词。
      </div>
      <!-- 设置加载失败(RTSPLIT-2):此前静默吞掉,失败与「切模式没生效」无法区分 -->
      <div v-if="settingsError" class="sv-feedback err">{{ settingsError }}</div>
      <!-- 模式徽标:标注当前编辑的是哪个模式的提示词(两模式独立存储),随 appMode 即时切换 -->
      <div class="sv-field-label sub">
        系统提示词(Agent 模式)
        <span class="sv-tag">{{ modeBadgeText }}</span>
      </div>
      <textarea
        v-model="agentSystemPrompt"
        rows="6"
        class="sv-input"
        :placeholder="promptPlaceholder"
        spellcheck="false"
      />
      <p class="sv-note">该提示词按模式独立存储，互不影响。</p>
      <!-- 说明按模式分叉:任务侧是**追加**在内置执行者指令之后,不是替换;
           沿用角色扮演那句「完整替换内置模板」会让用户以为写了就覆盖一切(RTSPLIT-4)。 -->
      <p v-if="!isTaskMode" class="sv-note">
        <b>Custom Prompt 会完整替换内置模板，不是追加。</b>占位符替换为当前角色与已命中的世界书内容。不写
        <code v-pre>{{world_info}}</code> 时世界书不会自动注入（<b>变量状态块除外</b>：它是变量更新的唯一
        载体，漏写占位符时由引擎兜底注入到系统提示词末尾，避免变量更新静默停摆）。可用工具清单与「何时调用」由引擎在
        Agent 模式下动态注入到系统提示词末尾，无需在此手写工具说明。
      </p>
      <p v-else class="sv-note">
        <b>任务模式是追加，不是替换：</b>本段注入在内置执行者指令、执行者职责、世界书之后，内置指令始终保留。
        支持占位符 <code v-pre>{{char}}</code> / <code v-pre>{{user}}</code> / <code v-pre>{{lastUserMessage}}</code>
        （任务目标），其余占位符原样保留。工具由请求参数下发，引擎不在任务侧注入文字版工具指南。
      </p>
      <div class="sv-inp-row">
        <label class="sv-inp-tag">搜索端点</label>
        <input
          v-model="searchEndpoint"
          type="text"
          class="sv-input"
          placeholder="https://html.duckduckgo.com/html/(留空 = 默认 DuckDuckGo)"
          spellcheck="false"
        />
      </div>
      <p class="sv-note">search 工具联网搜索使用的端点；自定义服务需返回相似 HTML 结构。</p>
      <!-- 变量组与反思提示词:仅角色扮演(聊天)消费,任务模式隐藏(TM-SET-3);
           批量保存路径同步按模式过滤(useAgentSettings)。 -->
      <template v-if="!isTaskMode">
      <div class="sv-inp-row">
        <label class="sv-inp-tag" title="变量状态注入位置">变量状态注入位置</label>
        <select v-model="mvuVarsPosition" class="sv-input">
          <option value="system">system 提示词(默认，兼容)</option>
          <option value="user_tail">最新用户消息尾部(缓存友好)</option>
        </select>
      </div>
      <p class="sv-note">
        世界书/变量状态(<code v-pre>{{format_message_variable}}</code>)的注入位置。「最新用户消息尾部」模式把
        变量块移出 system 提示词，变量更新不再使 system + 早期历史的前缀缓存整体失效，可显著提升
        DeepSeek / Anthropic / OpenAI 等提供商的 prompt caching 命中率；需角色卡把
        <code v-pre>{{format_message_variable}}</code> 写在常驻世界书条目中才生效。
      </p>

      <!-- HB-7:变量两步生成(状态栏/变量更新)的独立模型与温度档。
           此前字段可落盘但无 API/UI 通路、无消费点(死配置),本批次接线。 -->
      <div class="sv-inp-row">
        <label class="sv-inp-tag">变量生成模型</label>
        <select v-model="mvuModel" class="sv-input" title="变量与状态栏两步生成使用的模型;留空 = 与正文共用同一模型">
          <option value="">跟随正文模型(默认)</option>
          <option v-for="m in modelOptions" :key="m" :value="m">{{ m }}</option>
        </select>
      </div>
      <div class="sv-inp-row">
        <label class="sv-inp-tag">变量生成温度</label>
        <input
          v-model="mvuTemperatureInput"
          type="number"
          min="0"
          max="2"
          step="0.05"
          class="sv-input inject-num"
          placeholder="跟随内置(0.3)"
          title="变量两步生成的温度档;留空 = 用内置 0.3(与正文温度解耦,降低温度可提高结构化遵循度)"
        />
      </div>
      <p class="sv-note">变量更新/状态栏调用独立于正文：模型留空即共用，温度留空用内置 0.3(0-2,越小越稳)。</p>
      <div class="sv-inp-row" style="align-items: flex-start">
        <label class="sv-inp-tag" style="padding-top: var(--space-2)">反思提示词</label>
        <textarea
          v-model="reflectPrompt"
          rows="3"
          class="sv-input"
          placeholder="留空 = 内置规则检查;填写后反思步骤调用一次 LLM 判定输出 PASS / FAIL"
          spellcheck="false"
        />
      </div>
      <p class="sv-note">
        深度 / Agent 模式的反思步骤：留空时用内置规则(空输出、截断、未答疑问)检查；填写后改为调用模型，按本提示词检查
        草稿是否满足人设、字数、格式等要求，判定输出须以 <code>PASS</code> 或 <code>FAIL</code> 开头，失败自动重新生成
        (最多 3 次)。模型输出无法解析时自动回退内置规则，不影响流程。
      </p>
      </template>
      <p v-if="isTaskMode" class="sv-note">
        变量与状态栏、反思提示词仅角色扮演(聊天)使用，已在任务模式下隐藏；如需调整请切到角色扮演模式。
      </p>
      <div class="sv-btn-row">
        <button class="sv-btn primary sv-btn-fill" :disabled="agentSaving" @click="onSaveAgent">
          {{ agentSaving ? '保存中...' : '保存 Agent 设置' }}
        </button>
        <button class="sv-btn ghost sv-btn-fill" :disabled="agentSaving" @click="onResetAgent">
          {{ resetPromptLabel }}
        </button>
        <div v-if="agentMsg" class="sv-feedback ok sv-feedback-flex">{{ agentMsg }}</div>
      </div>

      <div class="sv-separator">
        <div class="sv-field-label sub">授权与命令执行</div>
        <p class="sv-note">
          工具授权(三档模式 / 始终需授权 / 已授权限撤销)与命令执行(总开关、Android
          ROOT/Shizuku 档位、执行审计)已移到独立分区「Agent 与任务 → 授权与命令执行」,
          便于在移动端直接找到授权入口。
        </p>
      </div>

      <div class="sv-separator">
        <div class="sv-field-label sub">
          主 Agent 提示词(运行时，注入模型)
          <span class="sv-tag">{{ runtimePromptBadge }}</span>
        </div>
        <textarea
          v-model="agentPromptMd"
          rows="14"
          class="sv-input sv-md-area"
          :placeholder="runtimePromptPlaceholder"
          spellcheck="false"
        />
        <p class="sv-note">
          该文件按模式独立存储，切换模式会切换到对应文件：角色扮演用
          <code>DATA_DIR/AGENTS_RUNTIME.md</code>（缺省为内置创作向文本），任务模式用
          <code>DATA_DIR/AGENTS_RUNTIME.task.md</code>（<b>缺省为空 = 任务模式不额外注入</b>）。
          保存后下一轮生成即生效。
        </p>
        <p class="sv-note">
          内容会注入到对应模式 system 消息开头。角色扮演侧内含「角色定位 / 创作原则 / 工具使用原则 / 输出纪律」，
          其中「工具使用原则」段只在<b>带工具的模式</b>(Agent / 自定义流程)下发——快速 / 深度模式没有工具调用能力，
          该段由引擎整段省略，无需为此改文本。任务模式注入时会按任务目标展开
          <code v-pre>{{char}}</code> / <code v-pre>{{user}}</code> / <code v-pre>{{lastUserMessage}}</code> 占位符，
          并以不可信来源边界包裹；任务的执行者指令、世界书与「系统提示词」仍照常注入，不会因留空而丢失。
        </p>
        <p class="sv-note">
          项目根 AGENTS.md 仅供开发 agent 使用，不在此编辑，也绝不会注入运行时模型。
        </p>
        <div class="sv-btn-row">
          <button class="sv-btn primary sv-btn-fill" :disabled="mdSaving" @click="saveAgentPromptMd">
            {{ mdSaving ? '保存中...' : '保存主 Agent 提示词' }}
          </button>
          <button class="sv-btn ghost sv-btn-fill" :disabled="mdLoading" @click="loadAgentPromptMd">
            {{ mdLoading ? '加载中...' : '重新加载' }}
          </button>
          <div
            v-if="mdMsg"
            class="sv-feedback-flex"
            :class="mdMsg.kind === 'ok' ? 'ok' : 'err'"
          >{{ mdMsg.text }}</div>
        </div>
      </div>

      <div class="sv-separator">
        <div class="sv-field-label sub">最终提示词预览（脱敏）</div>
        <p class="sv-note">按 source / role / layer / order 展示。历史仅显示角色、长度与哈希，不返回聊天正文或 API Key。</p>
        <button class="sv-btn ghost sv-btn-fill" :disabled="previewLoading" @click="loadPromptPreview">
          {{ previewLoading ? '加载中...' : '刷新最终提示词预览' }}
        </button>
        <div v-if="previewError" class="sv-feedback err">{{ previewError }}</div>
        <div v-if="promptPreview" class="sv-stack" style="margin-top: var(--space-2-5)">
          <!-- 结构化告警(WB-E):后端只回码,文案在 utils/promptWarnings 单一出处映射 -->
          <div v-for="code in promptPreview.warnings" :key="code" class="sv-feedback info">
            {{ promptWarningText(code) }}
          </div>
          <div v-for="layer in promptPreview.layers" :key="`${layer.order}-${layer.source}`" class="sv-data-row">
            <div class="info">
              <b>#{{ layer.order }} · L{{ layer.layer }} · {{ layer.role }} · {{ layer.source }}</b>
              <pre class="sv-code" style="white-space: pre-wrap; max-height: 180px; overflow: auto">{{ layer.content }}</pre>
            </div>
          </div>
          <p class="sv-note">{{ promptPreview.note }}</p>
        </div>
      </div>
    </div>
  </div>
</template>
