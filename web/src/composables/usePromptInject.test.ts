// @vitest-environment jsdom
// usePromptInject 导入判定回归网(STIMP-2):形状判定 + 三路导入流程 + 仓库示例文件。
// 缺陷背景:修复前 ST 判据查 data.floors(kedai 自有楼层形状),与后端 parsing/preset.rs
// 「以 prompts 为唯一依据」相反 —— 真酒馆预设必被前端拒绝(「无法识别的 JSON 格式」),
// kedai 自家导出因 floors 条目带 role/position 反被误判为 ST 送后端 400。见 docs/计划.md STIMP 章。
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';

// 部分 mock:仅替换两条导入相关 API,其余保持真实(先例:ChatInput.test.ts)
vi.mock('../api', async (importOriginal) => {
  const orig = await importOriginal<typeof import('../api')>();
  return {
    ...orig,
    importPromptPreset: vi.fn(),
    savePromptInject: vi.fn(),
  };
});

import * as api from '../api';
import { useAppStore } from '../store';
import { detectInjectFileShape, usePromptInject } from './usePromptInject';
// 仓库自带示例预设:静态 JSON 导入(jsdom 下 import.meta.url 是 http 方案,readFileSync 只收 file: URL)
import examplePreset from '../../../examples/presets/可待精华增强楼层.json';

/** 导入流程用例的统一入口:真实 change 事件 + 定义 target(源码只读 target.files 并写其 value) */
function buildImportEvent(payload: unknown, name = 'preset.json'): Event {
  const file = new File([JSON.stringify(payload)], name, { type: 'application/json' });
  const evt = new Event('change');
  Object.defineProperty(evt, 'target', { value: { files: [file], value: '' } });
  return evt;
}

function makeConfig(floorCount: number): api.PromptInjectConfig {
  return {
    mode: 'complex',
    simple: { banned_words_enabled: false } as api.PromptInjectConfig['simple'],
    floors: Array.from({ length: floorCount }, (_, i) => ({
      id: `f${i}`,
      name: `楼层${i}`,
      content: `内容${i}`,
      role: 'system' as const,
      position: 'system' as const,
      depth: 0,
      enabled: true,
      order: i,
    })),
  };
}

function setup() {
  setActivePinia(createPinia());
  const store = useAppStore();
  const state = usePromptInject();
  return { store, state };
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.spyOn(window, 'confirm').mockReturnValue(true);
});

describe('detectInjectFileShape 形状判定', () => {
  it('顶层 prompts 数组 → st(酒馆预设)', () => {
    expect(detectInjectFileShape({ name: 'x', prompts: [{ identifier: 'a' }], prompt_order: [] })).toBe('st');
  });

  it('prompts 为空数组仍判 st(是否可导入交后端给精确错误)', () => {
    expect(detectInjectFileShape({ prompts: [] })).toBe('st');
  });

  it('mode 字符串 + simple 字段 → kedai(本工具导出配置)', () => {
    expect(detectInjectFileShape({ mode: 'complex', simple: { banned_words_enabled: false }, floors: [] })).toBe('kedai');
  });

  it('floors-only 形状(缺 mode/simple)→ unknown', () => {
    expect(detectInjectFileShape({ floors: [{ role: 'user', position: 'system' }] })).toBe('unknown');
  });

  it('空对象 → unknown', () => {
    expect(detectInjectFileShape({})).toBe('unknown');
  });

  it('null 与字符串等非对象 → unknown', () => {
    expect(detectInjectFileShape(null)).toBe('unknown');
    expect(detectInjectFileShape('not-an-object')).toBe('unknown');
  });
});

describe('onImportPreset 三路分派', () => {
  it('st 形状 → 调 importPromptPreset 并整体替换楼层草稿与 store', async () => {
    const { store, state } = setup();
    const config = makeConfig(3);
    vi.mocked(api.importPromptPreset).mockResolvedValue({ ok: true, imported: 3, config });

    await state.onImportPreset(buildImportEvent({ prompts: [{ identifier: 'a' }], prompt_order: [] }));

    expect(api.importPromptPreset).toHaveBeenCalledTimes(1);
    expect(api.savePromptInject).not.toHaveBeenCalled();
    expect(store.promptInject?.floors.length).toBe(3);
    expect(state.injectDraft.value?.floors.length).toBe(3);
    expect(state.injectMsg.value).toContain('已导入 3 条楼层');
  });

  it('kedai 导出形状 → 调 savePromptInject(含禁词迁移)而不误走 ST 分支', async () => {
    const { store, state } = setup();
    vi.mocked(api.savePromptInject).mockImplementation(async (cfg) => cfg);
    const payload = {
      mode: 'simple',
      simple: {
        banned_words_enabled: true,
        banned_words: [{ word: '笨蛋' }],
      },
      floors: [{
        id: 'f0', name: '楼层0', content: '内容0', role: 'user', position: 'system',
        depth: 0, enabled: true, order: 0,
      }],
    };

    await state.onImportPreset(buildImportEvent(payload));

    expect(api.importPromptPreset).not.toHaveBeenCalled();
    expect(api.savePromptInject).toHaveBeenCalledTimes(1);
    const sent = vi.mocked(api.savePromptInject).mock.calls[0][0];
    expect(sent.simple.banned_prompt).toContain('笨蛋');
    expect(store.promptInject?.mode).toBe('simple');
    expect(state.injectMsg.value).toContain('已导入配置');
  });

  it('无法识别的形状 → 两条 API 零调用且错误文案只对真无效文件出现', async () => {
    const { state } = setup();

    await state.onImportPreset(buildImportEvent({ foo: 1 }));

    expect(api.importPromptPreset).not.toHaveBeenCalled();
    expect(api.savePromptInject).not.toHaveBeenCalled();
    expect(state.injectMsg.value.startsWith('导入失败:')).toBe(true);
    expect(state.injectMsg.value).toContain('无法识别的 JSON 格式');
  });
});

describe('仓库自带示例回归', () => {
  it('examples/presets/可待精华增强楼层.json 判定为 st(自 v0.2.0 起不可导入的缺陷锚点)', () => {
    expect(Array.isArray(examplePreset.prompts)).toBe(true);
    expect(detectInjectFileShape(examplePreset)).toBe('st');
  });
});
