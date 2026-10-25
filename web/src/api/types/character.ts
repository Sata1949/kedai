// 角色 / 会话 / 消息域类型(与后端契约一致)。

export interface CharacterRecord {
  id: string;
  name: string;
  chara_name: string;
  description: string;
  file_path: string;
  avatar_path: string | null;
  data_raw?: Record<string, unknown>;
  /** 开场白(first_mes) */
  first_mes?: string;
  /** 备用开场列表(alternate_greetings;与 first_mes 组成多开场) */
  alternate_greetings?: string[];
  /** 角色卡正则脚本(用于消息 HTML 渲染) */
  regex_scripts?: RegexScript[];
  /** 角色卡内嵌插件检测结果(酒馆助手等;仅详情接口返回) */
  card_plugins?: CardPluginInfo[];
  /** 卡元数据(远程资源页 TavernHelper shim 口令推导用;列表/详情均携带) */
  creator?: string;
  character_version?: string;
  creator_notes?: string;
  created_at: string;
}

/** 角色卡内嵌插件能力点 */
export interface PluginFeature {
  id: string;
  label: string;
  detected: boolean;
}

/** 角色卡内嵌插件(如酒馆助手 SillyTavern-Assistant) */
export interface CardPluginInfo {
  id: string;
  name: string;
  name_en: string;
  /** 检测到即视为启用(kedai 内置实现,无需安装) */
  enabled: boolean;
  /** 来源:character_card(角色卡内嵌) */
  source: string;
  description: string;
  features: PluginFeature[];
}

/** 角色卡正则脚本(extensions.regex_scripts) */
export interface RegexScript {
  id: string;
  script_name: string;
  find_regex: string;
  replace_string: string;
  markdown_only: boolean;
  enabled: boolean;
  /** 酒馆楼层深度下限:仅对深度 >= min_depth 的消息生效(0 = 最新一条);缺省不限 */
  min_depth?: number | null;
  /** 酒馆楼层深度上限:仅对深度 <= max_depth 的消息生效;缺省不限 */
  max_depth?: number | null;
}

export interface SessionInfo {
  id: string;
  character_id: string;
  title: string;
  created_at: string;
  updated_at: string;
}

/** 会话 + 角色名 + 消息数(聊天记录面板) */
export interface SessionWithCharacter extends SessionInfo {
  character_name?: string | null;
  message_count?: number;
}

export interface ChatMessage {
  id: number;
  session_id: string;
  role: 'user' | 'assistant' | 'system';
  content: string;
  /** 服务端宏展开后的显示文本({{char}}/{{getvar}} 等);缺省时前端回退 content */
  content_display?: string;
  extra: Record<string, unknown>;
  created_at: string;
}

export interface StMessage {
  id?: number;
  role: 'user' | 'assistant' | 'system';
  content: string;
  extra?: Record<string, unknown>;
}

/** mvu 变量快照(消息级) */
export interface MvuSnapshot {
  stat_data?: Record<string, unknown>;
  display_data?: Record<string, unknown>;
}

/** 消息记录(含 extra,可含 mvu 快照) */
export interface MessageRecord {
  id: number;
  session_id: string;
  role: 'user' | 'assistant' | 'system';
  content: string;
  extra?: Record<string, unknown>;
  created_at: string;
}

/** 快速回复记录(阶段四 4b;后端 QuickReplyRecord 契约) */
export interface QuickReplyRecord {
  id: number;
  name: string;
  label: string;
  content: string;
  enabled: boolean;
  /** 注入位置权重(0-4,对齐酒馆 injection position;当前仅用于排序) */
  position: number;
  /** 组内排序(升序) */
  sort_order: number;
  created_at?: string;
  updated_at?: string;
}

/** slash 命令元信息(阶段四 4a;后端 GET /api/slash/commands) */
export interface SlashCommandMeta {
  name: string;
  description: string;
  params: string;
}

// ---- 用户脚本(ScriptTree,阶段三;对齐 JS-Slash-Runner「酒馆助手」存储格式) ----

/** 脚本按钮(脚本侧可见的自定义按钮) */
export interface ScriptButton {
  name: string;
  visible?: boolean;
}

/** 单个脚本:可执行体,存于角色卡 extensions.tavern_helper 或全局脚本表 */
export interface ScriptNode {
  type: 'script';
  enabled: boolean;
  name: string;
  /** uuid */
  id: string;
  /** 脚本正文 */
  content: string;
  /** 脚本说明 */
  info?: string;
  button?: {
    enabled: boolean;
    buttons: ScriptButton[];
  };
  /** script 作用域变量(阶段三执行层读取) */
  data?: Record<string, unknown>;
  /** 随卡导出开关(酒馆 export_with) */
  export_with?: {
    data: boolean;
    button: boolean;
  };
}

/** 文件夹:可嵌套一层 script 节点 */
export interface ScriptFolder {
  type: 'folder';
  enabled: boolean;
  name: string;
  /** uuid */
  id: string;
  icon?: string;
  color?: string;
  scripts: ScriptTreeNode[];
}

/** 脚本树节点(顶层数组元素) */
export type ScriptTreeNode = ScriptNode | ScriptFolder;

/** 脚本树:顶层数组(可含 script / folder,文件夹内可再含 script) */
export type ScriptTree = ScriptTreeNode[];
