// 角色 store:角色列表 CRUD、当前角色选择、搜索过滤、开场白派生、角色卡脚本授权。
// 从 store.ts 按领域拆分。跨 store 引用(切角色时重置 chat 会话状态 /
// 同步 uiPrefs 的 HTML 渲染开关)均在动作运行时解析,setup 阶段不实例化其他 store。
import { defineStore } from 'pinia';
import { computed, ref } from 'vue';
import * as api from '../api';
import { idleAgent } from '../sseReducer';
import {
  LocalScriptAuthorizationStore,
  hashRegexScripts,
  type ScriptAuthorizationGrant,
} from '../scriptAuthorization';
import { useChatStore } from './chat';
import { useUiPrefsStore } from './uiPrefs';

export const useCharacterStore = defineStore('app.character', () => {
  // ===== 状态 =====
  const characters = ref<api.CharacterRecord[]>([]);
  const currentCharacterId = ref<string | null>(null);
  const searchQuery = ref('');

  const scriptAuthorizationStore = new LocalScriptAuthorizationStore(localStorage);
  /** 当前角色脚本内容哈希；角色或脚本变化时重算。 */
  const currentScriptHash = ref('');
  /**
   * 授权变更信号:grant/revoke/删卡时递增,驱动依赖 localStorage 的
   * currentScriptAuthorized 等 computed 重算。scriptAuthorizationStore 本身
   * 不是响应式对象,若不引入此信号,授权后按钮/面板不会刷新。
   */
  const scriptAuthVersion = ref(0);
  /** 授权列表快照，供设置页查看与逐卡撤销。 */
  const scriptAuthorizations = ref<ScriptAuthorizationGrant[]>(scriptAuthorizationStore.list());

  // ===== 派生状态 =====
  const currentCharacter = computed(() =>
    characters.value.find((c) => c.id === currentCharacterId.value) ?? null,
  );
  /**
   * 当前角色全部开场(多开场切换用):主开场 first_mes + 备用开场 alternate_greetings,均去空。
   * 空数组 = 无开场;下标即 greeting_index(0=主开场,1..=备用)。
   */
  const currentGreetings = computed<string[]>(() => {
    const c = currentCharacter.value;
    if (!c) return [];
    const list: string[] = [];
    if (c.first_mes?.trim()) list.push(c.first_mes);
    for (const g of c.alternate_greetings ?? []) {
      if (g.trim()) list.push(g);
    }
    return list;
  });
  const currentCharacterName = computed(
    () => currentCharacter.value?.chara_name ?? currentCharacter.value?.name ?? '未选择角色',
  );
  const currentScriptAuthorized = computed(() => {
    const id = currentCharacterId.value;
    // 引用授权变更信号,确保 grant/revoke 后本 computed 重新求值
    void scriptAuthVersion.value;
    return !!id && !!currentScriptHash.value && scriptAuthorizationStore.isAuthorized(id, currentScriptHash.value);
  });
  /** 角色列表按搜索词过滤 */
  const filteredCharacters = computed(() => {
    const q = searchQuery.value.trim().toLowerCase();
    if (!q) return characters.value;
    return characters.value.filter((c) =>
      [c.chara_name, c.name, c.description].join(' ').toLowerCase().includes(q),
    );
  });

  // ===== 动作 =====
  async function loadCharacters(autoSelect = true): Promise<void> {
    try {
      characters.value = await api.listCharacters();
      if (currentCharacterId.value && !characters.value.some((c) => c.id === currentCharacterId.value)) {
        currentCharacterId.value = null;
        const chat = useChatStore();
        chat.currentSessionId = null;
        chat.sessions = [];
        chat.messages = [];
      }
      if (autoSelect && !currentCharacterId.value && characters.value.length > 0) {
        await selectCharacter(characters.value[0].id);
      }
    } catch (e) {
      console.error('加载角色失败', e);
    }
  }

  async function selectCharacter(id: string): Promise<void> {
    currentCharacterId.value = id;
    // HTML 渲染开关按角色卡记忆:有记忆用记忆,无记忆回退全局默认
    useUiPrefsStore().syncRenderHtmlToCurrent();
    const selected = characters.value.find((c) => c.id === id);
    // 无条件计算脚本哈希:无脚本卡也得到合法哈希(空数组 SHA-256),使
    // currentScriptAuthorized 与 authorizeCurrentCharacterScripts 对无脚本卡同样生效,
    // 顶栏 JS 授权按钮常驻可授权/撤销;执行层因脚本数组为空永不执行,无安全影响。
    currentScriptHash.value = await hashRegexScripts(selected?.regex_scripts ?? []);
    const chat = useChatStore();
    chat.currentSessionId = null;
    chat.sessions = [];
    chat.messages = [];
    chat.lastUsage = null;
    chat.agent = idleAgent();
    chat.mvuVariables = { stat_data: {}, display_data: {} };
    // 加载该角色 [InitVar] 初始变量条目(mvu 初始化数据)
    try {
      chat.initVarEntries = await api.fetchInitVars(id);
    } catch {
      chat.initVarEntries = {};
    }
    try {
      await chat.loadSessions(id);
      if (chat.sessions.length > 0) {
        chat.currentSessionId = chat.sessions[0].id;
        await chat.loadHistory(chat.sessions[0].id);
      } else if (characters.value.find((c) => c.id === id)?.first_mes) {
        // 角色带开场白:自动新建会话,由后端注入开场白并显示
        await chat.newSession();
      }
    } catch (e) {
      console.error('加载会话失败', e);
    }
  }

  async function uploadCharacter(file: File): Promise<api.CharacterRecord> {
    const record = await api.uploadCharacter(file);
    await loadCharacters(false);
    await selectCharacter(record.id);
    return record;
  }

  async function deleteCharacter(id: string): Promise<void> {
    await api.deleteCharacter(id);
    scriptAuthorizationStore.revoke(id);
    scriptAuthorizations.value = scriptAuthorizationStore.list();
    scriptAuthVersion.value += 1;
    // 一并清理该角色卡的 HTML 渲染开关记忆(随卡删除,不留孤儿数据)
    useUiPrefsStore().removeRenderHtmlOverride(id);
    await loadCharacters();
  }

  function authorizeCurrentCharacterScripts(): void {
    const id = currentCharacterId.value;
    if (!id || !currentScriptHash.value) return;
    scriptAuthorizationStore.grant(id, currentScriptHash.value);
    scriptAuthorizations.value = scriptAuthorizationStore.list();
    scriptAuthVersion.value += 1;
  }

  function revokeCharacterScripts(characterId: string): void {
    scriptAuthorizationStore.revoke(characterId);
    scriptAuthorizations.value = scriptAuthorizationStore.list();
    scriptAuthVersion.value += 1;
  }

  function isCharacterScriptAuthorized(characterId: string, scriptHash: string): boolean {
    return scriptAuthorizationStore.isAuthorized(characterId, scriptHash);
  }

  /** 更新角色提示词/开场白(description/first_mes/alternate_greetings),并同步本地列表 */
  async function updateCharacterPrompt(
    id: string,
    patch: { description?: string; first_mes?: string; alternate_greetings?: string[] },
  ): Promise<void> {
    const updated = await api.updateCharacter(id, patch);
    const idx = characters.value.findIndex((c) => c.id === id);
    if (idx !== -1) {
      characters.value[idx].description = updated.description;
      characters.value[idx].data_raw = updated.data_raw;
      if (updated.first_mes !== undefined) characters.value[idx].first_mes = updated.first_mes;
      if (updated.alternate_greetings !== undefined) {
        characters.value[idx].alternate_greetings = updated.alternate_greetings;
      }
    }
  }

  return {
    characters,
    currentCharacterId,
    searchQuery,
    currentScriptHash,
    scriptAuthorizations,
    currentCharacter,
    currentGreetings,
    currentCharacterName,
    currentScriptAuthorized,
    filteredCharacters,
    loadCharacters,
    selectCharacter,
    uploadCharacter,
    deleteCharacter,
    authorizeCurrentCharacterScripts,
    revokeCharacterScripts,
    isCharacterScriptAuthorized,
    updateCharacterPrompt,
  };
});
