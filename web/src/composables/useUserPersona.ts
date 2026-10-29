// 用户设定(USERPERSONA-1):角色扮演专属的全局用户角色配置。
// 模块级 config 供显示层只读(ChatMessageItem / scriptRunner),draft 供设置区编辑;
// 保存走 API,不直写 Pinia store(check-arch 直写基线)。
import { computed, ref } from 'vue';
import * as api from '../api';

const DEFAULT_CONFIG: api.UserPersonaConfig = { enabled: false, name: '', description: '' };

/** 已保存配置(显示层只读) */
const config = ref<api.UserPersonaConfig>({ ...DEFAULT_CONFIG });
/** 设置区草稿(保存时才提交) */
const draft = ref<api.UserPersonaConfig>({ ...DEFAULT_CONFIG });
const loading = ref(false);
const saving = ref(false);
const error = ref('');
let loaded = false;

function clone(c: api.UserPersonaConfig): api.UserPersonaConfig {
  return { enabled: c.enabled, name: c.name, description: c.description };
}

/** 从服务端加载;失败时保留当前配置并记录错误(不抛,避免阻塞聊天) */
async function load(): Promise<boolean> {
  if (loading.value) return false;
  loading.value = true;
  error.value = '';
  try {
    const c = await api.getUserPersona();
    config.value = clone(c);
    draft.value = clone(c);
    loaded = true;
    return true;
  } catch (e) {
    error.value = `读取失败:${(e as Error).message}`;
    return false;
  } finally {
    loading.value = false;
  }
}

/** 启动/首次使用时的懒加载:失败静默回退默认,不阻塞聊天 */
async function ensureLoaded(): Promise<void> {
  if (loaded || loading.value) return;
  await load();
}

/** 保存草稿;成功后同步 config;失败保留 draft 并给出错误 */
async function save(): Promise<boolean> {
  if (saving.value) return false;
  saving.value = true;
  error.value = '';
  try {
    const c = await api.saveUserPersona(clone(draft.value));
    config.value = clone(c);
    draft.value = clone(c);
    loaded = true;
    return true;
  } catch (e) {
    error.value = `保存失败:${(e as Error).message}`;
    return false;
  } finally {
    saving.value = false;
  }
}

/** 清空草稿(仍需保存才落库) */
function resetDraft(): void {
  draft.value = { ...DEFAULT_CONFIG };
}

/**
 * 显示层称呼:仅「已启用 + 称呼非空」时用称呼,否则回退「用户」。
 * 判据必须与后端 `UserPersonaConfig::user_name()` 一致——否则关闭开关但保留文本时,
 * 后端 content_display 展开成「用户」、前端消息渲染展开成称呼,同一条消息两种称呼。
 */
const userName = computed(() =>
  config.value.enabled && config.value.name.trim() ? config.value.name.trim() : '用户',
);
/** 显示层用户设定:仅启用时提供 */
const userDescription = computed(() =>
  config.value.enabled ? config.value.description : '',
);

export function useUserPersona() {
  return {
    config,
    draft,
    loading,
    saving,
    error,
    userName,
    userDescription,
    load,
    ensureLoaded,
    save,
    resetDraft,
  };
}
