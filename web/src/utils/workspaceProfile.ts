// 工作区画像的展示辅助(CODE-4)。
//
// 单一出处:创建表单(Sidebar)与任务详情(TaskBoard)两处都要把后端的类型标识渲染成人看的名,
// 各写一份必然漂移(后端的 kind 是线格式,前端展示名是另一回事)。

/** 类型标识 → 展示名。**未登记取值原样回显**——后端加了新类型时,前端显示原始标识比假装认识更诚实。 */
const KIND_LABELS: Record<string, string> = {
  rust: 'Rust',
  node: 'Node',
  python: 'Python',
  go: 'Go',
  java: 'Java',
  dotnet: '.NET',
};

export function projectKindLabel(kind: string): string {
  return KIND_LABELS[kind] ?? kind;
}

/** 一条命中的 tooltip:证据文件 + 建议命令(命令是常量表文案,不宣称已核实可用)。 */
export function projectHintTitle(kind: string, marker: string, command: string): string {
  return `${projectKindLabel(kind)}:${marker} → 建议验证:${command}`;
}
