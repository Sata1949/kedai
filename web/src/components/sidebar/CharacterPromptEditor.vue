<script setup lang="ts">
// 角色提示词 / 开场白编辑弹窗（C-2A，2026-10-25；原 Sidebar 776-844）。
// 草稿在组件内独立维护；保存仍走 store.updateCharacterPrompt（状态变更单一入口）。
import { onBeforeUnmount, ref, watch } from 'vue';
import { useAppStore } from '../../store';
import type { CharacterRecord } from '../../api';

const props = defineProps<{
  character: CharacterRecord;
}>();
const emit = defineEmits<{ close: [] }>();

const store = useAppStore();

interface PromptDraft {
  id: string;
  name: string;
  text: string;
  first_mes: string;
  alts: string[];
}

function makeDraft(c: CharacterRecord): PromptDraft {
  return {
    id: c.id,
    name: c.chara_name,
    text: c.description ?? '',
    first_mes: c.first_mes ?? '',
    alts: [...(c.alternate_greetings ?? [])],
  };
}

const draft = ref<PromptDraft>(makeDraft(props.character));
const savingPrompt = ref(false);
const promptMsg = ref('');
let closeTimer: ReturnType<typeof setTimeout> | null = null;

// 弹窗打开期间若父级换了角色（理论上不会，防御性同步）
watch(
  () => props.character,
  (c) => {
    draft.value = makeDraft(c);
    promptMsg.value = '';
  },
);

function addAlternateGreeting(): void {
  draft.value.alts.push('');
}

function removeAlternateGreeting(i: number): void {
  draft.value.alts.splice(i, 1);
}

async function savePrompt(): Promise<void> {
  const e = draft.value;
  if (savingPrompt.value) return;
  savingPrompt.value = true;
  promptMsg.value = '';
  try {
    await store.updateCharacterPrompt(e.id, {
      description: e.text,
      first_mes: e.first_mes,
      alternate_greetings: e.alts.map((a) => a.trim()).filter((a) => a.length > 0),
    });
    promptMsg.value = '已更新(开场白已自动识别并保存)';
    closeTimer = setTimeout(() => emit('close'), 400);
  } catch (err) {
    promptMsg.value = `保存失败:${(err as Error).message}`;
  } finally {
    savingPrompt.value = false;
  }
}

onBeforeUnmount(() => {
  if (closeTimer) clearTimeout(closeTimer);
});
</script>

<template>
  <Teleport to="body">
    <div v-if="draft" class="sv-modal-mask" @click.self="emit('close')">
      <div class="sv-modal pm-edit">
        <div class="sv-modal-head">
          <h2 class="flex items-center gap-2">
            <span class="sv-supreme pink" /> 编辑提示词
          </h2>
          <button class="sv-btn ghost sv-btn-square" @click="emit('close')">✕</button>
        </div>
        <div class="sv-modal-body">
          <div class="sv-field">
            <div class="sv-field-label"><span class="sv-supreme blue" /> {{ draft.name }}</div>
            <textarea
              v-model="draft.text"
              rows="16"
              class="sv-input sv-area xl"
              placeholder="角色提示词(角色设定)。留空表示无提示词,直接以助手身份回复。"
              spellcheck="false"
            ></textarea>
            <div class="sv-field-label sv-mt14">
              <span class="sv-supreme pink-deep" /> 开场白(First Message)
            </div>
            <textarea
              v-model="draft.first_mes"
              rows="5"
              class="sv-input sv-area md"
              placeholder="主开场:新建会话时作为角色的第一句话(自动发送)。留空表示无主开场。"
              spellcheck="false"
            ></textarea>
            <p class="sv-note sv-mt6">
              主开场之外可添加备用开场,新建会话时可在多个开场之间选择。
            </p>

            <!-- 备用开场列表 -->
            <div v-for="(g, i) in draft.alts" :key="i" class="sv-mt12">
              <div class="flex items-center gap-2 sv-mb6">
                <span class="sv-tag">备用 {{ i + 1 }}</span>
                <span class="sv-flex-1" />
                <button class="sv-btn ghost sv-btn-sm" @click="removeAlternateGreeting(i)">删除</button>
              </div>
              <textarea
                v-model="draft.alts[i]"
                rows="4"
                class="sv-input sv-area sm"
                placeholder="备用开场内容(留空保存时自动忽略)。"
                spellcheck="false"
              ></textarea>
            </div>

            <button class="sv-btn ghost sv-btn-sm sv-mt12" @click="addAlternateGreeting">
              ＋ 添加备用开场
            </button>
            <p class="sv-note sv-mt8">
              提示词作为角色的系统设定注入每次对话;开场白在新建会话时作为角色的第一句话(自动识别角色卡内嵌开场白)。
            </p>
          </div>
          <div v-if="promptMsg" class="sv-feedback" :class="promptMsg.includes('失败') ? 'err' : 'ok'">
            {{ promptMsg }}
          </div>
        </div>
        <div class="sv-modal-foot">
          <button class="sv-btn ghost sv-mr8" @click="emit('close')">取消</button>
          <button class="sv-btn primary" :disabled="savingPrompt" @click="savePrompt">
            {{ savingPrompt ? '保存中...' : '保存' }}
          </button>
        </div>
      </div>
    </div>
  </Teleport>
</template>
