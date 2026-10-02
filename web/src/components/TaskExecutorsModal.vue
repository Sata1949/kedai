<script setup lang="ts">
// 任务执行者管理模态框(2026-09-17:执行者与角色扮演角色卡解耦)。
//
// 为什么需要独立界面:此前任务创建下拉直接列角色扮演角色卡,用户无法为任务单独
// 描述「这个执行者该干什么」,两个模式的角色资产也互相污染。本面板提供执行者库的
// 增删改:每个执行者只有 名称 + 执行者指令(+ 可选温度)。
//
// 交互:左侧列表选择 → 右侧表单编辑(本地草稿)→ 逐条保存/删除(与后端 upsert 语义一致)。
// 保存后 store 直接替换列表(后端响应带最新列表),任务创建下拉即时可见。
import { computed, onMounted, ref } from 'vue';
import { storeToRefs } from 'pinia';
import { useAppStore } from '../store';
import type { TaskExecutor } from '../api';

const store = useAppStore();
const { executors, executorsError, executorsLoading } = storeToRefs(store);

const close = (): void => {
  store.taskExecutorsOpen = false;
};

/** 编辑中的草稿(id 为空 = 新建未落库) */
interface Draft {
  id: string;
  name: string;
  instruction: string;
  temperature: string;
}

const draft = ref<Draft>(emptyDraft());
const saving = ref(false);
const deleting = ref(false);
const msg = ref<{ kind: 'ok' | 'err'; text: string } | null>(null);

function emptyDraft(): Draft {
  return { id: '', name: '', instruction: '', temperature: '' };
}

const isNew = computed(() => draft.value.id === '');
/** 草稿是否有未保存改动(与选中执行者逐字段比对;新建时看是否有输入) */
const dirty = computed(() => {
  const d = draft.value;
  if (isNew.value) return Boolean(d.name.trim() || d.instruction.trim() || d.temperature.trim());
  const origin = executors.value.find((e) => e.id === d.id);
  if (!origin) return true;
  return (
    d.name !== origin.name ||
    d.instruction !== origin.instruction ||
    (d.temperature === '' ? null : Number(d.temperature)) !== (origin.temperature ?? null)
  );
});

/** 选中某执行者进入编辑态(丢弃未保存草稿) */
function selectExecutor(e: TaskExecutor): void {
  draft.value = {
    id: e.id,
    name: e.name,
    instruction: e.instruction,
    temperature: e.temperature == null ? '' : String(e.temperature),
  };
  msg.value = null;
}

function startNew(): void {
  draft.value = emptyDraft();
  msg.value = null;
}

async function save(): Promise<void> {
  if (saving.value) return;
  const name = draft.value.name.trim();
  const instruction = draft.value.instruction.trim();
  if (!name) {
    msg.value = { kind: 'err', text: '请填写执行者名称' };
    return;
  }
  if (!instruction) {
    msg.value = { kind: 'err', text: '请填写执行者指令:描述该执行者的职责与工作方式' };
    return;
  }
  // 温度:留空 = 用任务默认;填了就必须是合法数值
  let temperature: number | null = null;
  const tRaw = draft.value.temperature.trim();
  if (tRaw) {
    const t = Number(tRaw);
    if (!Number.isFinite(t) || t < 0 || t > 2) {
      msg.value = { kind: 'err', text: '温度需在 0-2 之间(留空表示沿用任务模式默认温度)' };
      return;
    }
    temperature = t;
  }

  saving.value = true;
  msg.value = null;
  try {
    const saved = await store.saveExecutor({
      id: draft.value.id || undefined,
      name,
      instruction,
      temperature,
    });
    // 保存后切到落库结果(新建时补齐后端分配的 id 与时间戳)
    draft.value = {
      id: saved.id,
      name: saved.name,
      instruction: saved.instruction,
      temperature: saved.temperature == null ? '' : String(saved.temperature),
    };
    msg.value = { kind: 'ok', text: '已保存执行者;任务创建下拉即时可见' };
    setTimeout(() => (msg.value = null), 2500);
  } catch (e) {
    msg.value = { kind: 'err', text: `保存失败:${(e as Error).message}` };
  } finally {
    saving.value = false;
  }
}

async function remove(): Promise<void> {
  if (isNew.value || deleting.value) return;
  const name = draft.value.name || '该执行者';
  if (!confirm(`确定删除执行者「${name}」?已引用它的任务不受影响,执行时会回退为通用执行者。`)) {
    return;
  }
  deleting.value = true;
  msg.value = null;
  try {
    await store.deleteExecutor(draft.value.id);
    startNew();
    msg.value = { kind: 'ok', text: '已删除执行者' };
    setTimeout(() => (msg.value = null), 2500);
  } catch (e) {
    msg.value = { kind: 'err', text: `删除失败:${(e as Error).message}` };
  } finally {
    deleting.value = false;
  }
}

onMounted(() => {
  void store.loadExecutors();
});
</script>

<template>
  <div class="sv-modal-mask" @click.self="close">
    <div class="sv-modal wb-modal">
      <div class="sv-modal-head">
        <h2 class="flex items-center gap-2">
          <span class="sv-supreme pink" /> 任务执行者
        </h2>
        <button class="sv-btn ghost sv-btn-square" title="关闭" @click="close">✕</button>
      </div>

      <div class="sv-modal-body">
        <div class="sv-field">
          <div class="sv-field-label"><span class="sv-supreme blue" /> 说明</div>
          <p class="sv-note" style="margin: 0; line-height: 1.8">
            执行者是<b>任务模式的独立角色</b>,与角色扮演的角色卡互不影响。任务创建时选择一个执行者,
            其<b>执行者指令</b>会作为「执行者职责」注入任务执行的系统提示词。<br />
            不选执行者(或用「通用执行者」)= 不注入任何身份描述,为默认行为。<br />
            删除执行者不会影响已引用它的任务:执行时查不到即回退通用执行者。
          </p>
        </div>

        <!-- 列表 + 编辑(左选右改) -->
        <div class="sv-field">
          <div class="sv-field-label">
            <span class="sv-supreme yellow" /> 执行者库
            <span class="sv-wb-count">{{ executors.length }} 个</span>
          </div>

          <div v-if="executorsError" class="sv-feedback err" style="margin: var(--space-2) 0">{{ executorsError }}</div>
          <div v-else-if="executorsLoading && executors.length === 0" class="sv-note">加载中…</div>
          <div v-else class="exec-layout">
            <!-- 左:执行者列表 -->
            <div class="exec-list">
              <button class="exec-item" :class="{ active: isNew }" @click="startNew">
                <span class="exec-item-name">＋ 新建执行者</span>
                <span class="exec-item-desc">不使用已有配置</span>
              </button>
              <button
                v-for="e in executors"
                :key="e.id"
                class="exec-item"
                :class="{ active: e.id === draft.id }"
                @click="selectExecutor(e)"
              >
                <span class="exec-item-name" :title="e.name">{{ e.name }}</span>
                <span class="exec-item-desc">{{ e.instruction }}</span>
              </button>
              <p v-if="executors.length === 0" class="sv-note" style="padding: var(--space-2) var(--space-1)">
                还没有执行者,点上方「＋ 新建执行者」创建第一个。
              </p>
            </div>

            <!-- 右:编辑表单 -->
            <div class="exec-form">
              <div class="sv-inp-row">
                <label class="sv-inp-tag">名称</label>
                <input v-model="draft.name" type="text" class="sv-input" placeholder="如:审稿员 / 数据分析师" spellcheck="false" />
              </div>
              <div class="sv-inp-row" style="align-items: flex-start">
                <label class="sv-inp-tag" style="padding-top: var(--space-2)">执行者指令</label>
                <textarea
                  v-model="draft.instruction"
                  rows="10"
                  class="sv-input"
                  placeholder="描述这个执行者的职责与工作方式,例如:你是严苛的技术审稿人,逐条指出事实错误与逻辑漏洞,并给出可执行的修改建议。"
                  spellcheck="false"
                />
              </div>
              <p class="sv-note">
                该文本整段注入任务执行提示词,会被标记为外部来源(untrusted)——
                <b>不要在此填写安全策略或试图改变工具授权规则</b>,那些由应用层控制。
              </p>
              <div class="sv-inp-row">
                <label class="sv-inp-tag">温度</label>
                <input
                  v-model="draft.temperature"
                  type="text"
                  class="sv-input"
                  placeholder="留空 = 沿用任务模式默认温度(0-2)"
                  spellcheck="false"
                />
              </div>

              <div class="sv-wb-edit-foot">
                <button
                  v-if="!isNew"
                  class="sv-btn danger sv-btn-sm"
                  :disabled="deleting || saving"
                  @click="remove"
                >
                  {{ deleting ? '删除中…' : '删除' }}
                </button>
                <span v-if="dirty" class="sv-note" style="margin-left: auto">有未保存改动</span>
                <button
                  class="sv-btn primary sv-btn-sm"
                  style="margin-left: auto"
                  :disabled="saving || (!dirty && !isNew)"
                  @click="save"
                >
                  {{ saving ? '保存中…' : (isNew ? '创建执行者' : '保存修改') }}
                </button>
              </div>
              <div v-if="msg" class="sv-feedback" :class="msg.kind">{{ msg.text }}</div>
            </div>
          </div>
        </div>
      </div>

      <div class="sv-modal-foot">
        <button class="sv-btn primary" @click="close">完成</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* 左选右改布局:窄屏(移动端)改上下堆叠,避免两栏被压成不可读宽度。
   样式全部 scoped(MAINTENANCE D-5:新增组件样式禁止进 style.css)。 */
.exec-layout {
  display: grid;
  grid-template-columns: minmax(180px, 240px) minmax(0, 1fr);
  gap: var(--space-3);
  margin-top: var(--space-2);
}
@media (max-width: 640px) {
  .exec-layout { grid-template-columns: minmax(0, 1fr); }
}

.exec-list {
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
  max-height: 380px;
  overflow-y: auto;
  border: var(--bw-thin) solid var(--sv-line);
  padding: var(--space-1);
}

.exec-item {
  display: flex;
  flex-direction: column;
  gap: 2px;
  align-items: flex-start;
  width: 100%;
  padding: var(--space-1-5) var(--space-2);
  border: var(--bw-thin) solid transparent;
  background: transparent;
  text-align: left;
  cursor: pointer;
}
.exec-item:hover { background: var(--sv-white); border-color: var(--sv-pink); }
.exec-item.active { background: var(--sv-ink); border-color: var(--sv-ink); color: var(--sv-white); }

/* UIP-10:长名截断(原无截断,长执行者名把卡片顶宽);全文走 title */
.exec-item-name { font-size: 12px; font-weight: 700; max-width: 100%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
/* 指令预览截断成一行:列表只做识别,全文在右侧表单看 */
.exec-item-desc {
  font-size: 11px;
  color: var(--sv-ink-faint);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  max-width: 100%;
}
.exec-item.active .exec-item-desc { color: var(--sv-on-ink-soft); }

.exec-form { display: flex; flex-direction: column; gap: var(--space-1-5); }
</style>
