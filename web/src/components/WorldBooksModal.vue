<script setup lang="ts">
// 世界书管理模态框:
//   - 角色卡内嵌世界书(character_book):当前角色内嵌条目展示 + 编辑(位置/关键词/常驻/激活)
//   - 独立世界书:上传/绑定/启用/删除 + 条目编辑(同酒馆 World Info 编辑器)
import { ref, computed, onMounted } from 'vue';
import { storeToRefs } from 'pinia';
import { useAppStore } from '../store';
import * as api from '../api';
import SkeletonBlock from './SkeletonBlock.vue';
import WorldBookEntryEditor from './WorldBookEntryEditor.vue';
import { diffEntryIds, isRenderableEntry } from '../utils/worldBookEntryDiff';

const store = useAppStore();
const { worldBooks, characters, currentCharacter, currentCharacterId } = storeToRefs(store);

const fileInput = ref<HTMLInputElement | null>(null);
const uploadError = ref('');
const uploadMsg = ref('');
const busy = ref(false);
/** 上传时是否绑定当前选中角色 */
const bindCurrent = ref(true);

// ===== 角色卡内嵌世界书(当前角色) =====
const charEntries = ref<api.WorldBookEntry[]>([]);
const charEntriesLoaded = ref(false);
const charSaving = ref(false);
const charMsg = ref<{ kind: 'ok' | 'err'; text: string } | null>(null);
/** 打开时的条目 id 基线:保存时用它算显式 removed/added,不能靠「视图内没有」猜删除 */
const charBaseline = ref<Set<number>>(new Set());

function idSet(entries: api.WorldBookEntry[]): Set<number> {
  return new Set(entries.map((e) => e.id));
}

/** 用保存响应刷新草稿,但保留本地尚未填内容的空行(否则保存后空行凭空消失)。
 *  仍属于基线的空行继续留在基线里,下次保存按叠加处理,避免被当成 added 反复换 uid。 */
function mergeSavedDraft(
  draft: api.WorldBookEntry[],
  saved: api.WorldBookEntry[],
  baseline: Set<number>,
): { entries: api.WorldBookEntry[]; baseline: Set<number> } {
  const savedIds = new Set(saved.map((e) => e.id));
  const localBlanks = draft.filter((e) => !savedIds.has(e.id) && !isRenderableEntry(e));
  const nextBaseline = new Set(savedIds);
  for (const e of localBlanks) {
    if (baseline.has(e.id)) nextBaseline.add(e.id);
  }
  return { entries: [...saved, ...localBlanks], baseline: nextBaseline };
}

/** 归一注入角色:后端 role=None 序列化时省略(undefined),下拉「自动」选项值是 null,需对齐 */
function normalizeRoles(entries: api.WorldBookEntry[]): void {
  for (const e of entries) e.role = e.role ?? null;
}

/** 提交前把「被清空的可选数值」转成缺省:数字输入框清空时 Vue 交出空串,
 *  而 scan_depth 的后端语义是「不带该键 = 保持原值」("" 会被反序列化判为非法类型)。 */
function omitClearedOptional(entries: api.WorldBookEntry[]): api.WorldBookEntry[] {
  return entries.map((e) => ((e.scan_depth as unknown) === '' ? { ...e, scan_depth: undefined } : e));
}

async function loadCharacterEntries(): Promise<void> {
  const cid = currentCharacterId.value;
  if (!cid) return;
  charEntriesLoaded.value = false;
  charMsg.value = null;
  try {
    charEntries.value = await api.getCharacterWorldEntries(cid);
    normalizeRoles(charEntries.value);
    charBaseline.value = idSet(charEntries.value);
  } catch (err) {
    charMsg.value = { kind: 'err', text: `加载角色卡世界书失败:${(err as Error).message}` };
  } finally {
    charEntriesLoaded.value = true;
  }
}

async function saveCharacterEntries(): Promise<void> {
  const cid = currentCharacterId.value;
  if (!cid || charSaving.value) return;
  charSaving.value = true;
  charMsg.value = null;
  try {
    const draft = charEntries.value;
    const { removed, added } = diffEntryIds(charBaseline.value, draft.map((e) => e.id));
    const saved = await api.saveCharacterWorldEntries(
      cid,
      omitClearedOptional(draft),
      removed,
      added,
    );
    normalizeRoles(saved);
    const merged = mergeSavedDraft(draft, saved, charBaseline.value);
    charEntries.value = merged.entries;
    charBaseline.value = merged.baseline;
    charMsg.value = { kind: 'ok', text: '角色卡世界书已保存(下次发送生效)' };
    setTimeout(() => (charMsg.value = null), 2500);
  } catch (err) {
    charMsg.value = { kind: 'err', text: `保存失败:${(err as Error).message}` };
  } finally {
    charSaving.value = false;
  }
}

// ===== 独立世界书条目编辑 =====
/** 正在编辑条目的世界书 id */
const editingBookId = ref<string | null>(null);
/** 条目编辑草稿:bookId → entries */
const entryDrafts = ref<Record<string, api.WorldBookEntry[]>>({});
/** 打开编辑器时的条目 id 基线:保存时算显式 removed/added */
const entryBaselines = ref<Record<string, Set<number>>>({});
const entrySaving = ref<Record<string, boolean>>({});
const entryMsg = ref<{ kind: 'ok' | 'err'; text: string } | null>(null);

const editCount = computed(() => (editingBookId.value && entryDrafts.value[editingBookId.value] ? entryDrafts.value[editingBookId.value].length : 0));
/** 当前编辑的独立世界书(找不到 = null;保存按钮据此禁用,替代非空断言) */
const editingBook = computed(() => worldBooks.value.find((b) => b.id === editingBookId.value) ?? null);
/** 当前编辑草稿(非空引用;模板里不再对可能为 null 的 editingBookId 索引) */
const editingEntries = computed(() =>
  editingBookId.value ? entryDrafts.value[editingBookId.value] ?? [] : [],
);
/** 当前草稿是否保存中(模板不再直接索引可能为 null 的 id) */
const editingSaving = computed(() =>
  editingBookId.value ? !!entrySaving.value[editingBookId.value] : false,
);

async function openEntries(b: api.WorldBookRecord): Promise<void> {
  if (editingBookId.value === b.id) {
    editingBookId.value = null;
    return;
  }
  entryMsg.value = null;
  try {
    const entries = await api.getWorldBookEntries(b.id);
    normalizeRoles(entries);
    entryDrafts.value[b.id] = entries;
    entryBaselines.value[b.id] = idSet(entries);
    editingBookId.value = b.id;
  } catch (err) {
    delete entryBaselines.value[b.id];
    listOpError.value = `加载条目失败:${(err as Error).message}`;
  }
}

function addEntryRow(): void {
  const id = editingBookId.value;
  const draft = id && entryDrafts.value[id];
  if (!draft) return;
  const maxId = draft.reduce((m, e) => Math.max(m, e.id), 0);
  draft.push({ id: maxId + 1, comment: '新条目', keys: [], regex: null, use_regex: false, constant: false, enabled: true, content: '', position: 1 });
}

function addCharEntryRow(): void {
  const maxId = charEntries.value.reduce((m, e) => Math.max(m, e.id), 0);
  charEntries.value.push({ id: maxId + 1, comment: '新条目', keys: [], regex: null, use_regex: false, constant: false, enabled: true, content: '', position: 1 });
}

function removeEntryRow(draft: api.WorldBookEntry[], i: number): void {
  draft.splice(i, 1);
}

/** 子组件字段补丁：写回父级草稿对象（两套编辑器共用;不改数组引用,保存逻辑照旧读草稿） */
function applyEntryPatch(entry: api.WorldBookEntry, key: keyof api.WorldBookEntry, value: unknown): void {
  Object.assign(entry, { [key]: value });
}

/** 常驻/触发切换并联动注入位置:常驻 → 位置3(常态),触发 → 位置1(激发),与 6 层规范对齐 */
function toggleConstant(en: api.WorldBookEntry): void {
  en.constant = !en.constant;
  en.position = en.constant ? 3 : 1;
}

/** 组内上移/下移(常态/激发各一组,不跨组);移动后重排 order */
function moveEntryRow(draft: api.WorldBookEntry[], i: number, dir: -1 | 1): void {
  const to = i + dir;
  if (to < 0 || to >= draft.length) return;
  [draft[i], draft[to]] = [draft[to], draft[i]];
  draft.forEach((e, idx) => (e.order = idx));
}

// `_b` 为调用点签名保留(整本记录当前不需要),故以 _ 前缀表达有意未用
async function saveEntries(_b: api.WorldBookRecord): Promise<void> {
  const id = editingBookId.value;
  const draft = id && entryDrafts.value[id];
  if (!id || !draft || entrySaving.value[id]) return;
  entrySaving.value[id] = true;
  entryMsg.value = null;
  try {
    const baseline = entryBaselines.value[id] ?? new Set<number>();
    const { removed, added } = diffEntryIds(baseline, draft.map((e) => e.id));
    const saved = await api.saveWorldBookEntries(
      id,
      omitClearedOptional(draft),
      removed,
      added,
    );
    normalizeRoles(saved);
    const merged = mergeSavedDraft(draft, saved, baseline);
    entryDrafts.value[id] = merged.entries;
    entryBaselines.value[id] = merged.baseline;
    entryMsg.value = { kind: 'ok', text: '条目已保存(下次发送生效)' };
    setTimeout(() => (entryMsg.value = null), 2500);
  } catch (err) {
    entryMsg.value = { kind: 'err', text: `保存失败:${(err as Error).message}` };
  } finally {
    delete entrySaving.value[id];
  }
}

const close = (): void => {
  store.worldBooksOpen = false;
};

function sourceLabel(source: string): string {
  return source === 'character_card' ? '角色卡内嵌' : '独立';
}

async function onFilePicked(e: Event): Promise<void> {
  const input = e.target as HTMLInputElement;
  const file = input.files?.[0];
  input.value = '';
  if (!file || busy.value) return;
  busy.value = true;
  uploadError.value = '';
  uploadMsg.value = '';
  try {
    const cid = bindCurrent.value ? currentCharacterId.value ?? undefined : undefined;
    const record = await store.uploadWorldBook(file, cid);
    const conv = record.conversion;
    const convText =
      conv && (conv.converted_count > 0 || conv.constant_auto_count > 0)
        ? `｜自动转换 ${conv.converted_count} 条(常态自动判定 ${conv.constant_auto_count} 条、关键词归一 ${conv.key_normalized_count} 条)`
        : '';
    uploadMsg.value = `已上传「${record.name}」,${record.entry_count} 条条目生效${convText}`;
  } catch (err) {
    uploadError.value = (err as Error).message;
  } finally {
    busy.value = false;
  }
}

// ===== 自动分配属性机制自检 =====
const autoCheck = ref<{ running: boolean; result: api.AutoAssignCheckResult | null; error: string | null }>({
  running: false,
  result: null,
  error: null,
});

async function runAutoAssignCheck(): Promise<void> {
  if (autoCheck.value.running) return;
  autoCheck.value = { running: true, result: null, error: null };
  try {
    autoCheck.value.result = await api.checkWorldBookAutoAssign(currentCharacterId.value ?? undefined);
  } catch (err) {
    autoCheck.value.error = `自检失败:${(err as Error).message}`;
  } finally {
    autoCheck.value.running = false;
  }
}

// ===== 独立世界书列表加载错误 =====
const listLoadError = ref<string>('');
/** 列表区操作错误(打开条目/启停/绑定/删除),就近显示在列表上方 */
const listOpError = ref<string>('');

async function toggleEnabled(b: api.WorldBookRecord): Promise<void> {
  try {
    listOpError.value = '';
    await store.updateWorldBook(b.id, { enabled: !b.enabled });
  } catch (err) {
    listOpError.value = `操作失败:${(err as Error).message}`;
  }
}

async function onBindChange(e: Event, b: api.WorldBookRecord): Promise<void> {
  const val = (e.target as HTMLSelectElement).value;
  if (val === (b.character_id ?? '')) return;
  try {
    listOpError.value = '';
    await store.updateWorldBook(b.id, { character_id: val });
  } catch (err) {
    listOpError.value = `绑定失败:${(err as Error).message}`;
  }
}

async function removeBook(b: api.WorldBookRecord): Promise<void> {
  if (!confirm(`确定删除世界书「${b.name}」?删除后不再注入。`)) return;
  try {
    listOpError.value = '';
    await store.deleteWorldBook(b.id);
    if (editingBookId.value === b.id) {
      editingBookId.value = null;
      delete entryDrafts.value[b.id];
      delete entryBaselines.value[b.id];
    }
  } catch (err) {
    listOpError.value = `删除失败:${(err as Error).message}`;
  }
}

onMounted(() => {
  void loadWorldBooksChecked();
  void loadCharacterEntries();
});

/** 加载独立世界书列表并捕获失败(避免误显示「暂无独立世界书」) */
async function loadWorldBooksChecked(): Promise<void> {
  listLoadError.value = '';
  await store.loadWorldBooks();
  if (store.worldBooksError) {
    listLoadError.value = `加载独立世界书失败:${store.worldBooksError}`;
  }
}
</script>

<template>
  <div class="sv-modal-mask" @click.self="close">
    <div class="sv-modal wb-modal">
      <!-- 头部 -->
      <div class="sv-modal-head">
        <h2 class="flex items-center gap-2">
          <span class="sv-supreme pink" /> 世界书
        </h2>
        <button class="sv-btn ghost sv-btn-square" @click="close">✕</button>
      </div>

      <div class="sv-modal-body">
        <!-- 说明 -->
        <div class="sv-field">
          <div class="sv-field-label"><span class="sv-supreme blue" /> 说明</div>
          <p class="sv-note" style="margin: 0; line-height: 1.8">
            世界书(World Info)在对话时按关键词命中注入设定。<br />
            <b>内嵌世界书</b>:角色卡内
            <code>character_book</code>
            条目随角色卡自动生效(常驻条目始终注入,其余按关键词匹配),可在此直接编辑。<br />
            <b>独立世界书</b>:上传 SillyTavern 世界书 JSON 后,默认全局启用,也可绑定到指定角色;<br />
            条目编辑完整支持酒馆惯例:<code>位置</code>/<code>注入顺序</code>(排序)、
            <code>关键词</code>/<code>副关键词</code>(子串匹配)、<code>正则</code>、
            <code>插入深度</code>/<code>扫描窗口</code>(酒馆 depth 与 scanDepth,语义分离)、<code>常驻</code>(始终注入)、
            <code>命中概率</code>、<code>大小写敏感</code>与<code>激活状态</code>(总开关)。<br />
            <b>状态定义</b>:<code>激活</code> = 该条目参与注入的总开关(停用 = 完全不注入);
            <code>触发</code> = 激活且非常驻的条目,按关键词/正则命中最近对话才注入;
            <code>常驻</code> = 始终注入,无需关键词命中。
          </p>
        </div>

        <!-- 自动分配机制自检 -->
        <div class="sv-field">
          <div class="sv-field-label"><span class="sv-supreme green" /> 自动分配机制</div>
          <p class="sv-note" style="margin: 0 0 var(--space-2); line-height: 1.8">
            条目「注入角色」选<code>自动</code>时按 常驻→系统提示词、激发→用户 自动分配;
            上传时自动转换关键词拆分、常态/激发判定与属性默认自动。可一键自检链路是否可用。
          </p>
          <div class="sv-stack">
            <button class="sv-btn ghost sv-btn-sm" :disabled="autoCheck.running" @click="runAutoAssignCheck">
              {{ autoCheck.running ? '检查中…' : '检查自动分配机制' }}
            </button>
            <div v-if="autoCheck.error" class="sv-feedback err" style="margin-top: var(--space-1-5)">{{ autoCheck.error }}</div>
            <div v-else-if="autoCheck.result" class="sv-wb-check">
              <div class="sv-feedback" :class="autoCheck.result.ok ? 'ok' : 'err'" style="margin: var(--space-1-5) 0">{{ autoCheck.result.summary }}</div>
              <div v-for="c in autoCheck.result.checks" :key="c.name" class="sv-wb-check-item">
                <span class="sv-tag" :class="c.status === 'ok' ? 'sv-tag-on' : ''">{{ c.status === 'ok' ? '正常' : '异常' }}</span>
                <span class="sv-wb-check-name">{{ c.label }}</span>
                <span class="sv-wb-check-detail">{{ c.detail }}</span>
              </div>
            </div>
          </div>
        </div>

        <!-- 角色卡内嵌世界书(当前角色) -->
        <div class="sv-field">
          <div class="sv-field-label">
            <span class="sv-supreme red" /> 角色卡内嵌世界书
            <span class="sv-wb-count">{{ currentCharacter?.chara_name ?? '未选择角色' }}</span>
          </div>
          <SkeletonBlock v-if="currentCharacterId && !charEntriesLoaded" :lines="3" />
          <div v-else-if="currentCharacterId && charMsg?.kind === 'err'" class="sv-empty" style="padding: var(--space-4) var(--space-3)">
            <div class="sv-empty-geo mb8">
              <span class="sq black" />
              <span class="sq pink" />
              <span class="sq deep" />
              <i class="diag" />
            </div>
            <p class="sv-note-mini err">{{ charMsg.text }}</p>
          </div>
          <div v-else-if="currentCharacterId && charEntries.length === 0" class="sv-empty" style="padding: var(--space-5) var(--space-3)">
            <div class="sv-empty-geo mb8">
              <span class="sq black" />
              <span class="sq pink" />
              <span class="sq deep" />
            <i class="diag" />
            </div>
            <p style="font-size: 12px">该角色卡未内嵌世界书</p>
            <p style="font-size: 11px">在角色卡 JSON 中提供 character_book 后即可在此编辑</p>
          </div>
          <div v-else-if="!currentCharacterId" class="sv-empty" style="padding: var(--space-5) var(--space-3)">
            <div class="sv-empty-geo mb8">
              <span class="sq black" />
              <span class="sq pink" />
              <span class="sq deep" />
              <i class="diag" />
            </div>
            <p style="font-size: 12px">请在左侧选择一个角色</p>
            <p style="font-size: 11px">选中后可查看/编辑其内嵌世界书</p>
          </div>
          <template v-else>
            <div class="sv-wb-edit">
              <WorldBookEntryEditor
                v-for="(en, i) in charEntries"
                :key="`c-${en.id}-${i}`"
                :entry="en"
                :index="i"
                :total="charEntries.length"
                keys-placeholder="逗号分隔;命中最近对话中的任意词即注入"
                regex-placeholder="可选;填写并勾选正则后优先于关键词匹配"
                @patch="(k, v) => applyEntryPatch(en, k, v)"
                @toggle-constant="toggleConstant(en)"
                @move="(d) => moveEntryRow(charEntries, i, d)"
                @remove="removeEntryRow(charEntries, i)"
              />
              <div class="sv-wb-edit-foot">
                <button class="sv-btn ghost sv-btn-sm" @click="addCharEntryRow">＋ 新增条目</button>
                <button class="sv-btn primary sv-btn-sm" :disabled="charSaving" @click="saveCharacterEntries">
                  {{ charSaving ? '保存中…' : '保存角色卡世界书' }}
                </button>
              </div>
            </div>
            <div v-if="charMsg" class="sv-feedback" :class="charMsg.kind">{{ charMsg.text }}</div>
          </template>
        </div>

        <!-- 上传 -->
        <div class="sv-field">
          <div class="sv-field-label"><span class="sv-supreme" /> 上传独立世界书</div>
          <div class="sv-stack">
            <label class="sv-wb-bind">
              <input v-model="bindCurrent" type="checkbox" />
              绑定到当前角色「{{ currentCharacter?.chara_name ?? '未选择角色' }}」
              <span class="sv-wb-bind-note">(不勾选则为全局世界书)</span>
            </label>
            <button class="sv-btn primary sv-btn-fill" :disabled="busy" @click="fileInput?.click()">
              {{ busy ? '上传中…' : '选择 JSON 文件上传' }}
            </button>
            <input ref="fileInput" type="file" accept=".json,application/json" class="hidden" @change="onFilePicked" />
            <p class="sv-note" style="margin-top: var(--space-1-5)">支持 SillyTavern 世界书导出格式(顶层 <code>entries</code> 对象/数组)与角色卡 <code>character_book</code> 结构。</p>
          </div>
          <div v-if="uploadError" class="sv-feedback err">{{ uploadError }}</div>
          <div v-if="uploadMsg" class="sv-feedback ok">{{ uploadMsg }}</div>
        </div>

        <!-- 独立世界书列表 -->
        <div class="sv-field">
          <div class="sv-field-label">
            <span class="sv-supreme yellow" /> 独立世界书列表
            <span class="sv-wb-count">{{ worldBooks.length }} 本</span>
          </div>
          <div v-if="listLoadError" class="sv-feedback err" style="margin: var(--space-2) 0">{{ listLoadError }}</div>
          <div v-else-if="listOpError" class="sv-feedback err" style="margin: var(--space-2) 0">{{ listOpError }}</div>
          <div v-else-if="worldBooks.length === 0" class="sv-empty" style="padding: 28px var(--space-3)">
            <div class="sv-empty-geo mb10">
              <span class="sq black" />
              <span class="sq pink" />
              <span class="sq deep" />
            <i class="diag" />
            </div>
            <p style="font-size: 12px">暂无独立世界书</p>
            <p style="font-size: 11px">上传 JSON 后即在此列出</p>
          </div>
          <div v-else class="sv-datalist">
            <div v-for="b in worldBooks" :key="b.id" class="sv-data-row sv-wb-row">
              <div class="min-w-0 flex-1">
                <div class="flex items-center gap-2">
                  <b>{{ b.name }}</b>
                  <span class="sv-tag">{{ sourceLabel(b.source) }}</span>
                  <span class="sv-tag sv-tag-muted">{{ b.entry_count }} 条</span>
                  <span v-if="b.enabled" class="sv-tag sv-tag-on">启用</span>
                </div>
                <div class="sv-wb-sub">
                  <span v-if="b.character_id" class="sv-wb-sub-item">绑定:{{ b.character_name ?? '角色' }}</span>
                  <span v-else class="sv-wb-sub-item">全局</span>
                  <span class="sv-wb-sub-item">{{ new Date(b.created_at).toLocaleString('zh-CN', { hour12: false }) }}</span>
                </div>
              </div>

              <div class="sv-wb-ops">
                <select
                  class="sv-wb-select"
                  :value="b.character_id ?? ''"
                  title="绑定角色(空=全局)"
                  @change="onBindChange($event, b)"
                >
                  <option value="">全局</option>
                  <option v-for="c in characters" :key="c.id" :value="c.id">{{ c.chara_name }}</option>
                </select>
                <button class="sv-btn ghost sv-btn-sm" @click="openEntries(b)">
                  {{ editingBookId === b.id ? '收起' : '条目' }}
                </button>
                <button class="sv-btn ghost sv-btn-sm" :class="{ 'sv-btn-on': b.enabled }" @click="toggleEnabled(b)">
                  {{ b.enabled ? '停用' : '启用' }}
                </button>
                <button class="sv-btn danger sv-btn-sm" @click="removeBook(b)">删除</button>
              </div>
            </div>
          </div>
        </div>

        <!-- 独立世界书条目编辑区 -->
        <div v-if="editingBookId && entryDrafts[editingBookId]" class="sv-field">
          <div class="sv-field-label">
            <span class="sv-supreme red" /> 编辑条目
            <span class="sv-wb-count">{{ worldBooks.find((b) => b.id === editingBookId)?.name ?? editingBookId }} · {{ editCount }} 条</span>
          </div>
          <p class="sv-note" style="margin-top: 0">
            <b>状态定义</b>:<code>激活</code> = 条目参与注入的总开关(停用则不注入);
            <code>常驻</code> = 始终注入,归位「位置3(常态)」并入系统提示词;<code>触发</code> = 激活且非常驻时按关键词/正则命中,归位「位置1(激发)」追加到最新用户消息尾部。<br />
            注入顺序:同位置内按列表顺序注入(可 ↑↓ 排序);关键词/副关键词:逗号分隔,命中最近对话即注入;
            插入深度:酒馆 <code>depth</code>,仅作注入位置(atDepth)与窗口兜底;扫描窗口:酒馆 <code>scanDepth</code>,关键词只扫最近 N 条消息(0 = 全部,留空 = 保持原值);
            命中概率:按百分比随机决定是否注入;粘性/冷却:酒馆 sticky/cooldown 字段,当前版本尚未参与注入判定(编辑值仅原样保留)。
          </p>
          <div class="sv-wb-edit">
              <WorldBookEntryEditor
                v-for="(en, i) in editingEntries"
                :key="`e-${editingBookId}-${en.id}-${i}`"
                :entry="en"
                :index="i"
                :total="editingEntries.length"
                keys-placeholder="逗号分隔;留空且非常驻则需正则命中"
                regex-placeholder="可选;填写并开启后优先于关键词"
                @patch="(k, v) => applyEntryPatch(en, k, v)"
                @toggle-constant="toggleConstant(en)"
                @move="(d) => moveEntryRow(editingEntries, i, d)"
                @remove="removeEntryRow(editingEntries, i)"
              />
            <div class="sv-wb-edit-foot">
              <button class="sv-btn ghost sv-btn-sm" @click="addEntryRow">＋ 新增条目</button>
              <button
                class="sv-btn primary sv-btn-sm"
                :disabled="editingSaving || !editingBook"
                @click="editingBook && saveEntries(editingBook)"
              >
                {{ editingSaving ? '保存中…' : '保存条目' }}
              </button>
            </div>
          </div>
          <div v-if="entryMsg" class="sv-feedback" :class="entryMsg.kind">{{ entryMsg.text }}</div>
        </div>
      </div>

      <div class="sv-modal-foot">
        <button class="sv-btn primary" @click="close">完成</button>
      </div>
    </div>
  </div>
</template>
