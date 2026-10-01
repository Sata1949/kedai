import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createSSRApp, h, type Component } from 'vue';
import { renderToString } from 'vue/server-renderer';
import { createPinia, setActivePinia } from 'pinia';
import { useApiSettings } from '../../composables/useApiSettings';
import { useConnectionProfiles } from '../../composables/useConnectionProfiles';
import { usePromptInject } from '../../composables/usePromptInject';
import { useTaskStore } from '../../stores/task';
import ConnectionProfilesSection from './ConnectionProfilesSection.vue';
import ConnectionSection from './ConnectionSection.vue';
import McpSection from './McpSection.vue';
import CodingBundleSection from './CodingBundleSection.vue';
import PresetImportExportSection from './PresetImportExportSection.vue';
import AgentSettingsSection from './AgentSettingsSection.vue';
import GenParamsSection from './GenParamsSection.vue';
import SettingsModal from '../SettingsModal.vue';

// 设置区组件冒烟测试:项目无 jsdom / @vue/test-utils,沿用 CacheHealthPanel.test.ts 的
// SSR 模式(createSSRApp + renderToString)。SSR 不触发 onMounted,因此各 section
// 的数据加载不会真正发请求;此处只验证「渲染不炸 + 关键文案存在 + show 开关生效」。

// node 环境无 localStorage,而 store 初始化即访问(HTML 渲染记忆 / 脚本授权存储),补内存桩
const memStorage = new Map<string, string>();
vi.stubGlobal('localStorage', {
  getItem: (k: string) => memStorage.get(k) ?? null,
  setItem: (k: string, v: string) => void memStorage.set(k, String(v)),
  removeItem: (k: string) => void memStorage.delete(k),
  clear: () => memStorage.clear(),
  key: (i: number) => [...memStorage.keys()][i] ?? null,
  get length() { return memStorage.size; },
});

/** 以 pinia 上下文 SSR 渲染组件为 HTML 字符串(可传入共享 pinia:组件内部读 store
 *  的用例需要在渲染前设定 appMode 等状态,与 beforeEach 的活动 pinia 必须是同一个) */
async function render(
  comp: Component,
  props: Record<string, unknown> = {},
  pinia?: ReturnType<typeof createPinia>,
): Promise<string> {
  const app = createSSRApp({ render: () => h(comp, props) });
  app.use(pinia ?? createPinia());
  return renderToString(app);
}

/** 剥掉 SSR 输出里的 HTML 注释(dev 模式注释会进 HTML;断言「某块未渲染」时
 *  源码注释里的同名文案会误伤,先剥注释再断言) */
const stripComments = (html: string): string => html.replace(/<!--[\s\S]*?-->/g, '');

beforeEach(() => {
  // composable 在测试直接调用(构造 prop 状态)时需要活动 pinia
  setActivePinia(createPinia());
});

describe('ConnectionSection(后端连接区)', () => {
  it('渲染连接器信息与测试连接按钮(与 ApiSettingsSection 共享壳传入状态)', async () => {
    const state = useApiSettings();
    const html = await render(ConnectionSection, { state });
    expect(html).toContain('后端连接');
    expect(html).toContain('测试连接');
  });

  it('show=false 时根节点 display:none(embedded 模式按 activeSection 切换)', async () => {
    const state = useApiSettings();
    const html = await render(ConnectionSection, { state, show: false });
    expect(html).toMatch(/display:\s*none/);
  });
});

describe('ConnectionProfilesSection(连接配置区)', () => {
  it('渲染连接列表:默认单选 / 密钥掩码占位 / 停用行不可选为默认', async () => {
    const state = useConnectionProfiles();
    state.drafts.value = [
      {
        id: 'c1', name: '主连接', connector_type: 'openai-compatible',
        base_url: 'https://a.example/v1', model: 'ma', api_style: 'chat-completions',
        api_key: '', enabled: true, api_key_masked: '****1111', has_api_key: true,
      },
      {
        id: 'c2', name: '备用', connector_type: 'mock',
        base_url: '', model: '', api_style: 'anthropic',
        api_key: '', enabled: false, api_key_masked: '', has_api_key: false,
      },
    ];
    state.activeIndex.value = 0;
    const html = await render(ConnectionProfilesSection, { state });
    expect(html).toContain('连接配置');
    expect(html).toContain('主连接');
    expect(html).toContain('****1111'); // 密钥只回显掩码
    expect(html).toContain('OpenAI 兼容'); // 类型文案来自 api/labels.ts
    expect(html).toContain('接口格式'); // 方言下拉(接口方言批次)
    expect(html).toContain('Anthropic Messages'); // 方言文案来自 api/labels.ts
    expect(html).toContain('新增连接');
    expect(html).toContain('保存连接配置');
    expect(html).toContain('连接信息全局共享'); // 本批只做配置管理,聊天/任务仍用默认连接
    // 默认连接单选:第一行选中,停用的第二行不可选
    expect(html).toMatch(/id="conn-default-0"[^>]*checked/);
    expect(html).toMatch(/id="conn-default-1"[^>]*disabled/);
  });

  it('show=false 时根节点 display:none(embedded 模式按 activeSection 切换)', async () => {
    const state = useConnectionProfiles();
    const html = await render(ConnectionProfilesSection, { state, show: false });
    expect(html).toMatch(/display:\s*none/);
  });

  it('任务模式渲染「任务模式默认连接」下拉;角色扮演模式不渲染(TM-SET-1)', async () => {
    // 角色扮演:任务模式专属块不得出现(先剥源码注释,防注释同名文案误伤)
    const rpState = useConnectionProfiles();
    const rpHtml = stripComments(await render(ConnectionProfilesSection, { state: rpState }));
    expect(rpHtml).not.toContain('任务模式默认连接');

    // 任务模式:渲染下拉(含「跟随默认连接」项)与回退链说明
    useTaskStore().appMode = 'task';
    const state = useConnectionProfiles();
    state.drafts.value = [
      {
        id: 'c1', name: '主连接', connector_type: 'openai-compatible',
        base_url: 'https://a.example/v1', model: 'ma', api_style: 'chat-completions',
        api_key: '', enabled: true, api_key_masked: '****1111', has_api_key: true,
      },
    ];
    state.taskDefaultConnectionId.value = 'c1';
    const html = stripComments(await render(ConnectionProfilesSection, { state }));
    expect(html).toContain('任务模式默认连接');
    expect(html).toContain('跟随默认连接');
    expect(html).toContain('逐任务');
  });
});

describe('PresetImportExportSection(预设导入/导出区)', () => {
  it('渲染导入/导出两行(合并后 standalone 也包含本区,防回归)', async () => {
    const state = usePromptInject();
    const html = await render(PresetImportExportSection, { state });
    expect(html).toContain('预设导入 / 导出');
    expect(html).toContain('导入酒馆预设');
    expect(html).toContain('导出注入配置');
  });
});

describe('SettingsModal(壳)', () => {
  it('standalone 模式渲染遮罩/头部,且包含预设导入导出区(原 standalone 缺失)', async () => {
    const html = await render(SettingsModal, { embedded: false });
    expect(html).toContain('sv-modal-mask');
    expect(html).toContain('预设导入 / 导出');
    expect(html).toContain('API 设置');
    expect(html).toContain('提示词注入');
  });

  it('embedded 模式仅渲染内容区,且只显示 activeSection 对应分区', async () => {
    const html = await render(SettingsModal, { embedded: true, activeSection: 'data' });
    expect(html).not.toContain('sv-modal-mask');
    expect(html).toContain('sv-settings-embedded');
    // 数据管理区可见;API 设置区应被 v-show 隐藏
    expect(html).toContain('数据管理');
  });

  it('装配:MCP 分区挂进壳(standalone 渲染;embedded 挂载不炸)', async () => {
    const standalone = await render(SettingsModal, { embedded: false });
    expect(standalone).toContain('MCP 服务');
    expect(standalone).toContain('重启后生效');

    const embedded = await render(SettingsModal, { embedded: true, activeSection: 'mcp' });
    expect(embedded).toContain('sv-settings-embedded');
    expect(embedded).toContain('MCP 服务');
  });

  it('装配:编码能力包分区挂进壳(standalone 渲染;embedded 按 activeSection 可见)', async () => {
    const standalone = await render(SettingsModal, { embedded: false });
    expect(standalone).toContain('编码能力包');

    const embedded = await render(SettingsModal, { embedded: true, activeSection: 'coding' });
    expect(embedded).toContain('sv-settings-embedded');
    expect(embedded).toContain('编码能力包');
  });
});

describe('CodingBundleSection(编码能力包区,默认关)', () => {
  it('渲染开关与语义说明(默认「已关闭」;仅任务模式生效 / 不影响角色扮演)', async () => {
    const html = await render(CodingBundleSection);
    expect(html).toContain('编码能力包');
    expect(html).toContain('启用编码能力包');
    expect(html).toContain('已关闭'); // 默认 task_coding_bundle_enabled=false
    expect(html).toContain('编码执行者模板');
    expect(html).toContain('不影响角色扮演模式');
  });

  it('show=false 时根节点 display:none(embedded 模式按 activeSection 切换)', async () => {
    const html = await render(CodingBundleSection, { show: false });
    expect(html).toMatch(/display:\s*none/);
  });
});

describe('McpSection(MCP 服务区,批次 6.2)', () => {
  it('渲染总开关/新增表单/「重启后生效」提示(默认关)', async () => {
    const html = await render(McpSection);
    expect(html).toContain('MCP 服务');
    expect(html).toContain('启用 MCP 服务');
    expect(html).toContain('已关闭'); // 默认 mcp_enabled=false
    expect(html).toContain('新增服务器');
    expect(html).toContain('重启后生效');
    expect(html).toContain('保存 MCP 设置');
  });

  it('show=false 时根节点 display:none(embedded 模式按 activeSection 切换)', async () => {
    const html = await render(McpSection, { show: false });
    expect(html).toMatch(/display:\s*none/);
  });
});

describe('AgentSettingsSection(Agent 设置区)', () => {
  it('两开关退役后不再渲染(TM-SET-2);其余字段照常(系统提示词 / 搜索端点 / 反思提示词)', async () => {
    const html = stripComments(await render(AgentSettingsSection));
    // 退役控件不得再出现
    expect(html).not.toContain('执行者人设');
    expect(html).not.toContain('任务继承提示词注入');
    // 其余 Agent 设置仍在
    expect(html).toContain('系统提示词');
    expect(html).toContain('搜索端点');
    expect(html).toContain('反思提示词');
    expect(html).toContain('保存 Agent 设置');
  });
});

describe('GenParamsSection(生成参数区:记忆槽预算/容量上限)', () => {
  it('渲染记忆字符预算与容量上限两个 number 输入(0 = 不限制说明)', async () => {
    const html = await render(GenParamsSection);
    expect(html).toContain('记忆字符预算');
    expect(html).toContain('记忆容量上限');
    expect(html).toContain('0 = 不限制');
    expect(html).toMatch(/max="20000"/);
    expect(html).toMatch(/max="10000"/);
  });

  it('show=false 时根节点 display:none(embedded 模式按 activeSection 切换)', async () => {
    const html = await render(GenParamsSection, { show: false });
    expect(html).toMatch(/display:\s*none/);
  });

  it('任务模式渲染「任务缺省」块与「写入任务推荐值」;角色扮演模式不渲染(TM-SET-1)', async () => {
    const rpHtml = stripComments(await render(GenParamsSection));
    expect(rpHtml).not.toContain('任务模式缺省');

    // 组件内部读 store.appMode:pinia 必须与设定状态的实例同一个
    const pinia = createPinia();
    setActivePinia(pinia);
    useTaskStore().appMode = 'task';
    const html = stripComments(await render(GenParamsSection, {}, pinia));
    expect(html).toContain('任务模式缺省');
    expect(html).toContain('写入任务推荐值');
    expect(html).toContain('温度 0.3');
  });
});
