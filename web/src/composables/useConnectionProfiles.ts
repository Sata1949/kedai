// 多套连接配置(批次 4):列表载入、行内编辑草稿、新增/删除/设为默认/启停与保存。
//
// 保存走既有 store.saveSettings 通道(与 useApiSettings 同一路径,patch 里多带
// connections / active_connection_id),不新增端点;语言与错误提示同 useApiSettings。
import { ref } from 'vue';
import { useAppStore } from '../store';
import * as api from '../api';
import type {
  ConnectionProfile,
  ConnectionProfilePatch,
  RuntimeSettings,
  RuntimeSettingsPatch,
} from '../api/types';

/** 行内编辑草稿;id 为空 = 后端尚未分配(新增行,保存后才拿到 id) */
export interface ConnectionDraft {
  id: string;
  name: string;
  connector_type: string;
  base_url: string;
  model: string;
  /** 只写字段:留空 = 不改动该连接的密钥(与后端「空 Key 忽略」同口径,接口层面无法清空密钥) */
  api_key: string;
  enabled: boolean;
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
    api_key: '',
    enabled: p.enabled,
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

  /** 用服务端设置体回填草稿与默认连接选择 */
  function applyServer(s: RuntimeSettings): void {
    drafts.value = (s.connections ?? []).map(toDraft);
    const i = drafts.value.findIndex((d) => d.id === s.active_connection_id);
    activeIndex.value = i >= 0 ? i : null;
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
      api_key: '',
      enabled: true,
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
        enabled: d.enabled,
      };
      if (d.id) row.id = d.id;
      if (d.api_key.trim()) row.api_key = d.api_key.trim();
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

  return {
    drafts, activeIndex, loading, saving, feedback,
    load, addDraft, requestRemove, removeDraft, setActive, buildPatch, save,
  };
}
