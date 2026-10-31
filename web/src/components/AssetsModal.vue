<script setup lang="ts">
// 附件管理弹窗(IMPHUB 步骤 D,2026-10-09)。
//
// 四个页签,把原先散在各处的资产组织面收在一处:
//   技能 / 流程 / 插件 —— 文件夹式分层(左文件夹树 + 右列表,「移动到…」下拉,不做拖拽);
//   预设正则        —— 扁平列表(正则条目没有 folder 元数据,只有启停/删除/查看)。
//
// folder 是**虚拟元数据**(不是真实目录):路径由条目自带字段派生,不做空文件夹持久化——
// 某文件夹下最后一个条目被移走时该文件夹自动消失(树构建见 utils/assetFolders)。
//
// 本弹窗取代了此前的 SkillsModal 与 PluginsModal:技能的内容编辑并入技能页签
// (但**不含**三项高级设置——按 2026-10-09 拍板删除其 UI,DB 列与导入字段保留兼容);
// 插件的「角色卡内嵌插件」检测面随之退役(kedai 已内置实现,无需用户操作)。
import { computed, onMounted, ref } from 'vue';
import { storeToRefs } from 'pinia';
import { useAppStore } from '../store';
import * as api from '../api';
import type { FolderNode } from '../utils/assetFolders';
import {
  buildFolderTree,
  entriesInFolder,
  listFolderPaths,
  normalizeFolderPath,
  validateFolderPath,
} from '../utils/assetFolders';

type TabKey = 'skills' | 'flows' | 'plugins' | 'regex';

const store = useAppStore();
const { skills, agentFlowLibrary, regexScripts } = storeToRefs(store);

const close = (): void => {
  store.assetsOpen = false;
};

const tab = ref<TabKey>('skills');
/** 每个页签各自记忆选中的文件夹(空串 = 未分组;仅 skills/flows/plugins 用) */
const selectedFolder = ref<Record<TabKey, string>>({ skills: '', flows: '', plugins: '', regex: '' });
const msg = ref<{ kind: 'ok' | 'err'; text: string } | null>(null);
const busy = ref(false);

function flash(kind: 'ok' | 'err', text: string): void {
  msg.value = { kind, text };
}

// ===== 插件数据(本地)=====
// 后端把「已注册工具」与「磁盘文件」分两个清单下发;附件管理以**文件**为条目,
// folder 只挂在工具上(同一文件的多条工具 folder 恒等),故按下标合成带 folder 的条目。
interface PluginEntry {
  file: string;
  folder: string;
  tools: api.ToolPluginInfo[];
}
const pluginTools = ref<api.ToolPluginInfo[]>([]);
const pluginFiles = ref<string[]>([]);

const pluginEntries = computed<PluginEntry[]>(() => {
  const byFile = new Map<string, api.ToolPluginInfo[]>();
  for (const t of pluginTools.value) {
    const list = byFile.get(t.file);
    if (list) list.push(t);
    else byFile.set(t.file, [t]);
  }
  const files = new Set<string>([...pluginFiles.value, ...byFile.keys()]);
  return [...files]
    .sort((a, b) => a.localeCompare(b))
    .map((file) => {
      const tools = byFile.get(file) ?? [];
      return { file, folder: normalizeFolderPath(tools[0]?.folder), tools };
    });
});

// ===== 文件夹树(按当前页签切换数据源;正则页签无 folder,不渲染树) =====
const folderEntries = computed<{ folder?: string }[]>(() => {
  switch (tab.value) {
    case 'skills':
      return skills.value;
    case 'flows':
      return agentFlowLibrary.value?.flows ?? [];
    case 'plugins':
      return pluginEntries.value;
    default:
      return [];
  }
});
const folderTree = computed(() => buildFolderTree(folderEntries.value));

/** 把嵌套树拍平成带缩进的行(模板里递归渲染不便,拍平后一次 v-for) */
interface FolderRow {
  path: string;
  name: string;
  depth: number;
  count: number;
  total: number;
}
function flatten(nodes: FolderNode[], depth = 0): FolderRow[] {
  const out: FolderRow[] = [];
  for (const n of nodes) {
    out.push({ path: n.path, name: n.name, depth, count: n.count, total: n.total });
    out.push(...flatten(n.children, depth + 1));
  }
  return out;
}
const folderRows = computed(() => flatten(folderTree.value.folders));
const folderPaths = computed(() => listFolderPaths(folderEntries.value));
const activeFolder = computed(() => selectedFolder.value[tab.value]);
function selectFolder(path: string): void {
  selectedFolder.value[tab.value] = path;
}

const skillList = computed(() => entriesInFolder(skills.value, activeFolder.value));
const flowList = computed(() => entriesInFolder(agentFlowLibrary.value?.flows ?? [], activeFolder.value));
const pluginList = computed(() => entriesInFolder(pluginEntries.value, activeFolder.value));

// ===== 「移动到…」下拉 =====
// 选中「新建…」时就地弹 prompt 拿路径(校验复用 assetFolders.validateFolderPath),
// 处理后把 select 复位成占位项,便于对同一目标再操作一次。
/** 移动动作是「就地发起、由 run() 统一回报」的 fire-and-forget,故回调返回 void 而非 Promise */
function onMoveSelect(evt: Event, doMove: (folder: string) => void): void {
  const el = evt.target as HTMLSelectElement;
  const value = el.value;
  el.value = '';
  if (!value) return;
  if (value === '__root__') {
    doMove('');
    return;
  }
  if (value === '__new__') {
    const input = window.prompt('新建文件夹路径(用 / 分隔层级;留空取消)', '');
    if (input === null) return;
    const err = validateFolderPath(input);
    if (err) {
      flash('err', err);
      return;
    }
    const path = normalizeFolderPath(input);
    if (!path) return;
    doMove(path);
    return;
  }
  doMove(value);
}

// ===== 技能页签 =====
const editingSkillId = ref<string | null>(null);
const skillDraft = ref({ name: '', description: '', content: '' });

function toggleSkillEdit(s: api.SkillRecord): void {
  editingSkillId.value = editingSkillId.value === s.id ? null : s.id;
  if (editingSkillId.value === s.id) {
    skillDraft.value = { name: s.name, description: s.description, content: s.content };
  }
}

async function run(action: () => Promise<void>, okText: string): Promise<void> {
  busy.value = true;
  msg.value = null;
  try {
    await action();
    flash('ok', okText);
  } catch (e) {
    flash('err', (e as Error).message);
  } finally {
    busy.value = false;
  }
}

function toggleSkill(s: api.SkillRecord): void {
  void run(async () => {
    await api.updateSkill(s.id, { enabled: !s.enabled });
    await store.loadSkills();
  }, `已${s.enabled ? '停用' : '启用'}技能「${s.name}」`);
}

function saveSkill(s: api.SkillRecord): void {
  void run(async () => {
    await api.updateSkill(s.id, {
      name: skillDraft.value.name,
      description: skillDraft.value.description,
      content: skillDraft.value.content,
    });
    await store.loadSkills();
    editingSkillId.value = null;
  }, `已保存技能「${skillDraft.value.name}」`);
}

function deleteSkill(s: api.SkillRecord): void {
  if (!window.confirm(`确定删除技能「${s.name}」?`)) return;
  void run(async () => {
    await api.deleteSkill(s.id);
    await store.loadSkills();
  }, `已删除技能「${s.name}」`);
}

function moveSkill(s: api.SkillRecord, folder: string): void {
  void run(async () => {
    await api.updateSkill(s.id, { folder });
    await store.loadSkills();
  }, `已移动技能「${s.name}」`);
}

// ===== 流程页签 =====
// 说明:后端的 PUT /api/agent-flows 语义是「保存并设为当前选中」,没有单独的「只改启停」端点;
// 故这里切启用态会顺带把该流程设为当前(整理分组用 move 端点,那个不动当前选中)。
function toggleFlow(f: api.AgentFlowConfig): void {
  void run(async () => {
    await store.saveAgentFlowConfig({ ...f, enabled: !f.enabled });
  }, `已${f.enabled ? '停用' : '启用'}流程「${f.name || '未命名'}」`);
}

function deleteFlow(f: api.AgentFlowConfig): void {
  if (!f.id) return;
  if (!window.confirm(`确定删除流程「${f.name || '未命名'}」?该操作不可撤销。`)) return;
  void run(async () => {
    await store.deleteAgentFlow(f.id as string);
  }, `已删除流程「${f.name || '未命名'}」`);
}

function moveFlow(f: api.AgentFlowConfig, folder: string): void {
  if (!f.id) return;
  void run(async () => {
    await api.moveAgentFlow(f.id as string, folder);
    await store.loadAgentFlow();
  }, `已移动流程「${f.name || '未命名'}」`);
}

/** 流程的步骤编辑在「综合设置 → Agent 与任务 → 执行流程」;这里只做组织,跳过去编辑 */
function editFlow(): void {
  store.assetsOpen = false;
  store.openSettingsAt('flow');
}

// ===== 插件页签 =====
async function loadPlugins(): Promise<void> {
  const status = await api.listPluginTools();
  pluginTools.value = status.tools;
  pluginFiles.value = status.files;
}

function movePlugin(entry: PluginEntry, folder: string): void {
  void run(async () => {
    await api.movePluginTool(entry.file, folder);
    await loadPlugins();
  }, `已移动插件「${entry.file}」`);
}

function deletePlugin(entry: PluginEntry): void {
  if (!window.confirm(`确定删除插件文件「${entry.file}」?其对应工具将不再可用。`)) return;
  void run(async () => {
    await api.deletePluginTool(entry.file);
    await loadPlugins();
  }, `已删除插件「${entry.file}」`);
}

function reloadPlugins(): void {
  void run(async () => {
    const r = await api.reloadPluginTools();
    await loadPlugins();
    if (!r.ok) throw new Error(`重载部分失败:${(r.errors ?? []).join('; ')}`);
    flash('ok', `已重载 ${r.loaded} 个,注销 ${r.removed ?? 0} 个`);
  }, '插件已重载');
}

// ===== 预设正则页签(扁平;无 folder)=====
function toggleRegex(r: api.RegexScript): void {
  const next = regexScripts.value.map((x) => (x.id === r.id ? { ...x, enabled: !x.enabled } : x));
  void run(async () => {
    await store.saveRegexScripts(next);
  }, `已${r.enabled ? '停用' : '启用'}正则「${r.script_name}」`);
}

function deleteRegex(r: api.RegexScript): void {
  if (!window.confirm(`确定删除正则「${r.script_name}」?`)) return;
  const next = regexScripts.value.filter((x) => x.id !== r.id);
  void run(async () => {
    await store.saveRegexScripts(next);
  }, `已删除正则「${r.script_name}」`);
}

onMounted(() => {
  // 四份数据都拉一次:页签切换是纯前端过滤,不值得每切一次再请求一轮。
  // 正则库的失败不挡其余页签(ensureRegexScriptsLoaded 同样吞错,这里显式 catch)。
  void store.loadSkills();
  void store.loadAgentFlow();
  void loadPlugins().catch((e) => flash('err', `插件加载失败:${(e as Error).message}`));
  void store.loadRegexScripts().catch(() => {});
});
</script>

<template>
  <div class="sv-modal-mask" @click.self="close">
    <div class="sv-modal lg">
      <div class="sv-modal-head">
        <h2 class="flex items-center gap-2">
          <span class="sv-supreme pink" /> 附件管理
        </h2>
        <button class="sv-btn ghost sv-btn-square" @click="close">✕</button>
      </div>

      <div class="sv-modal-body">
        <!-- 页签 -->
        <div class="sv-mode" style="margin-bottom: var(--space-3)">
          <button :class="{ active: tab === 'skills' }" @click="tab = 'skills'">技能</button>
          <button :class="{ active: tab === 'flows' }" @click="tab = 'flows'">流程</button>
          <button :class="{ active: tab === 'plugins' }" @click="tab = 'plugins'">插件</button>
          <button :class="{ active: tab === 'regex' }" @click="tab = 'regex'">预设正则</button>
        </div>

        <div v-if="msg" class="sv-feedback" :class="msg.kind === 'err' ? 'err' : 'ok'">{{ msg.text }}</div>

        <!-- 文件夹树(技能 / 流程 / 插件共用;正则无 folder 不渲染) -->
        <div v-if="tab !== 'regex'" class="asset-grid">
          <div class="asset-folders">
            <button
              class="asset-folder"
              :class="{ active: activeFolder === '' }"
              @click="selectFolder('')"
            >
              <span class="asset-folder-name">未分组</span>
              <span class="asset-folder-count">{{ folderTree.ungrouped }}</span>
            </button>
            <button
              v-for="row in folderRows"
              :key="row.path"
              class="asset-folder"
              :class="{ active: activeFolder === row.path }"
              :style="{ paddingLeft: `calc(var(--space-2-5) + ${row.depth * 12}px)` }"
              @click="selectFolder(row.path)"
            >
              <span class="asset-folder-name">{{ row.name }}</span>
              <span class="asset-folder-count">{{ row.total }}</span>
            </button>
            <p v-if="folderRows.length === 0 && folderTree.ungrouped === 0" class="sv-note" style="margin: var(--space-2) 0">
              暂无条目
            </p>
          </div>

          <div class="asset-list">
            <!-- ===== 技能列表 ===== -->
            <template v-if="tab === 'skills'">
              <div v-for="s in skillList" :key="s.id" class="asset-item">
                <div class="asset-item-main">
                  <div class="sv-char-name">
                    {{ s.name }}
                    <span v-if="!s.enabled" class="sv-badge-off">已停用</span>
                  </div>
                  <div class="sv-char-desc">{{ s.description || '无描述' }}</div>
                </div>
                <div class="asset-item-actions">
                  <select class="sv-select asset-move" @change="onMoveSelect($event, (f) => moveSkill(s, f))">
                    <option value="">移动到…</option>
                    <option value="__root__">未分组</option>
                    <option v-for="p in folderPaths" :key="p" :value="p">{{ p }}</option>
                    <option value="__new__">＋ 新建…</option>
                  </select>
                  <button class="sv-btn ghost sv-btn-sm" :disabled="busy" @click="toggleSkill(s)">
                    {{ s.enabled ? '停用' : '启用' }}
                  </button>
                  <button class="sv-btn ghost sv-btn-sm" :disabled="busy" @click="toggleSkillEdit(s)">
                    {{ editingSkillId === s.id ? '收起' : '编辑' }}
                  </button>
                  <button class="sv-btn danger sv-btn-sm" :disabled="busy" @click="deleteSkill(s)">删除</button>
                </div>
                <div v-if="editingSkillId === s.id" class="asset-edit">
                  <label class="sv-inp-row">
                    <span class="sv-inp-tag">名称</span>
                    <input v-model="skillDraft.name" class="sv-input" spellcheck="false" />
                  </label>
                  <label class="sv-inp-row">
                    <span class="sv-inp-tag">描述</span>
                    <input v-model="skillDraft.description" class="sv-input" spellcheck="false" />
                  </label>
                  <label class="sv-inp-row">
                    <span class="sv-inp-tag">内容</span>
                    <textarea v-model="skillDraft.content" class="sv-input asset-textarea" spellcheck="false" />
                  </label>
                  <div class="sv-btn-row">
                    <button class="sv-btn primary sv-btn-sm" :disabled="busy" @click="saveSkill(s)">
                      {{ busy ? '保存中...' : '保存' }}
                    </button>
                  </div>
                </div>
              </div>
              <p v-if="skillList.length === 0" class="sv-note">该文件夹下没有技能。</p>
            </template>

            <!-- ===== 流程列表 ===== -->
            <template v-else-if="tab === 'flows'">
              <div v-for="f in flowList" :key="f.id" class="asset-item">
                <div class="asset-item-main">
                  <div class="sv-char-name">
                    {{ f.name || '未命名流程' }}
                    <span v-if="!f.enabled" class="sv-badge-off">未启用</span>
                  </div>
                  <div class="sv-char-desc">{{ f.description || '无说明' }} · {{ f.steps.length }} 步</div>
                </div>
                <div class="asset-item-actions">
                  <select class="sv-select asset-move" @change="onMoveSelect($event, (fo) => moveFlow(f, fo))">
                    <option value="">移动到…</option>
                    <option value="__root__">未分组</option>
                    <option v-for="p in folderPaths" :key="p" :value="p">{{ p }}</option>
                    <option value="__new__">＋ 新建…</option>
                  </select>
                  <button class="sv-btn ghost sv-btn-sm" :disabled="busy" @click="toggleFlow(f)">
                    {{ f.enabled ? '停用' : '启用' }}
                  </button>
                  <button class="sv-btn ghost sv-btn-sm" @click="editFlow">编辑</button>
                  <button class="sv-btn danger sv-btn-sm" :disabled="busy" @click="deleteFlow(f)">删除</button>
                </div>
              </div>
              <p v-if="flowList.length === 0" class="sv-note">该文件夹下没有流程。</p>
            </template>

            <!-- ===== 插件列表 ===== -->
            <template v-else-if="tab === 'plugins'">
              <div class="sv-btn-row" style="margin-bottom: var(--space-2)">
                <button class="sv-btn ghost sv-btn-sm" :disabled="busy" @click="reloadPlugins">重载插件</button>
                <span class="sv-note">插件文件导入请走「综合导入 / 导出」的 ⑦ 段。</span>
              </div>
              <div v-for="p in pluginList" :key="p.file" class="asset-item">
                <div class="asset-item-main">
                  <div class="sv-char-name asset-mono">{{ p.file }}</div>
                  <div class="sv-char-desc">
                    {{ p.tools.length > 0 ? p.tools.map((t) => t.name).join('、') : '未注册工具(语法错误或已停用)' }}
                  </div>
                </div>
                <div class="asset-item-actions">
                  <select class="sv-select asset-move" @change="onMoveSelect($event, (fo) => movePlugin(p, fo))">
                    <option value="">移动到…</option>
                    <option value="__root__">未分组</option>
                    <option v-for="path in folderPaths" :key="path" :value="path">{{ path }}</option>
                    <option value="__new__">＋ 新建…</option>
                  </select>
                  <button class="sv-btn danger sv-btn-sm" :disabled="busy" @click="deletePlugin(p)">删除</button>
                </div>
              </div>
              <p v-if="pluginList.length === 0" class="sv-note">该文件夹下没有插件文件。</p>
            </template>
          </div>
        </div>

        <!-- ===== 预设正则(扁平列表,无文件夹)===== -->
        <div v-else class="asset-list">
          <p class="sv-note" style="margin-top: 0">
            全局正则：跨角色生效的显示层正则(全局先、卡级后)。发送层清洗不在此范围。
          </p>
          <div v-for="r in regexScripts" :key="r.id" class="asset-item">
            <div class="asset-item-main">
              <div class="sv-char-name">
                {{ r.script_name }}
                <span v-if="!r.enabled" class="sv-badge-off">已停用</span>
              </div>
              <div class="sv-char-desc asset-mono">/{{ r.find_regex }}/ → {{ r.replace_string || '(删除)' }}</div>
            </div>
            <div class="asset-item-actions">
              <button class="sv-btn ghost sv-btn-sm" :disabled="busy" @click="toggleRegex(r)">
                {{ r.enabled ? '停用' : '启用' }}
              </button>
              <button class="sv-btn danger sv-btn-sm" :disabled="busy" @click="deleteRegex(r)">删除</button>
            </div>
          </div>
          <p v-if="regexScripts.length === 0" class="sv-note">全局正则库为空(可经综合导入 ⑥ 段导入)。</p>
        </div>
      </div>

      <div class="sv-modal-foot">
        <button class="sv-btn primary" @click="close">完成</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* 附件管理布局(MAINTENANCE D-5:新增组件样式进 scoped,不进 style.css) */
.asset-grid {
  display: grid;
  grid-template-columns: 180px 1fr;
  gap: var(--space-3);
  align-items: start;
}
.asset-folders {
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
  border-right: var(--bw-thin) solid var(--sv-line);
  padding-right: var(--space-2);
  max-height: 52vh;
  overflow-y: auto;
}
.asset-folder {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  width: 100%;
  padding: var(--space-1) var(--space-2-5);
  border: var(--bw-thin) solid transparent;
  background: transparent;
  font-size: var(--text-sm);
  color: var(--sv-ink-dim);
  text-align: left;
  cursor: pointer;
}
.asset-folder:hover {
  border-color: var(--sv-pink);
  color: var(--sv-ink);
}
.asset-folder.active {
  background: var(--sv-ink);
  color: var(--sv-white);
  border-color: var(--sv-ink);
}
.asset-folder-name {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.asset-folder-count {
  flex: none;
  font-size: var(--text-2xs);
  opacity: 0.7;
}
.asset-list {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
  max-height: 52vh;
  overflow-y: auto;
}
.asset-item {
  display: grid;
  grid-template-columns: 1fr auto;
  gap: var(--space-1-5) var(--space-2);
  border: var(--bw-thin) solid var(--sv-line);
  padding: var(--space-2) var(--space-2-5);
}
.asset-item-main {
  min-width: 0;
}
.asset-item-actions {
  display: flex;
  align-items: center;
  gap: var(--space-1-5);
  flex-wrap: wrap;
  justify-content: flex-end;
}
.asset-move {
  max-width: 130px;
  font-size: var(--text-xs);
}
.asset-edit {
  grid-column: 1 / -1;
  display: flex;
  flex-direction: column;
  gap: var(--space-1-5);
  border-top: var(--bw-thin) solid var(--sv-line);
  padding-top: var(--space-2);
}
.asset-textarea {
  min-height: 80px;
  resize: vertical;
  font-family: inherit;
}
.asset-mono {
  font-family: ui-monospace, Consolas, monospace;
}
</style>
