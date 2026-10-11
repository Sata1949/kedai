// @vitest-environment jsdom
// AgentSettingsSection 的「最终提示词预览告警」渲染用例(WB-E,遗留 L31):
// 后端只回结构化码,组件须经单一出处模块翻成中文提示并展示;无告警时不渲染提示块。
import { describe, expect, it, vi } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import { createPinia, setActivePinia } from 'pinia';

const memStorage = new Map<string, string>();
vi.stubGlobal('localStorage', {
  getItem: (k: string) => memStorage.get(k) ?? null,
  setItem: (k: string, v: string) => void memStorage.set(k, String(v)),
  removeItem: (k: string) => void memStorage.delete(k),
  clear: () => memStorage.clear(),
  key: (i: number) => [...memStorage.keys()][i] ?? null,
  get length() { return memStorage.size; },
});

vi.mock('../../api', async (importOriginal) => {
  const orig = await importOriginal<typeof import('../../api')>();
  return {
    ...orig,
    getAgentPromptMd: vi.fn().mockResolvedValue({ content: '' }),
    getPromptPreview: vi.fn().mockResolvedValue({ ok: true, note: '', layers: [], warnings: [] }),
  };
});

import { getPromptPreview } from '../../api';
import type { PromptPreview } from '../../api';
import { promptWarningText } from '../../utils/promptWarnings';
import AgentSettingsSection from './AgentSettingsSection.vue';

async function mountWithWarnings(warnings: string[]) {
  setActivePinia(createPinia());
  const preview: PromptPreview = { ok: true, note: '脱敏说明', layers: [], warnings };
  vi.mocked(getPromptPreview).mockResolvedValue(preview);
  const wrapper = mount(AgentSettingsSection);
  await flushPromises();
  return wrapper;
}

describe('AgentSettingsSection 预览告警(WB-E)', () => {
  // 断言取「单一出处模块」的文案本体,不再硬编码:组件漏接 promptWarnings 即红
  it('有告警码时渲染该码的中文提示', async () => {
    const wrapper = await mountWithWarnings(['template_missing_world_info']);
    expect(wrapper.text()).toContain(promptWarningText('template_missing_world_info'));
  });

  it('无告警时不渲染提示块', async () => {
    const wrapper = await mountWithWarnings([]);
    expect(wrapper.text()).not.toContain(promptWarningText('template_missing_world_info'));
  });
});
