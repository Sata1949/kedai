// 多套连接配置(批次 4):列表载入、行内编辑草稿、新增/删除/设为默认/启停与保存。
//
// 保存走既有 store.saveSettings 通道(与 useApiSettings 同一路径,patch 里多带
// connections / active_connection_id),不新增端点;语言与错误提示同 useApiSettings。
import { computed, ref } from 'vue';
import { useAppStore } from '../store';
import * as api from '../api';
import type {
  ConnectionProfile,
  ConnectionProfilePatch,
  RuntimeSettings,
  RuntimeSettingsPatch,
} from '../api/types';
import type { ConnectionProbeParams } from '../api/settings';

/** 行内编辑草稿;id 为空 = 后端尚未分配(新增行,保存后才拿到 id) */
export interface ConnectionDraft {
  id: string;
  name: string;
  connector_type: string;
  base_url: string;
  model: string;
  /** 接口方言(取值域见 api/types.ts 的 ApiStyle;后端保存时会按 URL 后缀推断默认档) */
  api_style: string;
  /** 只写字段:留空 = 不改动该连接的密钥(与后端「空 Key 忽略」同口径);
   *  要清除已保存密钥,置 clearApiKey(瞬态)后保存 */
  api_key: string;
  /** 瞬态清除标记(不落服务端字段名):置 true 且 api_key 留空时,保存会下发
   *  clear_api_key=true 显式清空该连接已保存密钥(2026-10-03 API 设置补全) */
  clearApiKey: boolean;
  enabled: boolean;
  /** 模型能力位(2026-10-02 视觉能力包;2026-10-03 VISION-L6 收口:前缀续写仍为预留) */
  supports_vision: boolean;
  supports_structured_output: boolean;
  supports_prefix_completion: boolean;
  supports_mid_conversation_system: boolean;
  image_auto_split: boolean;
  /** 服务端密钥掩码(仅展示用) */
  api_key_masked: string;
  has_api_key: boolean;
}

function toDraft(p: ConnectionProfile): ConnectionDraft {
  return {
    id: p.id,
    name: p.name,
    connector_type: p.connector_type,
    base_url: p.base_url,
    model: p.model,
    api_style: p.api_style ?? 'chat-completions',
    api_key: '',
    clearApiKey: false,
    enabled: p.enabled,
    supports_vision: p.supports_vision ?? false,
    supports_structured_output: p.supports_structured_output ?? false,
    supports_prefix_completion: p.supports_prefix_completion ?? false,
    supports_mid_conversation_system: p.supports_mid_conversation_system ?? false,
    image_auto_split: p.image_auto_split ?? false,
    api_key_masked: p.api_key_masked ?? '',
    has_api_key: p.has_api_key ?? false,
  };
}

export function useConnectionProfiles() {
  const store = useAppStore();

  const drafts = ref<ConnectionDraft[]>([]);
  /** 默认连接的**行下标**(新增行还没有 id,故按行跟踪;保存后回填为服务端 id 对应行) */
  const activeIndex = ref<number | null>(null);
  const loading = ref(false);
  const saving = ref(false);
  const feedback = ref<{ kind: 'ok' | 'err' | 'info'; text: string } | null>(null);
  /** 任务模式默认连接(TM-SET-1;空串 = 跟随默认连接)。仅任务模式展示与修改;
   *  引用的连接须**已保存**(未保存的新增草稿没有 id,不能作为引用目标)。 */
  const taskDefaultConnectionId = ref('');
  /** 是否任务模式(任务默认连接下拉的可见性判据;角色扮演侧无此设置) */
  const isTaskMode = computed(() => store.appMode === 'task');

  /** 用服务端设置体回填草稿与默认连接选择 */
  function applyServer(s: RuntimeSettings): void {
    drafts.value = (s.connections ?? []).map(toDraft);
    const i = drafts.value.findIndex((d) => d.id === s.active_connection_id);
    activeIndex.value = i >= 0 ? i : null;
    taskDefaultConnectionId.value = s.task_default_connection_id ?? '';
  }

  /** 载入(组件挂载时调用) */
  async function load(): Promise<void> {
    loading.value = true;
    feedback.value = null;
    try {
      applyServer(await api.getSettings(store.appMode));
    } catch (e) {
      feedback.value = { kind: 'err', text: `加载连接配置失败:${(e as Error).message}` };
    } finally {
      loading.value = false;
    }
  }

  function addDraft(): void {
    drafts.value.push({
      id: '',
      name: `连接 ${drafts.value.length + 1}`,
      connector_type: 'openai-compatible',
      base_url: '',
      model: '',
      api_style: 'chat-completions',
      api_key: '',
      clearApiKey: false,
      enabled: true,
      supports_vision: false,
      supports_structured_output: false,
      supports_prefix_completion: false,
      supports_mid_conversation_system: false,
      image_auto_split: false,
      api_key_masked: '',
      has_api_key: false,
    });
  }

  /** 删除(带二次确认);草稿层移除,保存时才落库 */
  function requestRemove(index: number): void {
    const d = drafts.value[index];
    if (!d) return;
    if (!confirm(`确定删除连接「${d.name}」?其 API 密钥会一并删除,不可恢复。`)) return;
    removeDraft(index);
  }

  function removeDraft(index: number): void {
    clearRowProbeState(index);
    drafts.value.splice(index, 1);
    if (activeIndex.value === index) {
      // 被删的就是默认连接:交给后端回退到第一个启用连接(保存后回填真实选择)
      activeIndex.value = null;
    } else if (activeIndex.value != null && activeIndex.value > index) {
      activeIndex.value -= 1;
    }
  }

  function setActive(index: number): void {
    activeIndex.value = index;
  }

  /** 组装 patch:connections 是全量数组语义;密钥留空则不下发(后端保持原密钥) */
  function buildPatch(): RuntimeSettingsPatch {
    const list: ConnectionProfilePatch[] = drafts.value.map((d) => {
      const row: ConnectionProfilePatch = {
        name: d.name.trim(),
        connector_type: d.connector_type,
        base_url: d.base_url.trim(),
        model: d.model.trim(),
        api_style: d.api_style,
        enabled: d.enabled,
        // 能力位显式全量下发(布尔):取消勾选 = 显式 false 清空;旧客户端的「缺省=沿用」不受影响
        supports_vision: d.supports_vision,
        supports_structured_output: d.supports_structured_output,
        supports_prefix_completion: d.supports_prefix_completion,
        supports_mid_conversation_system: d.supports_mid_conversation_system,
        image_auto_split: d.image_auto_split,
      };
      if (d.id) row.id = d.id;
      if (d.api_key.trim()) {
        row.api_key = d.api_key.trim();
      } else if (d.clearApiKey && d.id) {
        // 显式清空已保存密钥(与 api_key 互斥,新输入优先);新行无密钥可清,不下发
        row.clear_api_key = true;
      }
      return row;
    });
    const patch: RuntimeSettingsPatch = { connections: list };
    const selected = activeIndex.value == null ? null : drafts.value[activeIndex.value];
    if (selected?.id) patch.active_connection_id = selected.id;
    return patch;
  }

  /** 保存:全量覆盖连接数组 + 默认连接;新行被设为默认时补一次(见下方注释) */
  async function save(): Promise<void> {
    if (saving.value) return;
    saving.value = true;
    feedback.value = null;
    // 新增行保存前还没有 id,而后端不接收客户端自带的 id(未命中的 id 一律视作新建),
    // 所以「把新行设为默认」只能等拿到 id 后再补一次 PUT。
    // 注意先记住**行下标**:紧随其后的回填会把 activeIndex 换成服务端口径的值。
    const pendingIndex = activeIndex.value;
    const pendingNewDefault = pendingIndex != null && !drafts.value[pendingIndex]?.id;
    try {
      await store.saveSettings(buildPatch());
      applyServer(await api.getSettings(store.appMode));
      if (pendingNewDefault && pendingIndex != null) {
        // 数组顺序即提交顺序(后端原样返回),故下标仍能定位到刚保存的那一行
        const id = drafts.value[pendingIndex]?.id;
        if (id) {
          await store.saveSettings({ active_connection_id: id });
          applyServer(await api.getSettings(store.appMode));
        }
      }
      feedback.value = { kind: 'ok', text: '连接配置已保存并生效' };
    } catch (e) {
      feedback.value = { kind: 'err', text: `保存失败:${(e as Error).message}` };
    } finally {
      saving.value = false;
    }
  }

  /** 任务模式默认连接切换即存(TM-SET-1;与授权三档同款「切换即存 + 失败回滚」)。
   *  走独立 patch(不随「保存连接配置」的全量数组一起提交):引用目标必须是**已保存**的
   *  连接 id,保存按钮的草稿数组里含未保存新行,不能混在一起判有效性。 */
  async function saveTaskDefaultConnection(id: string): Promise<void> {
    const prev = taskDefaultConnectionId.value;
    if (prev === id) return;
    taskDefaultConnectionId.value = id;
    feedback.value = null;
    try {
      await store.saveSettings({ task_default_connection_id: id });
      feedback.value = {
        kind: 'ok',
        text: id ? '任务模式默认连接已保存' : '任务模式已改为跟随默认连接',
      };
    } catch (e) {
      taskDefaultConnectionId.value = prev;
      feedback.value = { kind: 'err', text: `保存失败:${(e as Error).message}` };
    }
  }

  // ---------- 逐连接探测(2026-10-03 API 设置补全:测试连接 / 从 API 加载模型列表) ----------

  /** 行键:已保存行用 id(applyServer 回填后仍能对上),新增行用 `new-<行下标>`(临时) */
  function rowKey(index: number): string {
    const d = drafts.value[index];
    return d?.id ? d.id : `new-${index}`;
  }

  /** 逐行探测状态(纯 UI 态,不落库):key 见 rowKey */
  const testing = ref<Record<string, boolean>>({});
  const loadingModels = ref<Record<string, boolean>>({});
  const modelsByConn = ref<Record<string, string[]>>({});
  const testFeedback = ref<Record<string, { kind: 'ok' | 'err' | 'info'; text: string } | null>>(
    {},
  );

  /** 组装该行探测参数:显式草稿字段(未保存新行也可测)+ id(缺省字段与密钥回退已存值)。
   *  base_url/model 按草稿原样下发(显式空 = 按空值探测,与「显式清空地址」保存语义一致);
   *  api_key 仅在输入了新值时下发(留空 = 用已存密钥测)。 */
  function probePayload(index: number): ConnectionProbeParams {
    const d = drafts.value[index];
    if (!d) return {};
    const payload: ConnectionProbeParams = {
      connection: {
        connector_type: d.connector_type,
        base_url: d.base_url.trim(),
        api_style: d.api_style,
        model: d.model.trim(),
      },
    };
    const typedKey = d.api_key.trim();
    if (typedKey) payload.connection.api_key = typedKey;
    if (d.id) payload.connection_id = d.id;
    return payload;
  }

  /** 逐连接「测试连接」:打参数化探测端点,不影响已生效连接器与连接配置 */
  async function testConnection(index: number): Promise<void> {
    const key = rowKey(index);
    if (testing.value[key]) return;
    testing.value = { ...testing.value, [key]: true };
    testFeedback.value = { ...testFeedback.value, [key]: null };
    try {
      const res = await api.testConnect(probePayload(index));
      testFeedback.value = {
        ...testFeedback.value,
        [key]: res.ok
          ? { kind: 'ok', text: res.message ? `连接成功:${res.message}` : '连接成功' }
          : { kind: 'err', text: res.message || '连接失败' },
      };
    } catch (e) {
      testFeedback.value = {
        ...testFeedback.value,
        [key]: { kind: 'err', text: `测试失败:${(e as Error).message}` },
      };
    } finally {
      testing.value = { ...testing.value, [key]: false };
    }
  }

  /** 逐连接「从 API 加载模型列表」:结果写入该行 MODEL 输入的建议下拉 */
  async function refreshModelsFor(index: number): Promise<void> {
    const key = rowKey(index);
    if (loadingModels.value[key]) return;
    loadingModels.value = { ...loadingModels.value, [key]: true };
    try {
      const res = await api.refreshModels(probePayload(index));
      modelsByConn.value = { ...modelsByConn.value, [key]: res.models };
      if (res.message) {
        // 上游的「可能不支持 /models 接口」类提示按 info 呈现,不算失败
        testFeedback.value = { ...testFeedback.value, [key]: { kind: 'info', text: res.message } };
      }
    } catch (e) {
      testFeedback.value = {
        ...testFeedback.value,
        [key]: { kind: 'err', text: `加载模型列表失败:${(e as Error).message}` },
      };
    } finally {
      loadingModels.value = { ...loadingModels.value, [key]: false };
    }
  }

  /** 行删除时顺手清掉该行的临时探测状态(键可能随行下标漂移,不清理也只是残留) */
  function clearRowProbeState(index: number): void {
    const key = rowKey(index);
    delete testing.value[key];
    delete loadingModels.value[key];
    delete modelsByConn.value[key];
    delete testFeedback.value[key];
  }

  return {
    drafts, activeIndex, loading, saving, feedback,
    taskDefaultConnectionId, isTaskMode, saveTaskDefaultConnection,
    load, addDraft, requestRemove, removeDraft, setActive, buildPatch, save,
    rowKey, testing, loadingModels, modelsByConn, testFeedback,
    probePayload, testConnection, refreshModelsFor, clearRowProbeState,
  };
}
