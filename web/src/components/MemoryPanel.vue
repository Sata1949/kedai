<script setup lang="ts">
// 记忆库分区(优化面板):当前角色的跨会话记忆列表 + 检索/筛选 + 蒸馏当前会话 + 手动补录。
// 数据来自 /api/memory 与 /api/memory/search(api/memory.ts);交互逻辑在
// useMemoryPanel composable,展示与反馈文案纯函数在 memoryPanel.ts。
// 操作:顶部搜索框(250ms 防抖,空查询回退全量列表)+ kind 筛选(纯本地)+
// 行内编辑 content(PATCH)、删除(两段式确认)、注入开关(selected)、
// 置顶开关(pinned)、「清理已归档」(POST /api/memory/prune,两段式确认)、
// 「蒸馏当前会话」(未开启时 400 带去设置指引)。
// onServerPrefetch:服务端渲染测试通道(renderToString 会等待预取完成)。
import { onMounted, onServerPrefetch, ref } from 'vue';
import { useMemoryPanel } from '../composables/useMemoryPanel';
import {
  highlightSegments,
  KIND_FILTERS,
  type KindFilter,
  type MemoryRow,
} from './memoryPanel';

const props = defineProps<{ characterId?: string | null; sessionId?: string | null }>();

const panel = useMemoryPanel({
  characterId: () => props.characterId,
  sessionId: () => props.sessionId,
});
const {
  loading, error, shownRows, query, kindFilter, searching,
  distilling, distillMsg,
  actionMsg, newContent, adding,
  editingId, editContent, savingEdit,
  pendingDelete, deleting,
  pendingPrune, pruning,
  load, searchNow, setKindFilter, distillNow, addNow,
  startEdit, cancelEdit, saveEdit, toggleSelected, togglePinned,
  requestDelete, cancelDelete, confirmDelete,
  requestPrune, cancelPrune, pruneNow,
} = panel;

/** 模板事件包装:取 checkbox 勾选态(composable 保持纯参数可测) */
function onToggle(row: MemoryRow, ev: Event): void {
  void toggleSelected(row, (ev.target as HTMLInputElement).checked);
}

/** 模板事件包装:kind 筛选下拉值转 KindFilter */
function onKindChange(ev: Event): void {
  setKindFilter((ev.target as HTMLSelectElement).value as KindFilter);
}

/** 内容高亮分段(搜索命中标 <mark>,无查询时单段) */
function segments(row: MemoryRow) {
  return highlightSegments(row.content, query.value);
}

/** UIP-15:退场行钉在原位(absolute + 原宽),脱离 flex 流让剩余行立即补位并走 sv-list-move */
function pinLeavingRow(el: Element): void {
  const e = el as HTMLElement;
  e.style.position = 'absolute';
  e.style.top = `${e.offsetTop}px`;
  e.style.left = `${e.offsetLeft}px`;
  e.style.width = `${e.offsetWidth}px`;
}

/** UIP-15:入场动画结束后置 settled——Vue move 检测要求 transition 时长 > animation 时长,
    行自带 sv-list-in(0.15s+错峰延迟)会把类型判成 animation 致 move 整体跳过;
    置 settled 同时避免 DOM 位移时入场动画重播(位移重插会重启 CSS 动画) */
const settledIds = ref<Set<number>>(new Set());
function onRowSettled(row: MemoryRow, ev: AnimationEvent): void {
  if (ev.target !== ev.currentTarget || settledIds.value.has(row.id)) return;
  const next = new Set(settledIds.value);
  next.add(row.id);
  settledIds.value = next;
}

onMounted(load);
onServerPrefetch(load);
</script>

<template>
  <div class="sv-field">
    <div class="sv-field-label"><span class="sv-supreme pink-deep" /> 记忆库</div>
    <p class="sv-note" style="margin: 0 0 var(--space-2); line-height: 1.8">
      当前角色的跨会话记忆:由会话蒸馏、工具写入或手动补录;勾选「注入」的条目按
      使用热度注入提示词(条数上限见 设置 → 生成参数)。置顶条目优先注入。
    </p>

    <template v-if="characterId">
      <!-- 蒸馏 + 刷新 -->
      <div style="display: flex; gap: var(--space-2); align-items: center; margin-bottom: var(--space-2)">
        <button
          class="sv-btn primary sv-btn-sm"
          :disabled="distilling || !sessionId"
          :title="sessionId ? '把当前会话蒸馏为角色记忆' : '当前无会话,无法蒸馏'"
          @click="distillNow"
        >
          {{ distilling ? '蒸馏中…' : '蒸馏当前会话' }}
        </button>
        <button class="sv-btn ghost sv-btn-sm" :disabled="loading" @click="load">
          {{ loading ? '加载中…' : '刷新' }}
        </button>
      </div>
      <div v-if="distillMsg" class="sv-feedback" :class="distillMsg.kind" style="margin-bottom: var(--space-2)">
        {{ distillMsg.text }}
      </div>
      <div v-if="error" class="sv-feedback err" style="margin-bottom: var(--space-2)">{{ error }}</div>

      <!-- 搜索 + kind 筛选 + 清理已归档 -->
      <div class="memory-toolbar">
        <input
          v-model="query"
          class="sv-input memory-search"
          type="search"
          placeholder="搜索记忆内容(FTS5 全文检索)…"
          @keydown.enter.prevent="searchNow"
        />
        <select
          class="sv-select memory-kind-select"
          :value="kindFilter"
          title="按来源类型筛选(本地过滤)"
          @change="onKindChange"
        >
          <option v-for="k in KIND_FILTERS" :key="k.value" :value="k.value">{{ k.label }}</option>
        </select>
        <template v-if="pendingPrune">
          <span class="sv-script-meta" style="font-size: 11px">硬删除全部已归档记忆?</span>
          <button class="sv-btn ghost sv-btn-sm memory-danger" :disabled="pruning" @click="pruneNow">
            {{ pruning ? '清理中…' : '确认清理' }}
          </button>
          <button class="sv-btn ghost sv-btn-sm" @click="cancelPrune">取消</button>
        </template>
        <button
          v-else
          class="sv-btn ghost sv-btn-sm memory-danger"
          title="硬删除该角色所有未勾选注入(已归档)的记忆,不可恢复"
          @click="requestPrune"
        >
          清理已归档
        </button>
      </div>

      <!-- 手动补录 -->
      <div style="display: flex; gap: var(--space-2); align-items: center; margin-bottom: var(--space-2)">
        <input
          v-model="newContent"
          class="sv-input"
          style="flex: 1"
          placeholder="补录一条跨会话记忆(手动)"
          @keydown.enter="addNow"
        />
        <button class="sv-btn ghost sv-btn-sm" :disabled="adding" @click="addNow">
          {{ adding ? '添加中…' : '添加' }}
        </button>
      </div>
      <div v-if="actionMsg" class="sv-feedback" :class="actionMsg.kind" style="margin-bottom: var(--space-2)">
        {{ actionMsg.text }}
      </div>

      <!-- 记忆列表(置顶优先,其次 id 降序;按 kind/query 本地筛选)。UIP-15:删除/置顶重排走 sv-list 过渡 -->
      <TransitionGroup v-if="shownRows.length" name="sv-list" tag="div" class="memory-list" @before-leave="pinLeavingRow">
        <div
          v-for="row in shownRows"
          :key="row.id"
          class="memory-row"
          :class="{ 'memory-pinned': row.pinned, 'memory-row-settled': settledIds.has(row.id) }"
          @animationend="onRowSettled(row, $event)"
        >
          <div class="memory-row-head">
            <span v-if="row.pinned" class="memory-pin-tag" title="置顶:优先注入">置顶</span>
            <span class="kind-tag" :class="row.kindClass">{{ row.kindLabel }}</span>
            <span class="sv-script-meta" style="font-size: 11px">
              使用 {{ row.usage }} 次 · {{ row.lastUsage }}
            </span>
            <label class="memory-sel" title="勾选后按使用热度参与提示词注入">
              <input type="checkbox" :checked="row.selected" @change="onToggle(row, $event)" /> 注入
            </label>
          </div>

          <div v-if="editingId === row.id" class="memory-edit">
            <input v-model="editContent" class="sv-input" style="flex: 1" placeholder="记忆内容" />
            <button class="sv-btn ghost sv-btn-sm" :disabled="savingEdit" @click="saveEdit">
              {{ savingEdit ? '保存中…' : '保存' }}
            </button>
            <button class="sv-btn ghost sv-btn-sm" @click="cancelEdit">取消</button>
          </div>
          <p v-else class="memory-content">
            <template v-for="(seg, i) in segments(row)" :key="i">
              <mark v-if="seg.hit" class="memory-hit">{{ seg.text }}</mark>
              <template v-else>{{ seg.text }}</template>
            </template>
          </p>

          <div class="memory-ops">
            <template v-if="pendingDelete === row.id">
              <span class="sv-script-meta" style="font-size: 11px">确认删除该记忆?</span>
              <button class="sv-btn ghost sv-btn-sm memory-danger" :disabled="deleting" @click="confirmDelete">
                {{ deleting ? '删除中…' : '确认删除' }}
              </button>
              <button class="sv-btn ghost sv-btn-sm" @click="cancelDelete">取消</button>
            </template>
            <template v-else>
              <button
                class="sv-btn ghost sv-btn-sm"
                :title="row.pinned ? '取消置顶' : '置顶(优先注入)'"
                @click="togglePinned(row)"
              >
                {{ row.pinned ? '取消置顶' : '置顶' }}
              </button>
              <button class="sv-btn ghost sv-btn-sm" @click="startEdit(row)">编辑</button>
              <button class="sv-btn ghost sv-btn-sm memory-danger" @click="requestDelete(row)">删除</button>
            </template>
          </div>
        </div>
      </TransitionGroup>
      <p v-else-if="!error && !loading && !searching" class="sv-script-meta" style="font-size: 11px">
        {{ query.trim() || kindFilter !== 'all' ? '没有匹配的记忆:可调整搜索词或筛选条件。' : '暂无记忆:可蒸馏当前会话或上方手动补录。' }}
      </p>
    </template>
    <p v-else class="sv-script-meta" style="font-size: 11px">
      请先选择角色后查看记忆库。
    </p>
  </div>
</template>

<style scoped>
/* 顶部工具栏:搜索框 + kind 筛选 + 清理已归档(窄屏换行) */
.memory-toolbar {
  display: flex;
  gap: var(--space-1-5);
  align-items: center;
  flex-wrap: wrap;
  margin-bottom: var(--space-2);
}
.memory-search {
  flex: 1;
  min-width: 140px;
  padding: var(--space-1-5) var(--space-2-5);
  font-size: 12px;
}
.memory-kind-select {
  flex: none;
  width: 110px;
  padding: var(--space-1-5) var(--space-2);
  font-size: 12px;
}

/* 记忆列表:逐条卡片,头部 kind 标签 + 计数/时间 + 注入开关 */
.memory-list {
  display: flex;
  flex-direction: column;
  gap: var(--space-1-5);
  /* UIP-15:退场行以 absolute 钉位,容器须为定位上下文 */
  position: relative;
}
.memory-row {
  border: 1px solid var(--sv-line);
  padding: var(--space-1-5) var(--space-2);
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
  /* 2026-09 动效补齐:行进入 + 置顶态切换过渡(原先为突变);
     2026-10-02 UIP-9:入场改走全局唯一 sv-list-in,并补逐项 stagger(原无)
     UIP-15:本行自带 transition 与 sv-list-* 类同特异性、scoped 后注入会覆盖类——
     故把 opacity/transform 并入本行 transition(时长与 sv-list 一致,退场/重排才生效) */
  animation: sv-list-in var(--dur-normal) var(--ease-out) backwards;
  transition: border-color var(--dur-fast) var(--ease-standard),
    background var(--dur-fast) var(--ease-standard),
    opacity var(--dur-normal) var(--ease-standard),
    transform var(--dur-normal) var(--ease-out);
}
/* 列表逐项延迟入场(与任务侧列表同款语言;第 7 项起并入同一档) */
.memory-row:nth-child(1) { animation-delay: 0ms; }
.memory-row:nth-child(2) { animation-delay: var(--stagger-step); }
.memory-row:nth-child(3) { animation-delay: calc(var(--stagger-step) * 2); }
.memory-row:nth-child(4) { animation-delay: calc(var(--stagger-step) * 3); }
.memory-row:nth-child(5) { animation-delay: calc(var(--stagger-step) * 4); }
.memory-row:nth-child(6) { animation-delay: calc(var(--stagger-step) * 5); }
.memory-row:nth-child(n + 7) { animation-delay: calc(var(--stagger-step) * 6); }
/* UIP-15:入场播完即摘动画——行上任何 animation 都会让 Vue 的 move 检测判 type=animation 而跳过 */
.memory-row-settled {
  animation: none;
}
/* 置顶行:左侧粉色竖条 + 浅粉底,与列表其余行区分 */
.memory-pinned {
  border-left: 3px solid var(--sv-pink-deep);
  background: var(--sv-pink-light);
}
.memory-row-head {
  display: flex;
  gap: var(--space-2);
  align-items: center;
}
.memory-sel {
  margin-left: auto;
  display: flex;
  gap: var(--space-1);
  align-items: center;
  font-size: 11px;
  color: var(--sv-ink-dim);
  cursor: pointer;
  flex-shrink: 0;
}
.memory-content {
  margin: 0;
  font-size: 12px;
  line-height: 1.6;
  word-break: break-all;
}
/* 搜索命中高亮(不改变行高与排版) */
.memory-hit {
  background: var(--sv-yellow, #ffe066);
  color: inherit;
  padding: 0 1px;
}
.memory-edit {
  display: flex;
  gap: var(--space-1-5);
  align-items: center;
}
.memory-ops {
  display: flex;
  gap: var(--space-1-5);
  align-items: center;
}

/* kind 标签配色:蓝=蒸馏 / 黄=工具 / 绿=手动;未知中性灰(与 memoryPanel.ts 的 kindClass 对应)。
   2026-10-02 UIP-10 字色按底色分型:深底(蓝/灰)白字;亮底(黄/绿) ink 字
   ——原白字在黄底仅 1.85:1、绿底 3.30:1,过不了 WCAG AA */
.kind-tag {
  font-size: 10px;
  padding: 1px var(--space-1-5);
  color: var(--sv-ink);
  flex-shrink: 0;
}
.kind-distilled {
  background: var(--sv-blue);
  color: var(--sv-white);
}
.kind-tool {
  background: var(--sv-yellow);
}
.kind-manual {
  background: var(--sv-green);
}
.kind-unknown {
  background: var(--sv-ink-faint);
  color: var(--sv-white);
}

/* 置顶标记:深粉描边小标签,与 kind 实心色块区分层级。
   UIP-10:字色 pink-deep→pink-dark 且加白底(3.2→4.5:1;置顶行粉底上亦稳) */
.memory-pin-tag {
  font-size: 10px;
  padding: 1px 5px;
  border: 1px solid var(--sv-pink-deep);
  color: var(--sv-pink-dark);
  background: var(--sv-white);
  flex-shrink: 0;
}

/* 删除/清理按钮红色系(危险操作) */
.memory-danger {
  color: var(--sv-red);
}
</style>
