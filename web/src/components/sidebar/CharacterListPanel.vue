<script setup lang="ts">
// 角色模式中栏:搜索 / 空态 / 角色列表 / 长按与右键菜单（C-2A，2026-10-25）。
// 点击选中、长按等价右键；提示词编辑交给 CharacterPromptEditor，删除仍走 store action。
import { ref } from 'vue';
import { storeToRefs } from 'pinia';
import { useAppStore } from '../../store';
import CharacterPromptEditor from './CharacterPromptEditor.vue';
import type { CharacterRecord } from '../../api';

const store = useAppStore();
const { filteredCharacters, characters, currentCharacterId } = storeToRefs(store);

/** 角色搜索框只读 store + action 写入;纯 UI 过滤草稿,无持久化。 */
function onSearchInput(ev: Event): void {
  store.setSearchQuery((ev.target as HTMLInputElement).value);
}

function avatarFile(avatarPath: string | null): string {
  if (!avatarPath) return '';
  return avatarPath.split(/[\\/]/).pop() ?? '';
}

function avatarClass(id: string): string {
  const n = id.charCodeAt(0) || 0;
  if (n % 3 === 1) return 'alt-blue';
  if (n % 3 === 2) return 'alt-red';
  return '';
}

// ===== 右键菜单(编辑提示词 / 删除) =====
const ctxMenu = ref<{ x: number; y: number; char: CharacterRecord } | null>(null);

/** 长按菜单的坐标基准:「右键」在触屏上不存在,以长按等价替代。
 *  长按触发后需吞掉随后到来的 click,否则会连带选中该角色(用户本意只是开菜单)。 */
let longPressTimer: ReturnType<typeof setTimeout> | null = null;
let longPressOrigin: { x: number; y: number } | null = null;
let suppressClick = false;

const LONG_PRESS_MS = 500;
/** 手指位移超过此阈值视为滚动/拖拽,取消长按(避免滑动列表时误弹菜单) */
const LONG_PRESS_MOVE_TOLERANCE = 10;

function openCtxMenuAt(x: number, y: number, c: CharacterRecord): void {
  const clampedX = Math.min(x, window.innerWidth - 168);
  const clampedY = Math.min(y, window.innerHeight - 96);
  ctxMenu.value = { x: clampedX, y: clampedY, char: c };
}

function openCtxMenu(e: MouseEvent, c: CharacterRecord): void {
  e.preventDefault();
  openCtxMenuAt(e.clientX, e.clientY, c);
}

function cancelLongPress(): void {
  if (longPressTimer) {
    clearTimeout(longPressTimer);
    longPressTimer = null;
  }
  longPressOrigin = null;
}

function onCharTouchStart(e: TouchEvent, c: CharacterRecord): void {
  const touch = e.touches[0];
  if (!touch) return;
  longPressOrigin = { x: touch.clientX, y: touch.clientY };
  cancelLongPressTimerOnly();
  longPressTimer = setTimeout(() => {
    longPressTimer = null;
    const origin = longPressOrigin;
    longPressOrigin = null;
    if (!origin) return;
    suppressClick = true;
    openCtxMenuAt(origin.x, origin.y, c);
  }, LONG_PRESS_MS);
}

function onCharTouchMove(e: TouchEvent): void {
  if (!longPressTimer || !longPressOrigin) return;
  const touch = e.touches[0];
  if (!touch) return;
  const moved = Math.abs(touch.clientX - longPressOrigin.x) + Math.abs(touch.clientY - longPressOrigin.y);
  if (moved > LONG_PRESS_MOVE_TOLERANCE) cancelLongPressTimerOnly();
}

function cancelLongPressTimerOnly(): void {
  if (longPressTimer) {
    clearTimeout(longPressTimer);
    longPressTimer = null;
  }
}

/** 点击角色:长按已弹菜单时吞掉本次 click(仅吞一次) */
function onCharClick(c: CharacterRecord): void {
  if (suppressClick) {
    suppressClick = false;
    return;
  }
  void store.selectCharacter(c.id);
}

function closeCtx(): void {
  ctxMenu.value = null;
  cancelLongPress();
  suppressClick = false;
}

// ===== 编辑角色提示词 / 开场白 =====
const editingChar = ref<CharacterRecord | null>(null);

function openPromptEditor(c: CharacterRecord): void {
  editingChar.value = c;
  closeCtx();
}

// ===== 删除角色 =====
function deleteChar(c: CharacterRecord): void {
  closeCtx();
  if (!confirm(`确定删除角色「${c.chara_name}」?其会话与消息将一并删除。`)) return;
  void store.deleteCharacter(c.id);
}
</script>

<template>
  <div class="min-h-0 flex-1 flex flex-col">
    <input
      :value="store.searchQuery"
      type="text"
      placeholder="搜索角色..."
      class="sv-search"
      @input="onSearchInput"
    />

    <div v-if="characters.length === 0" class="sv-empty compact">
      <div class="sv-empty-geo mb10">
        <span class="sq black" />
        <i class="diag" />
      </div>
      <p class="sub">暂无角色</p>
      <p class="hint">点击下方「导入角色卡」开始</p>
    </div>

    <div class="min-h-0 flex-1 overflow-y-auto pr-1">
      <button
        v-for="c in filteredCharacters"
        :key="c.id"
        class="sv-char-item"
        :class="{ active: c.id === currentCharacterId }"
        @click="onCharClick(c)"
        @contextmenu="openCtxMenu($event, c)"
        @touchstart.passive="onCharTouchStart($event, c)"
        @touchmove.passive="onCharTouchMove($event)"
        @touchend="cancelLongPress"
        @touchcancel="cancelLongPress"
      >
        <span class="sv-avatar small char" :class="avatarClass(c.id)">
          <img v-if="c.avatar_path" :src="`/api/avatars/${avatarFile(c.avatar_path)}`" alt="" />
          <template v-else>{{ c.chara_name.charAt(0) }}</template>
        </span>
        <span class="min-w-0 flex-1">
          <span class="sv-char-name">{{ c.chara_name }}</span>
          <span class="sv-char-desc">{{ c.description || '无提示词' }}</span>
        </span>
      </button>
    </div>

    <!-- 角色操作菜单(编辑提示词 / 删除)。
         Teleport 到 body 是必需的:移动端侧栏抽屉带 transform + overflow-y:auto,
         而 transform 祖先会成为 position:fixed 后代的包含块,导致菜单定位偏移、
         遮罩只盖住侧栏、且被 overflow 裁切。挂到 body 后按视口坐标正常定位。 -->
    <Teleport to="body">
      <div v-if="ctxMenu" class="sv-ctx-mask" @click="closeCtx" @contextmenu.prevent="closeCtx">
        <div class="sv-ctx-menu" :style="{ left: `${ctxMenu.x}px`, top: `${ctxMenu.y}px` }" @click.stop>
          <div class="sv-ctx-title">{{ ctxMenu.char.chara_name }}</div>
          <button class="sv-ctx-item" @click="openPromptEditor(ctxMenu.char)">
            <svg viewBox="0 0 24 24">
              <path d="M12 20h9" />
              <path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4z" />
            </svg>
            编辑提示词
          </button>
          <button class="sv-ctx-item danger" @click="deleteChar(ctxMenu.char)">
            <svg viewBox="0 0 24 24">
              <path d="M3 6h18" />
              <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6" />
              <path d="M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />
              <path d="M10 11v6" />
              <path d="M14 11v6" />
            </svg>
            删除角色
          </button>
        </div>
      </div>
    </Teleport>

    <!-- 编辑角色提示词模态框(同样 Teleport:见上,侧栏 transform 会破坏 fixed 定位) -->
    <CharacterPromptEditor
      v-if="editingChar"
      :character="editingChar"
      @close="editingChar = null"
    />
  </div>
</template>
