// 工具 / 授权域类型(风险等级、授权模式、插件工具清单)。

export type ToolRisk = 'safe' | 'sensitive' | 'dangerous';

/** 授权模式三档(与后端 AuthorizationMode 一致)。
 *  strict:读/写/删文件都需授权;
 *  loose:读/写文件放行,删文件需授权(默认);
 *  bypass:除「系统路径(C 盘)写/删」外一律放行。 */
export type AuthorizationMode = 'strict' | 'loose' | 'bypass';

export interface ToolPermission {
  name: string;
  description: string;
  risk: ToolRisk;
  allowed: boolean;
  reason: string;
}

/** 插件工具信息(自定义工具插件;后端只回 Plugin 来源,内置/MCP 工具不在此列) */
export interface ToolPluginInfo {
  name: string;
  description: string;
  /** 来源标识(本端点恒为 'plugin';与 SSE 工具事件 origin 字段同词表) */
  origin: string;
  /** 来源文件基名(UI 按文件分组展示) */
  file: string;
  /** 虚拟文件夹路径(IMPHUB 附件管理;空串 = 未分组) */
  folder: string;
}

export interface PluginToolsStatus {
  tools: ToolPluginInfo[];
  files: string[];
}
