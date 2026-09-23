<script setup lang="ts">
// 设置区:连接配置(多套 API 连接:新增 / 编辑 / 删除 / 设为默认 / 启停)。
// 状态由壳(SettingsModal)创建一次后经 prop 传入 —— 与 ApiSettingsSection 同范式。
import { onMounted } from 'vue';
import { CONNECTOR_TYPE_LABELS, CONNECTOR_TYPE_ORDER } from '../../api/labels';
import type { useConnectionProfiles } from '../../composables/useConnectionProfiles';

const props = withDefaults(defineProps<{
  /** useConnectionProfiles 的返回对象(壳共享实例) */
  state: ReturnType<typeof useConnectionProfiles>;
  /** 是否显示(embedded 模式按 activeSection 切换;standalone 恒 true) */
  show?: boolean;
}>(), {
  show: true,
});

const {
  drafts, activeIndex, loading, saving, feedback,
  load, addDraft, requestRemove, setActive, save,
} = props.state;

/** 未登记的类型原样展示(与 labels.ts 的「未登记原样」语义一致) */
const typeLabel = (t: string): string => CONNECTOR_TYPE_LABELS[t] ?? t;

onMounted(() => {
  void load();
});
</script>

<template>
  <div v-show="props.show" class="sv-field">
    <div class="sv-field-label"><span class="sv-supreme red" /> 连接配置</div>
    <div class="sv-stack">
      <div v-if="loading" class="sv-count">加载中...</div>
      <div v-else-if="!drafts.length" class="sv-count">暂无连接,点「新增连接」添加一条。</div>

      <div v-for="(d, i) in drafts" :key="d.id || 'new-' + i" class="sv-conn-card">
        <div class="sv-inp-row">
          <label class="sv-inp-tag" :for="`conn-default-${i}`">默认</label>
          <input
            :id="`conn-default-${i}`"
            type="radio"
            name="kedai-conn-default"
            :checked="activeIndex === i"
            :disabled="!d.enabled"
            :title="d.enabled ? '设为默认连接' : '已停用的连接不能作为默认连接'"
            @change="setActive(i)"
          />
          <input v-model="d.name" type="text" class="sv-input" placeholder="连接名称" spellcheck="false" />
          <label class="sv-inp-tag" :for="`conn-enabled-${i}`">启用</label>
          <input :id="`conn-enabled-${i}`" v-model="d.enabled" type="checkbox" />
          <button class="sv-btn ghost" @click="requestRemove(i)">删除</button>
        </div>

        <div class="sv-inp-row">
          <label class="sv-inp-tag">类型</label>
          <select v-model="d.connector_type" class="sv-select">
            <option v-for="t in CONNECTOR_TYPE_ORDER" :key="t" :value="t">{{ typeLabel(t) }}</option>
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
            :placeholder="d.has_api_key ? `已配置 ${d.api_key_masked} · 留空保持现有` : '粘贴 API Key'"
            autocomplete="off"
            spellcheck="false"
          />
        </div>

        <div class="sv-inp-row">
          <label class="sv-inp-tag">MODEL</label>
          <input v-model="d.model" type="text" class="sv-input" placeholder="模型名" spellcheck="false" />
        </div>
      </div>

      <div class="sv-btn-row">
        <button class="sv-btn ghost sv-btn-fill" :disabled="loading" @click="addDraft">新增连接</button>
        <button class="sv-btn primary sv-btn-fill" :disabled="saving || loading" @click="save">
          {{ saving ? '保存中...' : '保存连接配置' }}
        </button>
      </div>
      <p class="sv-note">
        连接信息全局共享:聊天与任务当前都使用「默认」那套连接;逐节点 / 逐任务选用连接在后续批次。
        密钥只写不回显,留空表示不修改已有密钥。
      </p>
    </div>
    <div v-if="feedback" class="sv-feedback" :class="feedback.kind">{{ feedback.text }}</div>
  </div>
</template>
