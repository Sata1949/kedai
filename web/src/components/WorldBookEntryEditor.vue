<script setup lang="ts">
// 世界书条目编辑器（C-2A，2026-10-25；替代 WorldBooksModal 里逐字重复的两段 95 行）。
//
// 契约：本组件只渲染单条条目，**不改 prop 对象**；所有字段变更以 patch(key, value)
// 上抛，由父组件写回自己的草稿数组（角色卡内嵌 / 独立世界书两套数据源共用）。
// move/remove 也由父组件执行——组内排序与差集保存逻辑仍只有一份。
import { computed } from 'vue';
import type { WorldBookEntry } from '../api';

const props = defineProps<{
  entry: WorldBookEntry;
  index: number;
  total: number;
  /** 关键词占位文案(两套编辑器语义不同,由父级传入) */
  keysPlaceholder: string;
  /** 正则占位文案 */
  regexPlaceholder: string;
}>();

const emit = defineEmits<{
  /** 字段级补丁：父组件写回草稿数组 */
  patch: [key: keyof WorldBookEntry, value: unknown];
  /** 常驻/触发切换：父组件同时联动注入位置(常驻→3 / 触发→1) */
  toggleConstant: [];
  move: [delta: -1 | 1];
  remove: [];
}>();

const POSITION_OPTIONS = [
  { value: 0, label: '0(before_char)' },
  { value: 1, label: '1(激发/追加用户消息)' },
  { value: 2, label: '2(normal)' },
  { value: 3, label: '3(常态/系统提示词)' },
  { value: 4, label: '4(after_char)' },
];

const ROLE_OPTIONS = [
  { value: null, label: '自动' },
  { value: 'system', label: '系统提示词' },
  { value: 'user', label: '用户' },
  { value: 'assistant', label: '角色' },
] as const;

/** 关键词输入(逗号分隔) ↔ keys 数组 */
function keysToText(keys: string[]): string {
  return (keys ?? []).join(', ');
}
function textToKeys(text: string): string[] {
  return text
    .split(/[,，、\n]/)
    .map((s) => s.trim())
    .filter(Boolean);
}

function textValue(ev: Event): string {
  return (ev.target as HTMLInputElement).value;
}

/** 与 Vue v-model.number 同口径：空串保留空串，可解析则转 Number */
function numberValue(ev: Event): number | '' {
  const raw = (ev.target as HTMLInputElement).value;
  return raw === '' ? '' : Number(raw);
}

function patchText(key: keyof WorldBookEntry, ev: Event): void {
  emit('patch', key, textValue(ev));
}
function patchNumber(key: keyof WorldBookEntry, ev: Event): void {
  emit('patch', key, numberValue(ev));
}
function patchKeys(key: 'keys' | 'keys_secondary', ev: Event): void {
  emit('patch', key, textToKeys(textValue(ev)));
}
function patchPosition(ev: Event): void {
  emit('patch', 'position', Number((ev.target as HTMLSelectElement).value));
}
function patchRole(ev: Event): void {
  const v = (ev.target as HTMLSelectElement).value;
  emit('patch', 'role', v === '' ? null : (v as WorldBookEntry['role']));
}

const canMoveUp = computed(() => props.index > 0);
const canMoveDown = computed(() => props.index < props.total - 1);
</script>

<template>
  <div class="sv-wb-entry">
    <div class="sv-wb-entry-head">
      <input
        :value="entry.comment"
        type="text"
        class="sv-input sv-wb-comment"
        placeholder="条目名称"
        spellcheck="false"
        @input="patchText('comment', $event)"
      />
      <select
        class="sv-wb-select"
        title="注入位置(酒馆 position):常驻→位置3(并入系统提示词),激发→位置1(追加最新用户消息尾部)"
        @change="patchPosition"
      >
        <option
          v-for="opt in POSITION_OPTIONS"
          :key="String(opt.value)"
          :value="opt.value"
          :selected="entry.position === opt.value"
        >{{ opt.label }}</option>
      </select>
      <select
        class="sv-wb-select"
        title="注入角色:自动 = 常驻→系统提示词,激发→用户"
        @change="patchRole"
      >
        <option
          v-for="opt in ROLE_OPTIONS"
          :key="String(opt.value)"
          :value="opt.value ?? ''"
          :selected="(entry.role ?? null) === opt.value"
        >{{ opt.label }}</option>
      </select>
      <button
        class="sv-btn ghost sv-btn-sm"
        :class="{ 'sv-btn-on': entry.constant }"
        title="常驻:始终注入,位置3(并入系统提示词);触发:按关键词/正则命中,位置1(追加最新用户消息尾部)"
        @click="emit('toggleConstant')"
      >
        {{ entry.constant ? '常驻' : '触发' }}
      </button>
      <button
        class="sv-btn ghost sv-btn-sm"
        :class="{ 'sv-btn-on': entry.enabled }"
        title="激活:该条目参与注入的总开关(停用 = 完全不注入)"
        @click="emit('patch', 'enabled', !entry.enabled)"
      >
        {{ entry.enabled ? '激活' : '停用' }}
      </button>
      <button
        class="sv-btn ghost sv-btn-sm"
        :class="{ 'sv-btn-on': entry.case_sensitive }"
        title="大小写敏感(酒馆 caseSensitive)"
        @click="emit('patch', 'case_sensitive', !entry.case_sensitive)"
      >
        {{ entry.case_sensitive ? 'Aa敏感' : 'Aa不敏' }}
      </button>
      <button class="sv-btn ghost sv-btn-sm" title="上移(组内排序)" :disabled="!canMoveUp" @click="emit('move', -1)">↑</button>
      <button class="sv-btn ghost sv-btn-sm" title="下移(组内排序)" :disabled="!canMoveDown" @click="emit('move', 1)">↓</button>
      <button class="sv-btn danger sv-btn-sm" title="删除条目" @click="emit('remove')">✕</button>
    </div>
    <div class="sv-wb-entry-fields">
      <label class="sv-wb-field-tag">关键词</label>
      <input
        :value="keysToText(entry.keys)"
        type="text"
        class="sv-input"
        :placeholder="keysPlaceholder"
        spellcheck="false"
        @input="patchKeys('keys', $event)"
      />
    </div>
    <div class="sv-wb-entry-fields">
      <label class="sv-wb-field-tag">副关键词</label>
      <input
        :value="keysToText(entry.keys_secondary ?? [])"
        type="text"
        class="sv-input"
        placeholder="可选;与主关键词并列,命中其一即注入(酒馆 keysecondary)"
        spellcheck="false"
        @input="patchKeys('keys_secondary', $event)"
      />
    </div>
    <div class="sv-wb-entry-fields">
      <label class="sv-wb-field-tag">正则</label>
      <input
        :value="entry.regex ?? ''"
        type="text"
        class="sv-input"
        :placeholder="regexPlaceholder"
        spellcheck="false"
        @input="patchText('regex', $event)"
      />
      <button
        class="sv-btn ghost sv-btn-sm"
        :class="{ 'sv-btn-on': entry.use_regex }"
        title="启用正则匹配"
        :disabled="!entry.regex"
        @click="emit('patch', 'use_regex', !entry.use_regex)"
      >
        {{ entry.use_regex ? '正则开' : '正则关' }}
      </button>
    </div>
    <div class="sv-wb-entry-fields">
      <label class="sv-wb-field-tag">插入深度</label>
      <input
        :value="entry.depth"
        type="number"
        min="0"
        class="sv-input sv-wb-num"
        title="插入深度(酒馆 depth):position=atDepth 时从末尾算注入位置;条目未声明扫描窗口时兼作窗口兜底"
        placeholder="4"
        @input="patchNumber('depth', $event)"
      />
      <label class="sv-wb-field-tag">扫描窗口</label>
      <input
        :value="entry.scan_depth ?? ''"
        type="number"
        min="0"
        class="sv-input sv-wb-num"
        title="关键词扫描窗口(酒馆 scanDepth):关键词只扫最近 N 条消息(0 = 全部);留空 = 保持原值不改动"
        placeholder="未声明"
        @input="patchNumber('scan_depth', $event)"
      />
      <label class="sv-wb-field-tag">注入顺序</label>
      <input
        :value="entry.order"
        type="number"
        min="0"
        class="sv-input sv-wb-num"
        title="同位置内按此值从小到大注入(酒馆 order)"
        placeholder="100"
        @input="patchNumber('order', $event)"
      />
      <label class="sv-wb-field-tag">命中概率</label>
      <input
        :value="entry.probability"
        type="number"
        min="0"
        max="100"
        class="sv-input sv-wb-num"
        title="命中后按此百分比决定是否注入(酒馆 probability)"
        placeholder="100"
        @input="patchNumber('probability', $event)"
      />
      <button
        class="sv-btn ghost sv-btn-sm"
        :class="{ 'sv-btn-on': entry.use_probability }"
        title="启用命中概率(100 = 恒注入)"
        @click="emit('patch', 'use_probability', !entry.use_probability)"
      >
        {{ entry.use_probability ? '概率开' : '概率关' }}
      </button>
    </div>
    <div class="sv-wb-entry-fields">
      <label class="sv-wb-field-tag">粘性</label>
      <input
        :value="entry.sticky"
        type="number"
        min="0"
        class="sv-input sv-wb-num"
        title="酒馆 sticky 字段:命中后接下来 N 条消息持续注入;当前版本尚未参与注入判定(编辑值仅原样保留)"
        placeholder="0"
        @input="patchNumber('sticky', $event)"
      />
      <label class="sv-wb-field-tag">冷却</label>
      <input
        :value="entry.cooldown"
        type="number"
        min="0"
        class="sv-input sv-wb-num"
        title="酒馆 cooldown 字段:命中后 N 条消息内不再触发;当前版本尚未参与注入判定(编辑值仅原样保留)"
        placeholder="0"
        @input="patchNumber('cooldown', $event)"
      />
    </div>
    <textarea
      :value="entry.content"
      rows="6"
      class="sv-input sv-wb-entry-content"
      placeholder="注入内容(留空则该条不注入)"
      spellcheck="false"
      @input="patchText('content', $event)"
    />
  </div>
</template>
