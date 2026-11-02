// 主 Agent 提示词(DATA_DIR/AGENTS_RUNTIME[.task].md)编辑与最终提示词预览。
//
// 两处与模式相关(RTSPLIT-2,2026-11-02):
// - **文件也按模式分**:角色扮演读 AGENTS_RUNTIME.md,任务模式读 AGENTS_RUNTIME.task.md。
//   切模式必须重载正文,否则「提示词不随模式切换」正是用户看到的现象。
// - **预览带模式校验**:异步响应回来时若已切走,直接丢弃并补拉。此前用
//   `if (loading) return` 早退,切模式触发的重拉被静默吞掉,旧模式结果照样写回且无补偿。
import { ref } from 'vue';
import { useAppStore } from '../store';
import { storeToRefs } from 'pinia';
import * as api from '../api';

export function useAgentPromptEditor() {
  const store = useAppStore();
  const { currentSessionId, currentCharacterId, appMode } = storeToRefs(store);

  const agentPromptMd = ref('');
  /** 上次从服务端加载到的正文快照:用于判断「用户是否改了没保存」(切模式前提示用)。 */
  const agentPromptLoaded = ref('');
  const promptPreview = ref<api.PromptPreview | null>(null);
  const previewLoading = ref(false);
  const previewError = ref('');

  /**
   * 预览请求序号:每次发起递增。
   * 只有「序号仍是最新」的响应才允许写回——防止先发后到的旧模式结果覆盖新模式。
   */
  let previewSeq = 0;
  /** 在飞期间又被请求一次时登记「尾随重拉」,收尾后补一轮(不叠加并发)。 */
  let previewQueued = false;

  async function loadPromptPreview(): Promise<void> {
    // 在飞时不再静默丢弃:登记一次尾随重拉,保证切模式后一定会拿到新模式的结果
    if (previewLoading.value) {
      previewQueued = true;
      return;
    }
    previewLoading.value = true;
    let needRetry = false;
    const seq = ++previewSeq;
    // 发起时快照模式:响应回来要拿它跟「当前模式」比
    const requestedMode = appMode.value;
    previewError.value = '';
    try {
      const next = await api.getPromptPreview(
        currentSessionId.value ?? undefined,
        currentCharacterId.value ?? undefined,
        requestedMode,
      );
      // 三重判定,任一不成立就不写回:
      //   ① 期间没再发起新请求(seq);
      //   ② 当前模式仍是发起时那个;
      //   ③ 服务端回显的 mode 与发起时一致(以服务端为准,防本地抖动)。
      const superseded = seq !== previewSeq;
      const modeChanged = appMode.value !== requestedMode;
      const serverMismatch = next.mode !== undefined && next.mode !== requestedMode;
      if (superseded || modeChanged || serverMismatch) {
        needRetry = modeChanged || serverMismatch;
      } else {
        promptPreview.value = next;
      }
    } catch (e) {
      previewError.value = `预览失败:${(e as Error).message}`;
    } finally {
      previewLoading.value = false;
      const queued = previewQueued;
      previewQueued = false;
      if (queued || needRetry) void loadPromptPreview();
    }
  }

  const mdLoading = ref(false);
  const mdSaving = ref(false);
  const mdMsg = ref<{ kind: 'ok' | 'err'; text: string } | null>(null);
  /** 当前正文属于哪个模式(用于 UI 判断「这份内容是不是当前模式的」)。 */
  const agentPromptMode = ref<'roleplay' | 'task' | null>(null);

  async function loadAgentPromptMd(): Promise<void> {
    if (mdLoading.value) return;
    mdLoading.value = true;
    const requestedMode = appMode.value;
    try {
      const data = await api.getAgentPromptMd(requestedMode);
      // 期间切了模式:这份结果属于旧模式,丢弃(切模式会再触发一次加载)
      if (appMode.value !== requestedMode) return;
      agentPromptMd.value = data.content;
      agentPromptLoaded.value = data.content;
      agentPromptMode.value = requestedMode;
      mdMsg.value = null;
    } catch (e) {
      mdMsg.value = { kind: 'err', text: `读取失败:${(e as Error).message}` };
    } finally {
      mdLoading.value = false;
    }
  }

  async function saveAgentPromptMd(): Promise<void> {
    if (mdSaving.value) return;
    mdSaving.value = true;
    mdMsg.value = null;
    const requestedMode = appMode.value;
    try {
      const data = await api.saveAgentPromptMd(agentPromptMd.value, requestedMode);
      // 保存本身按发起时模式写入(正确);只是界面上的「已保存」提示要对应到那个模式
      mdMsg.value = { kind: 'ok', text: `已保存到 ${data.path}` };
      agentPromptLoaded.value = agentPromptMd.value;
      agentPromptMode.value = requestedMode;
      setTimeout(() => (mdMsg.value = null), 3000);
      // 运行时主提示词会进 system,保存后预览层应同步刷新
      if (appMode.value === requestedMode) void loadPromptPreview();
    } catch (e) {
      mdMsg.value = { kind: 'err', text: `保存失败:${(e as Error).message}` };
    } finally {
      mdSaving.value = false;
    }
  }

  return {
    agentPromptMd, agentPromptLoaded, agentPromptMode, promptPreview, previewLoading, previewError,
    mdLoading, mdSaving, mdMsg,
    loadPromptPreview, loadAgentPromptMd, saveAgentPromptMd,
  };
}
