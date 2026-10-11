// 附件管理的「虚拟文件夹」纯函数(IMPHUB 步骤 D)。
//
// folder 不是真实目录:它只是每个资产自带的一段元数据(技能列 / 流程字段 / 插件文件字段),
// 用 `/` 表达嵌套层级。本模块负责把这串路径规整、去重、展开成可渲染的文件夹树并计数——
// 全部是纯函数,与 Pinia / DOM 无关,故可在 node 环境全量单测。
//
// 「无空文件夹持久化」是硬口径:树里出现的每个路径都由至少一个条目派生,不存独立的文件夹清单;
// 某文件夹下最后一个条目被移走时该文件夹自动消失。因此没有「新建空文件夹」这种操作。
//
// 与后端的关系:路径字符串原样落库(后端只存串、不解析层级),层级语义完全在前端呈现层;
// 故规整规则(下面 normalizeFolderPath)是**唯一**的路径真源,读写两侧都必须经过它。

/** 文件夹路径分隔符(嵌套层级;与后端存储的字符串约定一致) */
export const FOLDER_SEP = '/';

/**
 * 规整虚拟文件夹路径:去首尾空白、逐段 trim、丢弃空段与 `.`/`..` 段、折叠重复分隔符。
 * 返回值空串 = 未分组。
 *
 * 为什么丢弃 `..` 段:路径是纯展示元数据,没有真实文件系统语义;保留 `..` 只会让
 * 树构建出现「回退到父级」的歧义展示。丢弃即归一,不报错(后端宽进)。
 */
export function normalizeFolderPath(raw: string | null | undefined): string {
  if (!raw) return '';
  return raw
    .split(FOLDER_SEP)
    .map((s) => s.trim())
    .filter((s) => s.length > 0 && s !== '.' && s !== '..')
    .join(FOLDER_SEP);
}

/**
 * 校验用户在「新建文件夹」输入框里填的路径;合法返回 null,否则返回中文错误。
 *
 * 空串视为合法(未分组)。规则:每段不得含控制字符(含换行/制表);
 * `..` 段被 normalizeFolderPath 丢弃,故不必报错——但纯 `..` 会规整成空串,
 * 这里明确提示,避免用户以为建了一个叫 `..` 的文件夹却落到未分组。
 */
export function validateFolderPath(raw: string): string | null {
  const text = (raw ?? '').trim();
  if (text === '') return null;
  // 用码点扫描而不是字面量正则:控制字符写进源码会被编辑器/格式化工具悄悄改写,
  // 码点判断对读者也一目了然。
  for (const ch of text) {
    const code = ch.codePointAt(0) ?? 0;
    if (code < 0x20 || code === 0x7f) return '文件夹名不能包含控制字符';
  }
  const normalized = normalizeFolderPath(text);
  if (normalized === '' && text !== '') return '文件夹名无效(不能只由 . / .. 组成)';
  return null;
}

/** 条目上可取 folder 的最小形状 */
export interface HasFolder {
  folder?: string;
}

/** 把路径展开成从根到自身的完整链:`a/b/c` → `['a', 'a/b', 'a/b/c']`(用于建树时补中间层) */
function ancestorPaths(path: string): string[] {
  const segs = path.split(FOLDER_SEP).filter(Boolean);
  const out: string[] = [];
  let cur = '';
  for (const s of segs) {
    cur = cur ? `${cur}${FOLDER_SEP}${s}` : s;
    out.push(cur);
  }
  return out;
}

/** 文件夹树节点(路径派生;仅含实际有条目的路径与其祖先) */
export interface FolderNode {
  /** 完整路径(不含「未分组」根) */
  path: string;
  /** 末段显示名 */
  name: string;
  /** 直接落在本文件夹的条目数(不含子孙) */
  count: number;
  /** 含子孙的条目总数(点击本节点时右侧应显示的条数) */
  total: number;
  children: FolderNode[];
}

/** 文件夹树根(未分组条目 + 顶层文件夹) */
export interface FolderTree {
  /** 未分组条目数(folder 规整后为空串) */
  ungrouped: number;
  /** 顶层文件夹(嵌套子级已建好,按名称排序) */
  folders: FolderNode[];
  /** 全部条目数 */
  total: number;
}

/**
 * 由条目集合构建文件夹树。
 *
 * 计数语义:`count` 是**直接**落在该文件夹的条目数,`total` 含子孙——前者用于文件夹行的
 * 直接计数展示,后者用于「点进去能看到多少条」。未分组条目不进树,单独由 `ungrouped` 承载。
 */
export function buildFolderTree(entries: readonly HasFolder[]): FolderTree {
  const nodes = new Map<string, FolderNode>();
  let ungrouped = 0;

  const ensure = (path: string): FolderNode => {
    const existing = nodes.get(path);
    if (existing) return existing;
    const segs = path.split(FOLDER_SEP);
    const node: FolderNode = {
      path,
      name: segs[segs.length - 1] ?? path,
      count: 0,
      total: 0,
      children: [],
    };
    nodes.set(path, node);
    return node;
  };

  for (const entry of entries) {
    const path = normalizeFolderPath(entry.folder);
    if (!path) {
      ungrouped += 1;
      continue;
    }
    for (const ancestor of ancestorPaths(path)) ensure(ancestor);
    ensure(path).count += 1;
  }

  // 汇总 total(自底向上:先算子节点,再把自己 count 加上去)
  const computeTotal = (node: FolderNode): number => {
    let sum = node.count;
    for (const child of node.children) sum += computeTotal(child);
    node.total = sum;
    return sum;
  };

  const roots: FolderNode[] = [];
  for (const node of nodes.values()) {
    const parentPath = node.path.includes(FOLDER_SEP)
      ? node.path.slice(0, node.path.lastIndexOf(FOLDER_SEP))
      : '';
    if (parentPath) {
      const parent = nodes.get(parentPath);
      if (parent) parent.children.push(node);
    } else {
      roots.push(node);
    }
  }

  const byName = (a: FolderNode, b: FolderNode): number => a.name.localeCompare(b.name);
  const sortDeep = (list: FolderNode[]): void => {
    list.sort(byName);
    for (const n of list) sortDeep(n.children);
  };
  sortDeep(roots);

  let total = ungrouped;
  for (const r of roots) total += computeTotal(r);

  return { ungrouped, folders: roots, total };
}

/**
 * 全部已存在的文件夹路径(去重、按路径排序),供「移动到…」下拉做候选。
 * 不含未分组(空串)——调用方若要提供「移回未分组」应自行在列表头部加一项。
 */
export function listFolderPaths(entries: readonly HasFolder[]): string[] {
  const set = new Set<string>();
  for (const entry of entries) {
    const path = normalizeFolderPath(entry.folder);
    if (path) set.add(path);
  }
  return [...set].sort((a, b) => a.localeCompare(b));
}

/** 取某文件夹的条目(含子孙;`''` = 未分组)。用于右侧列表按选中文件夹过滤。 */
export function entriesInFolder<T extends HasFolder>(entries: readonly T[], folderPath: string): T[] {
  const target = normalizeFolderPath(folderPath);
  if (!target) return entries.filter((e) => normalizeFolderPath(e.folder) === '');
  const prefix = `${target}${FOLDER_SEP}`;
  return entries.filter((e) => {
    const p = normalizeFolderPath(e.folder);
    return p === target || p.startsWith(prefix);
  });
}
