// 输入草稿仅存当前应用内存：切模式/会话不丢，刷新或退出后清除，不写本地存储。
import { reactive } from 'vue';
import { defineStore } from 'pinia';

export interface ChatDraft {
  text: string;
  attachments: File[];
  sending: boolean;
}

export const useComposerDraftsStore = defineStore('composerDrafts', () => {
  const chatDrafts = reactive(new Map<string, ChatDraft>());
  const taskDraft = reactive({ title: '', workspace: '', executorId: '', creating: false });

  function chatDraft(characterId: string | null, sessionId: string | null): ChatDraft {
    const key = JSON.stringify([characterId, sessionId]);
    const existing = chatDrafts.get(key);
    if (existing) return existing;
    // 首条发送会先新建会话；承接同一角色尚未绑定会话的草稿，拒收也能继续编辑。
    const unboundKey = JSON.stringify([characterId, null]);
    const unbound = sessionId ? chatDrafts.get(unboundKey) : undefined;
    const draft = unbound ?? reactive<ChatDraft>({ text: '', attachments: [], sending: false });
    chatDrafts.set(key, draft);
    if (unbound) chatDrafts.delete(unboundKey);
    return draft;
  }

  function setChatText(draft: ChatDraft, text: string): void {
    draft.text = text;
  }
  function setChatAttachments(draft: ChatDraft, files: File[]): void {
    draft.attachments = files;
  }
  function setSending(draft: ChatDraft, value: boolean): void {
    draft.sending = value;
  }
  function acceptChatSend(draft: ChatDraft, originalText: string, sentFiles: readonly File[]): void {
    if (draft.text === originalText) draft.text = '';
    const sent = new Set(sentFiles);
    draft.attachments = draft.attachments.filter((file) => !sent.has(file));
  }
  function setTaskDraft(patch: Partial<typeof taskDraft>): void {
    Object.assign(taskDraft, patch);
  }

  return { chatDrafts, taskDraft, chatDraft, setChatText, setChatAttachments, setSending, acceptChatSend, setTaskDraft };
});
