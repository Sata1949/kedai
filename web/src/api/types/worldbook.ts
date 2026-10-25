// 世界书域类型(记录 / 条目 / 上传转换统计)。

/** 世界书上传时的自动转换统计(后端返回;未发生转换时为 null/缺失) */
export interface WorldBookConversion {
  /** 处理的条目总数 */
  total_count: number;
  /** 实际发生转换的条目数 */
  converted_count: number;
  /** 缺失 constant 时自动判定为常态(常驻)的条数 */
  constant_auto_count: number;
  /** 关键词经字段变体/逗号拆分归一的条数 */
  key_normalized_count: number;
}

/** 世界书记录(列表/详情) */
export interface WorldBookRecord {
  id: string;
  name: string;
  /** 绑定角色 id;缺省 = 全局 */
  character_id?: string | null;
  character_name?: string | null;
  enabled: boolean;
  /** upload=独立上传;character_card=角色卡内嵌 */
  source: string;
  entry_count: number;
  data_raw?: Record<string, unknown>;
  /** 上传时的自动转换统计(仅上传响应返回) */
  conversion?: WorldBookConversion | null;
  created_at: string;
}

/** 世界书条目(预览/编辑;可与后端互写 data_raw) */
export interface WorldBookEntry {
  id: number;
  comment: string;
  /** 触发关键词(子串匹配) */
  keys: string[];
  /** 副关键词(酒馆 keysecondary) */
  keys_secondary?: string[];
  regex?: string | null;
  /** 是否用正则匹配(优先于关键词) */
  use_regex?: boolean;
  /** 常驻注入(酒馆「全局」) */
  constant: boolean;
  /** 激活状态(总开关:激活才参与注入) */
  enabled: boolean;
  content: string;
  /** 注入位置权重(0-4,酒馆 position) */
  position?: number;
  /** 插入深度(酒馆 depth:position=atDepth 时从末尾算注入位置;未声明扫描窗口时兼作窗口兜底) */
  depth?: number;
  /** 关键词扫描窗口(酒馆 scanDepth):关键词只扫最近 N 条消息(0=全部);缺省=未声明,保存时保持原值 */
  scan_depth?: number | null;
  /** 注入顺序(酒馆 order) */
  order?: number;
  /** 关键词大小写敏感 */
  case_sensitive?: boolean;
  /** 粘性(酒馆 sticky;当前版本尚未参与注入判定,仅原样保留) */
  sticky?: number;
  /** 冷却(酒馆 cooldown;当前版本尚未参与注入判定,仅原样保留) */
  cooldown?: number;
  /** 命中概率%(酒馆 probability) */
  probability?: number;
  /** 是否启用概率 */
  use_probability?: boolean;
  /** 注入消息角色(system / user / assistant;缺省:常驻→system,激发→user) */
  role?: 'system' | 'user' | 'assistant' | null;
}
