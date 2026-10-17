// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { mount } from '@vue/test-utils';
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

import { useCharacterStore } from '../stores/character';
import GreetingPickerModal from './GreetingPickerModal.vue';

function mountPicker() {
  setActivePinia(createPinia());
  const character = useCharacterStore();
  character.characters = [{
    id: 'c1',
    name: 'test',
    chara_name: '测试角色',
    description: '',
    file_path: '',
    avatar_path: null,
    first_mes: '主开场',
    alternate_greetings: ['备用开场'],
    created_at: '2026-10-17T00:00:00Z',
  }];
  character.currentCharacterId = 'c1';
  return { character, wrapper: mount(GreetingPickerModal, { props: { mode: 'new' } }) };
}

describe('GreetingPickerModal 无障碍', () => {
  beforeEach(() => memStorage.clear());

  it('声明 dialog/aria-modal,并渲染全部开场', () => {
    const { wrapper } = mountPicker();
    const dialog = wrapper.find('[role="dialog"]');
    expect(dialog.exists()).toBe(true);
    expect(dialog.attributes('aria-modal')).toBe('true');
    expect(dialog.attributes('aria-labelledby')).toBe('sv-greeting-title');
    expect(wrapper.findAll('.sv-greet-row')).toHaveLength(2);
    wrapper.unmount();
  });

  it('Esc 关闭对话框', async () => {
    const { wrapper } = mountPicker();
    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }));
    await wrapper.vm.$nextTick();
    expect(wrapper.emitted('close')).toHaveLength(1);
    wrapper.unmount();
  });
});
