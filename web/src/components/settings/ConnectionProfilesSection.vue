<script setup lang="ts">
// 设置区:连接配置(多套 API 连接:新增 / 编辑 / 删除 / 设为默认 / 启停;
// 任务模式默认连接选择 TM-SET-1:仅任务模式显示,切换即存)。
// 状态由壳(SettingsModal)创建一次后经 prop 传入 —— 与 ApiSettingsSection 同范式。
import { computed, onMounted } from 'vue';
import {
  API_STYLE_LABELS,
  API_STYLE_ORDER,
  CONNECTION_CAPABILITIES,
  CONNECTOR_TYPE_LABELS,
  CONNECTOR_TYPE_ORDER,
} from '../../api/labels';
import type { useConnectionProfiles } from '../../composables/useConnectionProfiles';

const props = withDefaults(
  defineProps<{
    /** useConnectionProfiles 的返回对象(壳共享实例) */
    state: ReturnType<typeof useConnectionProfiles>;
    /** 是否显示(embedded 模式按 activeSection 切换;standalone 恒 true) */
    show?: boolean;
  }>(),
  {
    show: true,
  },
);

const {
  drafts,
  activeIndex,
  loading,
  saving,
  feedback,
  load,
  addDraft,
  requestRemove,
  setActive,
  save,
  taskDefaultConnectionId,
  isTaskMode,
  saveTaskDefaultConnection,
} = props.state;

/** 未登记的类型原样展示(与 labels.ts 的「未登记原样」语义一致) */
const typeLabel = (t: string): string => CONNECTOR_TYPE_LABELS[t] ?? t;
/** 未登记的方言原样展示(同上) */
const apiStyleLabel = (s: string): string => API_STYLE_LABELS[s] ?? s;

/** 任务默认连接引用悬空(已不存在/停用):运行期后端软回退默认连接,这里给出显式提示 */
const taskDefaultDangling = computed(() =>
  !!taskDefaultConnectionId.value
  && !drafts.value.some((d) => d.id === taskDefaultConnectionId.value && d.enabled),
);

function onTaskDefaultChange(e: Event): void {
  void saveTaskDefaultConnection((e.target as HTMLSelectElement).value);
}

onMounted(() => {
  void load();
});
</script>

<template>
  <div v-show="props.show" class="sv-field sv-conn-section">
    <div class="sv-field-label"><span class="sv-supreme red" /> 连接配置</div>
    <div class="sv-stack">
      <div v-if="loading" class="sv-count">加载中...</div>
      <div v-else-if="!drafts.length" class="sv-count">暂无连接,点「新增连接」添加一条。</div>

      <div v-for="(d, i) in drafts" :key="d.id || 'new-' + i" class="sv-conn-card">
        <!-- 卡头:默认单选并进标签格(若单选自成 flex 项,会把名称输入右推 23px、与下方字段列错位) -->
        <div class="sv-inp-row sv-conn-head">
          <label class="sv-inp-tag sv-conn-def" :for="`conn-default-${i}`"
            >默认
            <input
              :id="`conn-default-${i}`"
              type="radio"
              name="kedai-conn-default"
              :checked="activeIndex === i"
              :disabled="!d.enabled"
              :title="d.enabled ? '设为默认连接' : '已停用的连接不能作为默认连接'"
              @change="setActive(i)"
          /></label>
          <input
            v-model="d.name"
            type="text"
            class="sv-input"
            placeholder="连接名称"
            spellcheck="false"
          />
          <label class="sv-inp-tag" :for="`conn-enabled-${i}`">启用</label>
          <input :id="`conn-enabled-${i}`" v-model="d.enabled" type="checkbox" />
          <button class="sv-btn danger" @click="requestRemove(i)">删除</button>
        </div>

        <div class="sv-inp-row">
          <label class="sv-inp-tag">类型</label>
          <select v-model="d.connector_type" class="sv-select">
            <option v-for="t in CONNECTOR_TYPE_ORDER" :key="t" :value="t">
              {{ typeLabel(t) }}
            </option>
          </select>
        </div>

        <div class="sv-inp-row">
          <label class="sv-inp-tag" title="接口方言:决定请求端点与流式协议;保存时按 BASE URL 端点后缀自动识别,此处可显式指定">接口格式</label>
          <select v-model="d.api_style" class="sv-select">
            <option v-for="s in API_STYLE_ORDER" :key="s" :value="s">
              {{ apiStyleLabel(s) }}
            </option>
          </select>
        </div>

        <div class="sv-inp-row">
          <label class="sv-inp-tag">BASE URL</label>
          <input
            v-model="d.base_url"
            type="text"
            class="sv-input"
            placeholder="https://api.openai.com/v1"
            spellcheck="false"
          />
        </div>

        <div class="sv-inp-row">
          <label class="sv-inp-tag">API KEY</label>
          <input
            v-model="d.api_key"
            type="password"
            class="sv-input"
            :placeholder="
              d.has_api_key ? `已配置 ${d.api_key_masked} · 留空保持现有` : '粘贴 API Key'
            "
            autocomplete="off"
            spellcheck="false"
          />
        </div>

        <div class="sv-inp-row">
          <label class="sv-inp-tag">MODEL</label>
          <input
            v-model="d.model"
            type="text"
            class="sv-input"
            placeholder="模型名"
            spellcheck="false"
          />
        </div>

        <!-- 模型能力位(2026-10-02 视觉能力包 D1):视觉输入 / 大图拆分已接入消费;
             其余三项为预留声明(如实标注,勾选仅记录) -->
        <div class="sv-inp-row sv-conn-caps">
          <label class="sv-inp-tag">模型能力</label>
          <div class="sv-conn-cap-list">
            <label
              v-for="cap in CONNECTION_CAPABILITIES"
              :key="cap.key"
              class="sv-conn-cap"
              :class="{ reserved: cap.reserved }"
              :title="cap.tip"
            >
              <input v-model="d[cap.key]" type="checkbox" />
              <span>{{ cap.label }}</span>
            </label>
          </div>
        </div>
      </div>

      <div class="sv-btn-row">
        <button class="sv-btn ghost sv-btn-fill" :disabled="loading" @click="addDraft">
          新增连接
        </button>
        <button class="sv-btn primary sv-btn-fill" :disabled="saving || loading" @click="save">
          {{ saving ? '保存中...' : '保存连接配置' }}
        </button>
      </div>

      <!-- 任务模式默认连接(TM-SET-1):仅任务模式显示。回退链:
           逐任务/节点显式连接 → 本项 → 默认连接;引用失效时运行期软回退默认连接。 -->
      <div v-if="isTaskMode" class="sv-separator">
        <div class="sv-field-label sub">任务模式默认连接</div>
        <div class="sv-inp-row">
          <label class="sv-inp-tag">默认连接</label>
          <select
            :value="taskDefaultConnectionId"
            class="sv-select"
            title="任务未单独指定连接时的缺省;逐任务与流程节点的显式选择仍优先。引用失效(被删/停用)时任务回退默认连接"
            @change="onTaskDefaultChange"
          >
            <option value="">跟随默认连接</option>
            <option
              v-for="d in drafts"
              :key="d.id || d.name"
              :value="d.id"
              :disabled="!d.id || !d.enabled"
            >
              {{ d.name || '(未命名)' }}{{ d.model ? `｜${d.model}` : '' }}{{ d.enabled ? '' : '(已停用)' }}
            </option>
          </select>
        </div>
        <p class="sv-note">
          任务模式未单独指定连接时使用(聊天的角色扮演不受影响)。新增的连接先「保存连接配置」后才能被选为任务默认。
        </p>
        <p v-if="taskDefaultDangling" class="sv-note flow-tool-warn">
          当前任务默认连接已不存在或已停用,运行任务时将回退默认连接;请改选或清除。
        </p>
      </div>

      <p class="sv-note">
        连接信息全局共享:角色扮演使用「默认」那套连接。
        <template v-if="isTaskMode">
          任务模式未单独选择时也走它,可用上方「任务模式默认连接」改指其它连接(逐任务 / 流程节点的显式选择仍优先)。
        </template>
        密钥只写不回显,留空表示不修改已有密钥。
      </p>
    </div>
    <div v-if="feedback" class="sv-feedback" :class="feedback.kind">{{ feedback.text }}</div>
  </div>
</template>

<style scoped>
/* 2026-09-27 UIFIX-1:`.sv-conn-card` 此前在样式表里**零定义**(模板引用 1 次、CSS 零命中),
   卡片内 5 行 .sv-inp-row 因此零间距,相邻两行的 3px 黑边框直接相接(实测行间距 0/0/1/0px)
   → 五行连成一整块「瓷砖」。这里给卡片真正的边界与呼吸:细边 + 白底 + 12px 内距,
   与外层 .sv-field 的 3px 印刷黑外框构成「外重内轻」两级层次。 */
.sv-conn-card {
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: var(--space-3);
  border: var(--bw-thin) solid var(--sv-line-strong);
  background: var(--sv-white);
}

/* 卡头:默认单选并进标签格后,名称输入与下方字段列同起点(.sv-inp-tag 的 72px + 10px gap)。
   显式给 radio 尺寸:标签字号是 10px,原生控件不写死尺寸会被一起缩小。 */
.sv-conn-def {
  display: inline-flex;
  align-items: center;
  gap: 6px;
}
.sv-conn-def input[type='radio'] {
  flex: none;
  width: 13px;
  height: 13px;
  margin: 0;
}
/* 「启用」不是字段标签而是开关说明,卡头里不再占标签列宽(默认格仍保持 72px 以维持列对齐) */
.sv-conn-head .sv-inp-tag:not(.sv-conn-def) {
  min-width: 0;
}

/* 模型能力勾选组(2026-10-02 视觉能力包 D1):多行紧排;预留项降饱和并虚线提示,
   与功能项(视觉输入 / 大图自动拆分)在观感上可区分。 */
.sv-conn-caps {
  align-items: flex-start;
}
.sv-conn-cap-list {
  display: flex;
  flex-wrap: wrap;
  gap: 6px 16px;
  padding-top: 3px;
}
.sv-conn-cap {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  cursor: help;
}
.sv-conn-cap.reserved {
  opacity: 0.62;
}
.sv-conn-cap.reserved span {
  border-bottom: 1px dashed var(--sv-line-strong);
}

/* 按钮行:全局 `.sv-btn-fill` 让两个按钮各占半宽(本卡实测 342.3×44.1px 的黑白大方块)。
   本组件内改为**内容宽 + 整行靠右**的动作条 —— 只在本卡生效,`.sv-btn-fill` 的其余 12 个
   使用点(API 设置/模型与生成/MCP 等)不受影响;两个都留内容宽时若靠左会空出 480px 空洞。 */
.sv-conn-section .sv-btn-row {
  justify-content: flex-end;
}
.sv-conn-section .sv-btn-row .sv-btn-fill {
  flex: 0 0 auto;
}
</style>
