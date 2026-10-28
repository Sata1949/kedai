<script setup lang="ts">
// 设置区:用户设定(USERPERSONA-1;仅角色扮演模式生效)。
// 状态在模块级 composable:显示层只读 config,本区编辑 draft,保存时才提交。
import { onMounted } from 'vue';
import { useUserPersona } from '../../composables/useUserPersona';

const props = withDefaults(defineProps<{
  show?: boolean;
}>(), {
  show: true,
});

const { draft, loading, saving, error, load, save, resetDraft } = useUserPersona();

onMounted(() => {
  void load();
});
</script>

<template>
  <div v-show="props.show" class="sv-field">
    <div class="sv-field-label"><span class="sv-supreme blue" /> 用户设定</div>
    <div class="sv-stack">
      <p class="sv-note">
        仅角色扮演模式生效;任务模式不注入、不替换称呼。启用后:
        <code v-pre>{{user}}</code> / <code v-pre>{{user_name}}</code> 替换为称呼,
        系统提示词追加【用户角色】段。
      </p>

      <div class="sv-inp-row">
        <label class="sv-inp-tag">启用</label>
        <button
          class="sv-btn ghost"
          :class="{ 'sv-btn-on': draft.enabled }"
          @click="draft.enabled = !draft.enabled"
        >
          {{ draft.enabled ? '已启用' : '已关闭' }}
        </button>
        <span class="sv-note">留空称呼时按「用户 / 你」处理</span>
      </div>

      <div class="sv-inp-row">
        <label class="sv-inp-tag">称呼</label>
        <input
          v-model="draft.name"
          type="text"
          maxlength="64"
          class="sv-input"
          placeholder="例如:林"
          :disabled="!draft.enabled"
          spellcheck="false"
        />
      </div>

      <div class="sv-inp-row" style="align-items: flex-start">
        <label class="sv-inp-tag">角色设定</label>
        <textarea
          v-model="draft.description"
          rows="6"
          maxlength="4000"
          class="sv-input"
          placeholder="身份、外貌、性格、与角色的关系、已知信息……"
          :disabled="!draft.enabled"
          spellcheck="false"
        />
      </div>

      <p class="sv-note">
        设定会经不可信素材边界包裹后注入;请不要在其中写系统指令或工具调用要求。
        任务模式保持「用户」,切换模式不会删除本配置,只是不生效。
      </p>

      <div class="sv-btn-row">
        <button class="sv-btn primary sv-btn-fill" :disabled="saving || loading" @click="save">
          {{ saving ? '保存中...' : '保存用户设定' }}
        </button>
        <button class="sv-btn ghost" :disabled="saving || loading" @click="resetDraft">清空</button>
        <div v-if="error" class="sv-feedback err sv-feedback-flex">{{ error }}</div>
        <div v-else-if="saving" class="sv-note">正在保存…</div>
      </div>
    </div>
  </div>
</template>
