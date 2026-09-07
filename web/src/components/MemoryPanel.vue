<script setup lang="ts">
// 记忆库分区(优化面板):当前角色的跨会话记忆列表 + 蒸馏当前会话 + 手动补录。
// 数据来自 /api/memory(api/memory.ts);交互逻辑在 useMemoryPanel composable,
// 展示与反馈文案纯函数在 memoryPanel.ts。
// 操作:行内编辑 content(PATCH)、删除(两段式确认)、注入开关(selected)、
// 顶部「蒸馏当前会话」(POST /api/memory/distill,未开启时 400 带去设置指引)。
// onServerPrefetch:服务端渲染测试通道(renderToString 会等待预取完成)。
import { onMounted, onServerPrefetch } from 'vue';
import { useMemoryPanel } from '../composables/useMemoryPanel';
import type { MemoryRow } from './memoryPanel';

const props = defineProps<{ characterId?: string | null; sessionId?: string | null }>();

const panel = useMemoryPanel({
  characterId: () => props.characterId,
  sessionId: () => props.sessionId,
});
const {
  loading, error, rows,
  distilling, distillMsg,
  actionMsg, newContent, adding,
  editingId, editContent, savingEdit,
  pendingDelete, deleting,
  load, distillNow, addNow,
  startEdit, cancelEdit, saveEdit, toggleSelected,
  requestDelete, cancelDelete, confirmDelete,
} = panel;

/** 模板事件包装:取 checkbox 勾选态(composable 保持纯参数可测) */
function onToggle(row: MemoryRow, ev: Event): void {
  void toggleSelected(row, (ev.target as HTMLInputElement).checked);
}

onMounted(load);
onServerPrefetch(load);
</script>

<template>
  <div class="sv-field">
    <div class="sv-field-label"><span class="sv-supreme pink-deep" /> 记忆库</div>
    <p class="sv-note" style="margin: 0 0 8px; line-height: 1.8">
      当前角色的跨会话记忆:由会话蒸馏、工具写入或手动补录;勾选「注入」的条目按
      使用热度注入提示词(条数上限见 设置 → 生成参数)。
    </p>

    <template v-if="characterId">
      <!-- 蒸馏 + 刷新 -->
      <div style="display: flex; gap: 8px; align-items: center; margin-bottom: 8px">
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
      <div v-if="distillMsg" class="sv-feedback" :class="distillMsg.kind" style="margin-bottom: 8px">
        {{ distillMsg.text }}
      </div>
      <div v-if="error" class="sv-feedback err" style="margin-bottom: 8px">{{ error }}</div>

      <!-- 手动补录 -->
      <div style="display: flex; gap: 8px; align-items: center; margin-bottom: 8px">
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
      <div v-if="actionMsg" class="sv-feedback" :class="actionMsg.kind" style="margin-bottom: 8px">
        {{ actionMsg.text }}
      </div>

      <!-- 记忆列表(id 降序) -->
      <div v-if="rows.length" class="memory-list">
        <div v-for="row in rows" :key="row.id" class="memory-row">
          <div class="memory-row-head">
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
          <p v-else class="memory-content">{{ row.content }}</p>

          <div class="memory-ops">
            <template v-if="pendingDelete === row.id">
              <span class="sv-script-meta" style="font-size: 11px">确认删除该记忆?</span>
              <button class="sv-btn ghost sv-btn-sm memory-danger" :disabled="deleting" @click="confirmDelete">
                {{ deleting ? '删除中…' : '确认删除' }}
              </button>
              <button class="sv-btn ghost sv-btn-sm" @click="cancelDelete">取消</button>
            </template>
            <template v-else>
              <button class="sv-btn ghost sv-btn-sm" @click="startEdit(row)">编辑</button>
              <button class="sv-btn ghost sv-btn-sm memory-danger" @click="requestDelete(row)">删除</button>
            </template>
          </div>
        </div>
      </div>
      <p v-else-if="!error && !loading" class="sv-script-meta" style="font-size: 11px">
        暂无记忆:可蒸馏当前会话或上方手动补录。
      </p>
    </template>
    <p v-else class="sv-script-meta" style="font-size: 11px">
      请先选择角色后查看记忆库。
    </p>
  </div>
</template>

<style scoped>
/* 记忆列表:逐条卡片,头部 kind 标签 + 计数/时间 + 注入开关 */
.memory-list {
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.memory-row {
  border: 1px solid var(--sv-line);
  padding: 6px 8px;
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.memory-row-head {
  display: flex;
  gap: 8px;
  align-items: center;
}
.memory-sel {
  margin-left: auto;
  display: flex;
  gap: 4px;
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
.memory-edit {
  display: flex;
  gap: 6px;
  align-items: center;
}
.memory-ops {
  display: flex;
  gap: 6px;
  align-items: center;
}

/* kind 标签配色:蓝=蒸馏 / 黄=工具 / 绿=手动;未知中性灰(与 memoryPanel.ts 的 kindClass 对应) */
.kind-tag {
  font-size: 10px;
  padding: 1px 6px;
  color: var(--sv-white);
  flex-shrink: 0;
}
.kind-distilled {
  background: var(--sv-blue);
}
.kind-tool {
  background: var(--sv-yellow);
}
.kind-manual {
  background: var(--sv-green);
}
.kind-unknown {
  background: var(--sv-ink-faint);
}

/* 删除按钮红色系(危险操作) */
.memory-danger {
  color: var(--sv-red);
}
</style>
