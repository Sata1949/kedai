import { beforeEach, describe, expect, it } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { useComposerDraftsStore } from './composerDrafts';

beforeEach(() => setActivePinia(createPinia()));

describe('输入草稿仅存应用内存', () => {
  it('角色与会话隔离，首次分配会话承接未绑定草稿', () => {
    const store = useComposerDraftsStore();
    const unbound = store.chatDraft('c1', null);
    store.setChatText(unbound, '未提交');
    expect(store.chatDraft('c1', 's1')).toBe(unbound);
    expect(store.chatDraft('c2', 's1').text).toBe('');
    expect(store.chatDraft('c1', 's2').text).toBe('');
    // 新建第二个会话不能重新带入第一个会话的文本。
    expect(store.chatDraft('c1', null).text).toBe('');
  });

  it('成功回执仅移除已发送文件，并保留新编辑内容', () => {
    const store = useComposerDraftsStore();
    const draft = store.chatDraft('c1', 's1');
    const sent = new File(['已发送'], 'sent.txt');
    const added = new File(['未发送'], 'added.txt');
    store.setChatText(draft, '下一条');
    store.setChatAttachments(draft, [sent, added]);
    store.acceptChatSend(draft, '上一条', [sent]);
    expect(draft.text).toBe('下一条');
    expect(draft.attachments).toEqual([added]);
    store.acceptChatSend(draft, '下一条', []);
    expect(draft.text).toBe('');
  });

  it('新应用实例没有旧草稿，任务与聊天草稿互不覆盖', () => {
    const store = useComposerDraftsStore();
    store.setTaskDraft({ title: '任务', workspace: 'D:/example', executorId: 'e1', creating: true });
    store.setChatText(store.chatDraft('c1', 's1'), '聊天');
    expect(store.taskDraft.title).toBe('任务');
    setActivePinia(createPinia());
    const next = useComposerDraftsStore();
    expect(next.taskDraft.title).toBe('');
    expect(next.taskDraft.creating).toBe(false);
    expect(next.chatDraft('c1', 's1').text).toBe('');
  });
});
