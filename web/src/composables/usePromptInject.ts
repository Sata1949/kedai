// 提示词注入设置:简单模式(禁词库/字数/转述/对话/视角)+ 复杂模式(楼层系统,仿 SillyTavern Prompt Manager)。
import { ref } from 'vue';
import { useAppStore } from '../store';
import * as api from '../api';
// 形状判定迁入 utils/importBundle(IMPHUB 步骤 C):纯函数集中一处供综合导入与测试共用;
// 此处 re-export 保持既有调用方零改动。
export { detectInjectFileShape, type InjectFileShape } from '../utils/importBundle';

export const FLOOR_ROLE_LABELS: Record<api.FloorRole, string> = {
  system: '系统提示词',
  user: '用户',
  assistant: '角色',
};

/** 酒馆宏速查(楼层内容编辑参考) */
export const MACRO_HINTS = [
  '{{char}} 角色名', '{{user}} 用户', '{{time}} 时间', '{{datetime}} 日期时间',
  '{{random:1,100}} 随机数', '{{roll:1d20}} 骰子', '{{newline}} 换行',
  '{{lastMessage}} 上一条消息', '{{lastUserMessage}} 最近用户消息', '{{lastCharMessage}} 最近角色消息',
  '{{firstMessage}} 首条消息', '{{personality}} 个性', '{{scenario}} 情景',
  '{{setvar::键::值}} 设置变量', '{{addvar::键::追加}} 追加变量', '{{getvar::键}} 读取变量',
  '{{trim}} 修饰下一宏去空白', '{{//注释}} 注释到行尾',
];

export function usePromptInject() {
  const store = useAppStore();

  const injectDraft = ref<api.PromptInjectConfig | null>(null);
  const injectSaving = ref(false);
  const injectMsg = ref('');
  const editingFloorId = ref<string | null>(null);
  const dragFloorId = ref<string | null>(null);

  /** 从服务端加载配置并克隆为可编辑草稿(保存时才提交) */
  async function loadInjectConfig(): Promise<void> {
    await store.loadPromptInject();
    const cfg = store.promptInject;
    injectDraft.value = cfg ? (JSON.parse(JSON.stringify(cfg)) as api.PromptInjectConfig) : null;
  }

  async function saveInjectNow(): Promise<void> {
    if (!injectDraft.value || injectSaving.value) return;
    injectSaving.value = true;
    injectMsg.value = '';
    try {
      await store.savePromptInjectConfig(injectDraft.value);
      injectMsg.value = '注入设置已保存';
      setTimeout(() => (injectMsg.value = ''), 2500);
    } catch (e) {
      injectMsg.value = `保存失败:${(e as Error).message}`;
    } finally {
      injectSaving.value = false;
    }
  }

  function newFloorId(): string {
    return (crypto.randomUUID?.() ?? `f-${Date.now()}`) as string;
  }

  function addFloor(): void {
    if (!injectDraft.value) return;
    const id = newFloorId();
    injectDraft.value.floors.push({
      id,
      name: '新楼层',
      content: '',
      role: 'user',
      position: 'system',
      depth: 0,
      enabled: true,
      order: injectDraft.value.floors.length,
    });
    editingFloorId.value = id;
  }

  function removeFloor(id: string): void {
    if (!injectDraft.value) return;
    injectDraft.value.floors = injectDraft.value.floors.filter((f) => f.id !== id);
    injectDraft.value.floors.forEach((f, i) => (f.order = i));
    if (editingFloorId.value === id) editingFloorId.value = null;
  }

  /** ↑/↓ 移动(拖拽的兜底操作;移动后统一重排 order) */
  function moveFloor(id: string, dir: -1 | 1): void {
    const fs = injectDraft.value?.floors;
    if (!fs) return;
    const idx = fs.findIndex((f) => f.id === id);
    const to = idx + dir;
    if (idx < 0 || to < 0 || to >= fs.length) return;
    [fs[idx], fs[to]] = [fs[to], fs[idx]];
    fs.forEach((f, i) => (f.order = i));
  }

  /** 导入 JSON / 导出注入配置已收编「综合导入 / 导出」(IMPHUB 步骤 C):
   *  原 onImportPreset / exportInjectConfig 在此删除;ST 预设与 kedai 注入配置的
   *  导入走 ImportBundleModal(形状判定与禁词迁移见 utils/importBundle)。 */

  // ===== 楼层拖拽(HTML5 DnD;drop 后重排 order) =====
  function onDragStart(e: DragEvent, id: string): void {
    dragFloorId.value = id;
    if (e.dataTransfer) e.dataTransfer.effectAllowed = 'move';
  }
  // `_id` 只为模板调用签名对齐,函数体不需要
  function onDragOver(e: DragEvent, _id: string): void {
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = 'move';
  }
  function onDrop(e: DragEvent, targetId: string): void {
    e.preventDefault();
    const fromId = dragFloorId.value;
    dragFloorId.value = null;
    if (!fromId || fromId === targetId) return;
    const fs = injectDraft.value?.floors;
    if (!fs) return;
    const from = fs.findIndex((f) => f.id === fromId);
    const to = fs.findIndex((f) => f.id === targetId);
    if (from < 0 || to < 0) return;
    const [item] = fs.splice(from, 1);
    fs.splice(to, 0, item);
    fs.forEach((f, i) => (f.order = i));
  }
  function onDragEnd(): void {
    dragFloorId.value = null;
  }

  return {
    injectDraft, injectSaving, injectMsg, editingFloorId, dragFloorId,
    loadInjectConfig, saveInjectNow, addFloor, removeFloor, moveFloor,
    onDragStart, onDragOver, onDrop, onDragEnd,
  };
}
