// 宏调试 / 综合导入导出域类型。

/** 历史消息条目({{lastMessage}}/{{firstMessage}} 等宏读取) */
export interface MacroHistoryItem {
  role: string;
  content: string;
}

/** 宏展开上下文(全可选;缺省即空/默认,与服务端引擎缺省语义一致) */
export interface MacroExpandCtx {
  character_name?: string;
  character_description?: string;
  personality?: string;
  scenario?: string;
  user_name?: string;
  user_input?: string;
  history?: MacroHistoryItem[];
  /** 扁平变量 map({{getvar::key}}/{{var::key}} 读取;值为显示字符串) */
  vars?: Record<string, string | number | boolean>;
}

/** 宏展开结果 */
export interface MacroExpandResult {
  expanded: string;
}

/**
 * 综合导出件(bundle v1;与后端 `import_export::ImportBundleEnvelope` 对齐)。
 * 顶层只放元信息,全部内容进 `sections`(未知段键天然可忽略,前向兼容规则清晰);
 * 段序契约由后端具名结构体锁定:preset_entries → flows → prompts → skills → regex_scripts → plugins。
 */
export interface KedaiImportBundle {
  /** 版本键;导入侧 `> 1` 拒绝(「文件由更新版本导出」) */
  kedai_import_bundle: number;
  exported_at: string;
  generator: { app: string; version: string };
  /** 导出方模式(仅供人工识别;导入取段一律以本机当前模式为准) */
  app_mode: string;
  /** 6 段载荷(段缺省 = 该段不导入;`task` 导出天然不含 ①⑥) */
  sections: Record<string, unknown>;
}

/**
 * 综合导入的**逐段回执**(§2.1.4):`ok | skipped | failed`,一段 failed 不阻断其余段。
 * `section` 为段键(如 'flows'),`summary` 面向用户。
 */
export interface ImportSectionResult {
  section: string;
  status: 'ok' | 'skipped' | 'failed';
  summary: string;
  errors: string[];
}
