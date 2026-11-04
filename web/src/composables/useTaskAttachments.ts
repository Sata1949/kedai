// 任务附件的前端状态与动作(ATTACH-1,2026-11-03)。
//
// **为什么不做成 store 字段**:`web/src/stores/task.ts` 已被
// `tools/file-size-baseline.json` 锁在 1157 行「只降不升」。附件功能本身不需要跨模块
// 共享的响应式状态——它的两个消费点(侧栏底部入口、任务详情的附件清单)读同一份清单,
// 用模块级单例即可,不必往 store 里加字段、也不必让它再长。
//
// 两类状态:
//   1. **已上传清单**:按任务 id 缓存。真相在服务端(详情顶层也下发),这里是
//      「本次会话已经拿到过」的副本;切任务时按需重拉,避免每次进详情都请求。
//   2. **待上传队列**:用户**还没选中任务**时选的附件。任务创建成功后由
//      `TaskComposer.createAndRun` 调 `flushPending` 逐个上传。
//      队列里放的是 `File` 对象(浏览器内存),切模式/关侧栏都不该丢——故挂在模块级
//      而不是组件级;但**页面刷新后自然消失**,这是可接受的(用户还没提交任何东西)。
import { computed, ref } from 'vue';
import { deleteTaskAttachment, listTaskAttachments, uploadTaskAttachments } from '../api/tasks';
import { registerTaskAttachmentSink } from '../stores/storeBridge';
import type { TaskAttachment } from '../api';

/** 待上传的附件(未选中任务时选择)。带上本地 id 供 UI 去重与移除。 */
export interface PendingAttachment {
  /** 本地唯一 id(不是服务端 id——上传成功后才由后端生成) */
  localId: string;
  file: File;
}

/** 单任务附件个数上限(与后端 `MAX_ATTACHMENTS_PER_TASK` 同值;前端提前拦不产生请求)。 */
export const MAX_ATTACHMENTS_PER_TASK = 10;

/** 单文件字节上限(与后端 `MAX_ATTACHMENT_BYTES` 同值;25MB)。 */
export const MAX_ATTACHMENT_BYTES = 25 * 1024 * 1024;

let localSeq = 0;

// ==================== 模块级单例状态 ====================
// 用 ref 而非 reactive(Map 在 reactive 下也能用,但 ref + 显式替换更不容易踩到
// 「改了 Map 内部却没触发」的坑)。

/** 任务 id → 已上传附件清单 */
const listsByTask = ref(new Map<string, TaskAttachment[]>());
/**
 * 哪些任务的清单**已经有过一次权威来源**(服务端重拉或上传回执)。
 *
 * 为什么不能用 `attachmentsOf(id).length > 0` 代替(ATTFIX-4):空数组既可能是
 * 「读过了,确实一个都没有」,也可能是「还没读过」。两者混淆的直接后果是
 * **删掉最后一个附件后它又冒出来**——缓存变成空数组被判为「没缓存」,于是回落到
 * `currentTask.attachments`(详情快照,还是删之前的样子)。
 */
const loadedTasks = ref(new Set<string>());
/** 当前待上传队列(尚未绑定到任何任务) */
const pending = ref<PendingAttachment[]>([]);
/** 最近一次操作的错误文案(供 UI 显示;空串 = 无错误) */
const lastError = ref('');
/** 正在上传的标志(禁用重复点击) */
const uploading = ref(false);

/**
 * 校验收到的文件并转成待上传条目。
 *
 * **返回被拒的文件名与原因**,而不是静默丢弃:用户选了 12 个文件却只上去 10 个,
 * 必须知道是哪两个、为什么。调用方把 errors 交给 UI 显示。
 */
export function validateFiles(
  files: File[],
  alreadyCount: number,
): { accepted: File[]; errors: string[] } {
  const accepted: File[] = [];
  const errors: string[] = [];
  let count = alreadyCount;
  for (const f of files) {
    if (f.size > MAX_ATTACHMENT_BYTES) {
      errors.push(`${f.name}:超过 ${MAX_ATTACHMENT_BYTES / (1024 * 1024)}MB 上限`);
      continue;
    }
    if (f.size === 0) {
      errors.push(`${f.name}:文件为空`);
      continue;
    }
    if (count >= MAX_ATTACHMENTS_PER_TASK) {
      errors.push(`${f.name}:单任务最多 ${MAX_ATTACHMENTS_PER_TASK} 个附件`);
      continue;
    }
    accepted.push(f);
    count += 1;
  }
  return { accepted, errors };
}

/**
 * 丢弃某任务的清单缓存与加载态(模块级,`useTaskAttachments().forgetTask` 与
 * `stores/task.ts` 经桥回调两条路径共用同一实现)。
 */
function forgetTaskImpl(taskId: string): void {
  if (listsByTask.value.has(taskId)) {
    const next = new Map(listsByTask.value);
    next.delete(taskId);
    listsByTask.value = next;
  }
  // 加载态一并清掉:留着会让新任务(极端情况 id 复用)既不回落详情也不重拉
  if (loadedTasks.value.has(taskId)) {
    const nextLoaded = new Set(loadedTasks.value);
    nextLoaded.delete(taskId);
    loadedTasks.value = nextLoaded;
  }
}

/**
 * 任务附件的状态与动作。**每次调用返回同一份单例状态**(模块级 ref),
 * 因此侧栏与详情面板看到的是同一份清单。
 */export function useTaskAttachments() {
  /** 某任务的附件清单(响应式读) */
  function attachmentsOf(taskId: string | null): TaskAttachment[] {
    if (!taskId) return [];
    return listsByTask.value.get(taskId) ?? [];
  }

  /**
   * 某任务的清单是否**已有权威来源**(ATTFIX-4)。
   *
   * 「已加载且为空」与「从未加载」是两件事:判断该不该回落详情快照必须看这个，
   * 而不是看 `attachmentsOf(id).length > 0`。
   */
  function hasLoaded(taskId: string | null): boolean {
    if (!taskId) return false;
    return loadedTasks.value.has(taskId);
  }

  /** 把某任务的清单标记为已加载(内部用) */
  function markLoaded(taskId: string): void {
    if (loadedTasks.value.has(taskId)) return;
    const next = new Set(loadedTasks.value);
    next.add(taskId);
    loadedTasks.value = next;
  }

  /** 待上传队列的只读视图 */
  const pendingFiles = computed(() => pending.value);

  /** 加入待上传队列(未选中任务时的入口) */
  function enqueue(files: File[]): string[] {
    const { accepted, errors } = validateFiles(files, pending.value.length);
    for (const f of accepted) {
      localSeq += 1;
      pending.value = [...pending.value, { localId: `pa-${localSeq}`, file: f }];
    }
    lastError.value = errors.join(';');
    return errors;
  }

  /** 从待上传队列移除一项 */
  function removePending(localId: string): void {
    pending.value = pending.value.filter((p) => p.localId !== localId);
  }

  /** 清空待上传队列(创建失败/用户取消时) */
  function clearPending(): void {
    pending.value = [];
  }

  /**
   * 把待上传队列刷到指定任务(任务创建成功后调用)。
   *
   * 失败**不阻断任务执行**:附件没上去不该让任务跑不起来,但错误必须留在
   * `lastError` 里让用户看到(静默丢附件是更坏的结果)。
   * 成功上传的从队列里删掉;失败的留在队列里(用户可以重试或手动移除)。
   */
  async function flushPending(taskId: string): Promise<void> {
    if (pending.value.length === 0) return;
    uploading.value = true;
    lastError.value = '';
    try {
      const files = pending.value.map((p) => p.file);
      const uploaded = await uploadTaskAttachments(taskId, files);
      // 全部成功:队列清空,清单并入缓存
      pending.value = [];
      listsByTask.value = new Map(listsByTask.value).set(taskId, [
        ...(listsByTask.value.get(taskId) ?? []),
        ...uploaded,
      ]);
      markLoaded(taskId);
    } catch (err) {
      // 后端按批返回失败原因(整批要么全成要么 400),故队列整体保留
      lastError.value = (err as Error).message;
    } finally {
      uploading.value = false;
    }
  }

  /**
   * 直接上传到已选中的任务(选中任务时点「导入附件」走这条)。
   *
   * 与 `flushPending` 的差别:这里可以逐文件决定成功与否——失败的文件单独报错,
   * 成功的即时并入清单。
   */
  async function uploadTo(taskId: string, files: File[]): Promise<void> {
    // 配额只算**本任务已有**的附件(ATTFIX-11):`pending` 属于还没创建的任务,
    // 与任何已存在任务的配额无关。把它算进来会让「先给新任务攒了 9 个,再给已有
    // 任务传 2 个」被误拒。
    const existing = attachmentsOf(taskId).length;
    const { accepted, errors } = validateFiles(files, existing);
    if (errors.length > 0) lastError.value = errors.join(';');
    else lastError.value = '';
    if (accepted.length === 0) return;
    uploading.value = true;
    try {
      const uploaded = await uploadTaskAttachments(taskId, accepted);
      listsByTask.value = new Map(listsByTask.value).set(taskId, [
        ...(listsByTask.value.get(taskId) ?? []),
        ...uploaded,
      ]);
      markLoaded(taskId);
    } catch (err) {
      lastError.value = (err as Error).message;
    } finally {
      uploading.value = false;
    }
  }

  /**
   * 从服务端重拉某任务的清单(详情面板挂载/切换任务时调用)。
   *
   * 失败只记错误、保留旧值:附件清单读不到不该让详情面板整块空白
   * (与 store 里「详情加载失败保留旧值」的同款容错)。
   */
  async function refresh(taskId: string): Promise<void> {
    try {
      const list = await listTaskAttachments(taskId);
      listsByTask.value = new Map(listsByTask.value).set(taskId, list);
      markLoaded(taskId);
    } catch (err) {
      lastError.value = (err as Error).message;
    }
  }

  /**
   * 删除一个附件。
   *
   * 成功后**重拉清单**而不是本地 splice:后端在「该任务下不存在这个附件」时回 404,
   * 本地说法会与真相分叉(删了 A 却让 B 消失)。失败把原文写进 lastError。
   */
  async function remove(taskId: string, attachmentId: string): Promise<void> {
    try {
      await deleteTaskAttachment(taskId, attachmentId);
      await refresh(taskId);
    } catch (err) {
      lastError.value = (err as Error).message;
    }
  }

  /** 清掉错误(用户关掉提示条时) */
  function dismissError(): void {
    lastError.value = '';
  }

  /** 丢弃某任务的缓存(任务被删除时调用,避免 id 复用读到旧清单) */
  function forgetTask(taskId: string): void {
    forgetTaskImpl(taskId);
  }

  return {
    /** 任务 id → 清单(供模板直接取) */
    listsByTask,
    pendingFiles,
    uploading,
    lastError,
    attachmentsOf,
    hasLoaded,
    enqueue,
    removePending,
    clearPending,
    flushPending,
    uploadTo,
    refresh,
    remove,
    dismissError,
    forgetTask,
  };
}

/** 仅供测试:把模块级单例复位(避免用例间互相污染)。 */
export function resetTaskAttachmentsForTest(): void {
  listsByTask.value = new Map();
  loadedTasks.value = new Set();
  pending.value = [];
  lastError.value = '';
  uploading.value = false;
  localSeq = 0;
}

// 任务被删除时清掉它的清单缓存(ATTFIX-12)。
// 走桥而不是让 `stores/task.ts` import 本模块:store 卡在文件大小基线里只降不升,
// 且反向依赖会让分层倒挂(composable 才是消费 store 的那一侧)。
// 注册放在模块顶层(而非 useTaskAttachments 内):store 可能在任何人调
// useTaskAttachments 之前就删掉一个任务——那时也应该已经能拿到这个回调。
registerTaskAttachmentSink({ forgetTask: (taskId: string) => forgetTaskImpl(taskId) });
