// 工作区画像 / 任务交付摘要域类型(CODE-4、TMR-6)。

/** 工作区画像的一条命中(CODE-4;`services/workspace_profile.rs` 的同名结构体) */
export interface ProjectTypeHint {
  /** 类型标识(rust / node / python / go / java / dotnet;未知取值原样展示,不假装认识) */
  kind: string;
  /** 证据文件的相对路径(根层即文件名,子目录层形如 `server-rs/Cargo.toml`) */
  marker: string;
  /** 建议的验证命令(后端常量表文案,**不是**实测结论) */
  suggested_command: string;
  /** 命中层级:0 = 工作区根,1 = 直接子目录 */
  depth: number;
}

/**
 * 工作区画像(CODE-4):任务绑定工作区后展示「这是什么项目 + 建议怎么验证」。
 * 详情顶层 `workspace_profile` 与 `GET /api/workspace/profile` 是同一个函数的两处出口;
 * 未绑定工作区 → 详情里为 `null`(前端不渲染该行)。
 */
export interface WorkspaceProfile {
  /** 被探测的工作区根(冻结值原样回显,Windows 上可能含 `\\?\` 前缀) */
  root: string;
  /** 命中项,按后端常量表顺序(rust → node → python → go → java → dotnet) */
  detected: ProjectTypeHint[];
  /** 实际扫描的子目录数(不含根;根恒扫一次) */
  scanned_dirs: number;
  /** 是否因子目录数超上限而未扫全(诚实留痕,不假装扫完) */
  truncated: boolean;
}

/** 交付摘要里的单个必需文件（TMR-6；路径已归一化为工作区相对路径） */
export interface TaskDeliveryFile {
  path: string;
  /** 规划器里的步骤名（含「【主Agent-N】」前缀） */
  owner: string;
  /** 磁盘上当前是否存在（is_file）；工作区不可访问时为 false */
  exists: boolean;
}

/**
 * 任务交付摘要（TMR-6；GET /api/tasks/{id} 顶层 `delivery`）。
 *
 * - `checked=false`：本任务未声明必需文件，只按「本次有没有文件变更」弱校验；
 * - `workspace_available=false`：工作区当前不可访问，`missing` 为空不代表齐全；
 * - `changed` 只列去重后的前 200 条，总数看 `changed_count`。
 */
export interface TaskDeliverySummary {
  checked: boolean;
  workspace_available: boolean;
  required: TaskDeliveryFile[];
  missing: string[];
  changed: string[];
  changed_count: number;
  /**
   * 声明文件的内容体检不通过项（SKY-6：空文件 / 仅空白 / 未替换 `@@` 槽位）。
   * 旧服务端无此字段 → 按「无内容问题」容错。
   */
  content_issues?: string[];
  /**
   * 未完成（步骤 partial）的子目标名（SKY-6；SKY-8 起含预算收尾且声明文件缺失）。
   * 字段名为线格式兼容保留 `truncated`；旧服务端无此字段 → 按「无未完成步骤」容错。
   */
  truncated?: string[];
  /**
   * 根目录未声明的临时/草稿文件（SKY-1 交付卫生，2026-10-25）。
   * 只作提示，不影响任务终态；旧服务端无此字段 → 按「无卫生提示」容错。
   */
  hygiene_issues?: string[];
}
