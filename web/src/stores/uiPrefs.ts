// 界面偏好 store:各弹窗/面板开关、启动动画、安全 HTML 渲染开关(按角色卡记忆 + 全局默认)。
// 从 store.ts 按领域拆分。跨 store 引用(currentCharacterId / queueSettingsSave)均在
// 回调/动作运行时解析,setup 阶段不实例化其他 store,避免初始化环。
import { defineStore } from 'pinia';
import { ref, watch } from 'vue';
import { LocalRenderHtmlPreferenceStore } from '../renderHtmlPreference';
import { useCharacterStore } from './character';
import { useGenSettingsStore } from './genSettings';

export const useUiPrefsStore = defineStore('app.uiPrefs', () => {
  // ===== 弹窗/面板开关 =====
  const settingsOpen = ref(false);
  /** 提示词顺序管理面板开关 */
  const promptsOpen = ref(false);
  const agentDockOpen = ref(true);
  /** 右侧 Agent 面板开关(默认收起) */
  const agentPanelOpen = ref(false);
  const worldBooksOpen = ref(false);
  const quickRepliesOpen = ref(false);
  const audioOpen = ref(false);
  /** 插件管理弹窗开关 */
  const pluginsOpen = ref(false);
  const skillsOpen = ref(false);
  const contractsOpen = ref(false);
  const scriptsOpen = ref(false);
  const macrosOpen = ref(false);
  const eventsOpen = ref(false);
  const optimizeOpen = ref(false);
  /** 聊天记录面板开关 */
  const chatRecordsOpen = ref(false);
  /** 启动动画是否完成 */
  const splashDone = ref(false);

  // ===== 安全 HTML 渲染开关 =====
  /**
   * 消息 HTML 渲染开关(只控制安全 HTML,与 JavaScript 授权独立)。
   * 按角色卡记忆:每张卡在 localStorage 记录各自的开关,无记忆时回退全局默认
   * (settings.json 的 render_html)。当前 ref 恒为「当前角色卡的生效值」。
   */
  const renderHtml = ref(false);
  /** 全局默认 HTML 渲染开关(设置面板;无角色卡记忆时生效) */
  const defaultRenderHtml = ref(false);
  /** 每张角色卡的 HTML 渲染开关记忆(characterId -> boolean) */
  const renderHtmlPreferenceStore = new LocalRenderHtmlPreferenceStore(localStorage);
  const renderHtmlOverrides = ref<Record<string, boolean>>(renderHtmlPreferenceStore.read());

  /**
   * 按当前角色卡同步 HTML 渲染开关生效值:有角色卡记忆用记忆,否则用全局默认。
   * 程序性赋值走 restoring 闩锁,不触发持久化写入(避免为默认值凭空生成记忆)。
   */
  function syncRenderHtmlToCurrent(): void {
    const cid = useCharacterStore().currentCharacterId;
    const v = cid && cid in renderHtmlOverrides.value
      ? renderHtmlOverrides.value[cid]
      : defaultRenderHtml.value;
    if (renderHtml.value === v) return;
    if (renderHtmlSaveTimer) {
      clearTimeout(renderHtmlSaveTimer);
      renderHtmlSaveTimer = null;
    }
    restoringRenderHtml = true;
    renderHtml.value = v;
  }

  /** 删除角色卡时清理其 HTML 渲染开关记忆(随卡删除,不留孤儿数据) */
  function removeRenderHtmlOverride(characterId: string): void {
    if (!(characterId in renderHtmlOverrides.value)) return;
    const next = { ...renderHtmlOverrides.value };
    delete next[characterId];
    renderHtmlOverrides.value = next;
    renderHtmlPreferenceStore.write(next);
  }

  // renderHtml 变更时自动持久化(节流:300ms 内不重复保存)。
  // 有当前角色卡 → 写入该卡的 localStorage 记忆(不碰全局设置);
  // 无角色卡 → 写入全局默认(settings.json render_html)。
  let renderHtmlSaveTimer: ReturnType<typeof setTimeout> | null = null;
  let confirmedRenderHtml = renderHtml.value;
  let restoringRenderHtml = false;
  watch(renderHtml, (v) => {
    if (restoringRenderHtml) {
      restoringRenderHtml = false;
      return;
    }
    if (renderHtmlSaveTimer) clearTimeout(renderHtmlSaveTimer);
    renderHtmlSaveTimer = setTimeout(() => {
      const cid = useCharacterStore().currentCharacterId;
      if (cid) {
        // 按角色卡记忆:仅写 localStorage,不改全局默认
        const next = { ...renderHtmlOverrides.value, [cid]: v };
        renderHtmlOverrides.value = next;
        renderHtmlPreferenceStore.write(next);
        confirmedRenderHtml = v;
      } else {
        void useGenSettingsStore().queueSettingsSave({ render_html: v })
          .then(() => { confirmedRenderHtml = v; })
          .catch((error) => {
            console.error('HTML 渲染设置保存失败', error);
            restoringRenderHtml = true;
            renderHtml.value = confirmedRenderHtml;
          });
      }
    }, 300);
  });

  return {
    settingsOpen,
    promptsOpen,
    agentDockOpen,
    agentPanelOpen,
    worldBooksOpen,
    quickRepliesOpen,
    audioOpen,
    pluginsOpen,
    skillsOpen,
    contractsOpen,
    scriptsOpen,
    macrosOpen,
    eventsOpen,
    optimizeOpen,
    chatRecordsOpen,
    splashDone,
    renderHtml,
    defaultRenderHtml,
    renderHtmlOverrides,
    syncRenderHtmlToCurrent,
    removeRenderHtmlOverride,
  };
});
