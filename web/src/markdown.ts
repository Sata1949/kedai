// markdown.ts — 消息 Markdown 渲染(默认开启)
// 安全策略:html:false(LLM 输出不可信,原始 HTML 一律转义),链接新窗口。
// 渲染前剥离 mvu 的 <UpdateVariable> 块(其内容只用于变量更新,不展示)
// 与 <status_current_variable> 段标签(原版 MagVarUpdate 的变量状态段标记)。
// markdown-it v15:默认导出是可调用值(仅值,不可作类型),实例类型需从命名导出取
import MarkdownIt, { type MarkdownIt as MarkdownItInstance } from 'markdown-it';
import { parseUpdateVariable } from './mvu/parser';

/** <status_current_variable> 起止标签剥离(大小写不敏感;内容保留) */
const STATUS_VAR_RE = /<\/?\s*status_current_variable\s*>/gi;

/**
 * 剥离 HTML 注释(含**未闭合的尾部注释**)。酒馆式 HTML 渲染路径在 markdown 化之前调用。
 * 未闭合注释按「至文本结尾」剥除:流式输出下注释常跨帧未闭合,若把 `<!--` 后的半截
 * 文本当正文渲染,注释闭合的下一帧会出现「多一行再消失」的闪字面跳变;
 * 且 md(html:false) 兜底路径会把注释原样转义展示,先剥保证两条路径观感一致。
 */
export function stripHtmlComments(text: string): string {
  return text.replace(/<!--[\s\S]*?-->/g, '').replace(/<!--[\s\S]*$/, '');
}

/** 剥离协议块/段标签:UpdateVariable 块 + status_current_variable 标签 */
export function stripProtocolBlocks(text: string): string {
  let t = parseUpdateVariable(text).cleaned;
  if (STATUS_VAR_RE.test(t)) {
    STATUS_VAR_RE.lastIndex = 0;
    t = t.replace(STATUS_VAR_RE, '');
  }
  return t;
}

let md: MarkdownItInstance | null = null;

/** markdown 扩展插件(由插件框架注册) */
const mdExtensions: Array<(md: MarkdownItInstance) => void> = [];

/** 注册 markdown 扩展(插件框架调用) */
export function registerMarkdownExtension(ext: (md: MarkdownItInstance) => void): void {
  mdExtensions.push(ext);
  md = null; // 触发惰性重建
}

function buildMd(): MarkdownItInstance {
  const instance = new MarkdownIt({
    html: false,
    linkify: true,
    breaks: true,
    typographer: false,
  });

  // 链接:新窗口 + rel 安全属性
  const defaultLink = instance.renderer.rules.link_open ?? ((tokens, idx, options, _env, self) => self.renderToken(tokens, idx, options));
  instance.renderer.rules.link_open = (tokens, idx, options, env, self) => {
    tokens[idx].attrSet('target', '_blank');
    tokens[idx].attrSet('rel', 'noopener noreferrer');
    return defaultLink(tokens, idx, options, env, self);
  };

  for (const ext of mdExtensions) ext(instance);
  return instance;
}

function ensureMd(): MarkdownItInstance {
  if (!md) md = buildMd();
  return md;
}

/** 渲染 markdown 到安全 HTML(默认供 v-html 使用) */
export function renderMarkdown(text: string): string {
  if (!text) return '';
  // 剥离 mvu UpdateVariable 块与 status_current_variable 段标签
  const cleaned = stripProtocolBlocks(text);
  if (!cleaned.trim()) return '';
  return ensureMd().render(cleaned);
}

/** 酒馆式消息渲染的 markdown 实例(html:true,保留消息内原始 HTML 标签)
 *  与默认实例分开构建:html:false 是默认路径的安全基线(LLM 输出一律转义),
 *  不因「HTML 渲染」开关的存在而放宽。本实例的产出**未经白名单**,
 *  调用方必须再过 render.ts 的 sanitizeMessageHtml(消息专用白名单)后才可 v-html。 */
let mdHtml: MarkdownItInstance | null = null;

function buildMdHtml(): MarkdownItInstance {
  const instance = new MarkdownIt({
    html: true,
    linkify: true,
    breaks: true,
    typographer: false,
  });

  // 链接:新窗口 + rel 安全属性(与默认实例一致);href 协议白名单由 sanitizeMessageHtml 把关
  const defaultLink = instance.renderer.rules.link_open ?? ((tokens, idx, options, _env, self) => self.renderToken(tokens, idx, options));
  instance.renderer.rules.link_open = (tokens, idx, options, env, self) => {
    tokens[idx].attrSet('target', '_blank');
    tokens[idx].attrSet('rel', 'noopener noreferrer');
    return defaultLink(tokens, idx, options, env, self);
  };

  for (const ext of mdExtensions) ext(instance);
  return instance;
}

function ensureMdHtml(): MarkdownItInstance {
  if (!mdHtml) mdHtml = buildMdHtml();
  return mdHtml;
}

/**
 * 渲染消息 markdown,并保留原始 HTML 标签(酒馆式 HTML 渲染路径专用)。
 * 先在文本层剥协议块与 HTML 注释,再走 html:true 实例;产出为「未净化 HTML」,
 * 调用方必须过 sanitizeMessageHtml(白名单会丢弃 style/class/事件等一切不被允许的属性)。
 */
export function renderMarkdownWithHtml(text: string): string {
  if (!text) return '';
  const cleaned = stripHtmlComments(stripProtocolBlocks(text));
  if (!cleaned.trim()) return '';
  return ensureMdHtml().render(cleaned);
}

/** 仅剥离协议块/段标签(供纯文本场景复用) */
export function stripMvuBlocks(text: string): string {
  return stripProtocolBlocks(text);
}

/** 导出 md 实例(供测试/高级插件) */
export function getMarkdownIt(): MarkdownItInstance {
  return ensureMd();
}
