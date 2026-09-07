// 消息 HTML 渲染工具
// 流程:先在原始文本上把角色卡正则脚本(regex_scripts)命中的片段替换为唯一占位符,
//       再对整段做 HTML 转义(LLM 输出不可信,防注入),最后把占位符还原为脚本自带的
//       replaceString(作者可信 HTML)。
// 排版修复:v1 无脑剥离 <script> 且注入 <style> 为全局作用域,导致状态栏脚本不执行、
//           @keyframes 等命名污染整个页面。本版:
//           - 样式作用域化:每条规则加祖先前缀(等效 @scope 降级),keyframes 重命名
//           - 脚本受控执行:提取 <script> 源码(剥 import),由调用方在容器挂载后执行
//           - <UpdateVariable> 块由 mvu 解析器剥离,不进入渲染
import sanitizeHtml from 'sanitize-html';
import type { RegexScript } from './api';
import { parseUpdateVariable } from './mvu/parser';
import { renderMarkdown } from './markdown';

/** HTML 转义(防注入) */
export function escapeHtml(s: string): string {
  return s
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;');
}

/** JS 风格正则拆分为 (pattern, flags);非 /.../ 形式按普通字符串处理 */
export function splitJsRegex(input: string): { pattern: string; flags: string } | null {
  const t = input.trim();
  if (t.length >= 2 && t.startsWith('/')) {
    const m = /^\/([\s\S]*)\/([gimsuy]*)$/.exec(t);
    if (m) return { pattern: m[1], flags: m[2] };
  }
  return { pattern: t, flags: '' };
}

/**
 * 展开 replace_string 中的捕获组引用,与 String.prototype.replace 语义一致:
 * `$&` 匹配全文、`$1`..`$n` 捕获组、`$$` 字面量 $、`` $` `` 匹配前文本、`$'` 匹配后文本。
 * 函数式 replace 回调的返回值不会做 $ 展开,角色卡脚本 find:/...(\d+).../ replace:'<b>$1</b>'
 * 会原样输出 $1——这里补上标准展开。args 为回调原始参数(match, ...groups, offset, input)。
 */
export function expandReplaceRefs(replaceString: string, args: unknown[]): string {
  // 对齐 SillyTavern regex 引擎(engine.js runRegexScript):替换串经函数返回值插入,
  // 引擎只手工展开 {{match}}/$0、$n、$<name>;其余 $ 序列($$、$&、$`、$')一律字面保留——
  // 作者模板字面量里的 `$` 组合(如 wuwa 卡 `(.*)$\``)不会被吞,且 $& 在 ST 中也是字面量。
  const withMatch = replaceString.replace(/{{match}}/gi, '$0');
  const match = args[0] as string;
  // String.replace 回调参数:[match, p1..pn, offset, input] 或带命名组时尾部再跟 groups
  const maybeGroups = args[args.length - 1];
  const namedGroups =
    maybeGroups !== null && typeof maybeGroups === 'object'
      ? (maybeGroups as Record<string, string | undefined>)
      : undefined;
  const offsetIndex = namedGroups ? args.length - 3 : args.length - 2;
  const groupCount = offsetIndex - 1;
  return withMatch.replaceAll(/\$(\d+)|\$<([^>]+)>/g, (_m, num: string | undefined, name: string | undefined) => {
    if (name !== undefined) return namedGroups?.[name] ?? '';
    const i = Number(num);
    if (i === 0) return match;
    return i <= groupCount ? ((args[i] as string | undefined) ?? '') : '';
  });
}

/** 显示层身份宏上下文:charName 当前角色名,userName 用户名(与后端默认一致为「用户」) */
export interface DisplayMacroCtx {
  charName?: string;
  userName?: string;
}

/**
 * 展开脚本 replace_string 注入的身份宏({{user}}/{{char}} 等,大小写不敏感)。
 * 对齐 SillyTavern:显示层 substituteParams 作用于脚本替换后的完整文本——脚本输出里
 * 新引入的宏(含 <script> 源码内的 '{{user}}',如 wuwa 状态栏 sex-monitor)也会展开。
 * 只处理身份宏;变量类宏({{getvar}} 等)由后端 content_display 或 mvu 宿主负责。
 */
export function expandDisplayMacros(text: string, ctx: DisplayMacroCtx): string {
  const charName = ctx.charName ?? '';
  const userName = ctx.userName ?? '用户';
  return text
    .replace(/\{\{\s*(?:user|user_name)\s*\}\}/gi, userName)
    .replace(/\{\{\s*(?:char|char_name|character_name)\s*\}\}/gi, charName);
}

/** 提取 <script> 源码(剥掉 import 语句,避免加载外部依赖) */
export function extractScripts(s: string): string[] {
  const out: string[] = [];
  const re = /<script\b[^>]*>([\s\S]*?)<\/script\s*>/gi;
  let m: RegExpExecArray | null;
  while ((m = re.exec(s)) !== null) {
    let code = m[1].trim();
    if (!code) continue;
    // 剥 import '...'; 语句(单/双引号)
    code = code.replace(/\bimport\s+(['"][^'"]+['"])\s*;?\s*/g, '');
    // 剥 import { x } from '...' 与 import x from '...'
    code = code.replace(
      /\bimport\s+(?:\{[\s\S]*?\}\s*from\s*)?(?:[\w$]+\s*,\s*)?\{?[\s\S]*?\}?\s*from\s*['"][^'"]+['"]\s*;?/g,
      '',
    );
    // 剥动态 import('...')
    code = code.replace(/\bimport\s*\(\s*['"][^'"]+['"]\s*\)\s*;?/g, '');
    if (code.trim()) out.push(code.trim());
  }
  return out;
}

/** 提取并移除 <style> 内容 */
export function extractStyles(s: string): string[] {
  const out: string[] = [];
  const re = /<style\b[^>]*>([\s\S]*?)<\/style\s*>/gi;
  let m: RegExpExecArray | null;
  while ((m = re.exec(s)) !== null) {
    if (m[1].trim()) out.push(m[1].trim());
  }
  return out;
}

/**
 * 样式清洗(renderHtml 逐卡信任模型下的保真版)。
 *
 * renderHtml 是用户逐卡主动开启的渲染开关(与 JS 授权同属信任模型),角色卡界面
 * (悬浮球/弹窗/全屏遮罩/状态栏定位)依赖 position/z-index/url() 等声明,过度清洗
 * 会让卡片「只有文字没有界面」(wuwa 卡 bug)。放行:position/z-index/inset/四向
 * 偏移/pointer-events/cursor/content、vw/vh 视口单位、https 与 data:image 的 url()。
 * 仍禁:behavior/binding(IE 脚本绑定)、expression()、@import、javascript:,
 * 以及非白名单的 url()(http 混合内容、data: 文档、file: 本地读取)。
 */
export function sanitizeScopedCss(css: string): string {
  const cleanBlocks = (input: string, keyframe = false): string => splitCssBlocks(input)
    .map(({ head, inner, isAt }) => {
      const name = head.trim();
      if (isAt) {
        if (/^@(?:-webkit-)?keyframes\s+[\w-]+$/i.test(name)) {
          return `${name}{${cleanBlocks(inner, true)}}`;
        }
        return '';
      }
      const body = sanitizeCssDeclarations(inner);
      if (!body) return '';
      if (keyframe && !/^(?:from|to|(?:\d{1,3}(?:\.\d+)?)%)(?:\s*,\s*(?:from|to|(?:\d{1,3}(?:\.\d+)?)%))*$/i.test(name)) return '';
      return `${name} { ${body} }`;
    })
    .filter(Boolean)
    .join('\n');

  return cleanBlocks(css.replace(/<\/style/gi, ''));
}

/** url() 逐条白名单:仅 https 与 data:image(外链图/内联图);无 url() 直接放行 */
export function cssUrlsSafe(value: string): boolean {
  const re = /url\s*\(\s*(['"]?)([^'")]*)\1\s*\)/gi;
  let m: RegExpExecArray | null;
  while ((m = re.exec(value)) !== null) {
    const u = m[2].trim();
    if (!/^https:\/\//i.test(u) && !/^data:image\//i.test(u)) return false;
  }
  return true;
}

/**
 * CSS 声明清洗(规则体与 style 属性共用):
 * 剥 behavior/binding、expression()/@import/javascript:、非白名单 url();
 * 属性名限小写字母/连字符或 -- 自定义属性。
 */
export function sanitizeCssDeclarations(input: string): string {
  const deniedProperties = new Set(['behavior', 'binding', '-moz-binding']);
  const unsafeValue = /expression\s*\(|@import|javascript:/i;
  return splitCssDeclarations(input)
    .map((declaration) => {
      const colon = declaration.indexOf(':');
      if (colon <= 0) return '';
      const property = declaration.slice(0, colon).trim().toLowerCase();
      const value = declaration.slice(colon + 1).trim();
      if (!/^--[\w-]+$|^[a-z-]+$/.test(property)) return '';
      if (deniedProperties.has(property) || unsafeValue.test(value) || /<\/style/i.test(value)) return '';
      if (!cssUrlsSafe(value)) return '';
      return `${property}: ${value}`;
    })
    .filter(Boolean)
    .join('; ');
}

/** style 属性清洗(声明级,与规则体同一管线;供 sanitizeVisibleHtml 的 transformTags 用) */
export function sanitizeStyleAttribute(style: string): string {
  return sanitizeCssDeclarations(style);
}

function splitCssDeclarations(input: string): string[] {
  const declarations: string[] = [];
  let start = 0;
  let quote: string | null = null;
  let parentheses = 0;
  for (let index = 0; index <= input.length; index++) {
    const char = input[index];
    if (quote) {
      if (char === '\\') index++;
      else if (char === quote) quote = null;
    } else if (char === '"' || char === "'") quote = char;
    else if (char === '(') parentheses++;
    else if (char === ')') parentheses = Math.max(0, parentheses - 1);
    else if ((char === ';' || index === input.length) && parentheses === 0) {
      declarations.push(input.slice(start, index).trim());
      start = index + 1;
    }
  }
  return declarations;
}

/**
 * 内联事件处理器降级为数据属性(onclick="fn(this)" → data-kd-onclick="fn(this)")。
 * 宿主文档绝不执行卡内 inline 代码(安全边界不变:真实 on* 属性仍被清洗);
 * 宿主按属性生成监听,触发时把代码串 postMessage 回该块脚本所属沙箱,在沙箱内求值
 * (见 characterScriptSandbox 的 inline-event 分支;sandbox.html CSP 含 unsafe-eval,
 * 求值对象仅限作者自己的代码,与已授权执行的卡脚本同信任级)。
 * wuwa 状态栏 72 处 onclick(标签页切换等)依赖此链路。
 */
export function demoteInlineHandlers(html: string): string {
  return html.replace(
    /<([a-zA-Z][a-zA-Z0-9-]*)((?:"[^"]*"|'[^']*'|[^>"'])+)>/g,
    (whole: string, tag: string, attrs: string): string => {
      const rewritten = attrs.replace(
        /\s(on[a-zA-Z]+)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))/g,
        (_m, name: string, dq?: string, sq?: string, bare?: string): string => {
          const value = dq ?? sq ?? bare ?? '';
          // 只转义双引号:& 必须原样透传,否则值内已有实体(&quot; 等)会被二次转义,
          // 浏览器属性解析只解码一次,二次转义后沙箱拿到的是未解码的残串
          const escaped = value.replace(/"/g, '&quot;');
          return ` data-kd-${name.toLowerCase()}="${escaped}"`;
        },
      );
      return `<${tag}${rewritten}>`;
    },
  );
}

/**
 * 可见 HTML 白名单:不允许事件处理器、外部媒体或 javascript URL。
 * 链接与图片仅放行 https(与资源界面一致):角色卡作者 HTML 中的外链图(如
 * WuWa 开场界面的 Logo)与站外链接需要保留;data: 仍允许(状态栏内联图)。
 * style 属性放行(值过 sanitizeStyleAttribute 声明级清洗):作者界面的内联定位/
 * 显隐依赖它,整段剥掉会破坏布局(renderHtml 为逐卡主动开启的信任模型)。
 * 表单控件(input/select/textarea/option)放行:角色卡开场界面(如 WuWa 开场
 * 身份/版本/区域选择)依赖表单交互;disabled/checked/selected/value 等状态
 * 属性保留,事件处理器一律剥离(交互经 jQuery 子集绑定)。
 */
export function sanitizeVisibleHtml(html: string): string {
  return sanitizeHtml(html, {
    allowedTags: [
      'div', 'span', 'p', 'br', 'hr', 'strong', 'em', 'b', 'i', 'u', 's', 'small',
      'ul', 'ol', 'li', 'dl', 'dt', 'dd', 'table', 'thead', 'tbody', 'tfoot', 'tr', 'th', 'td',
      'details', 'summary', 'blockquote', 'code', 'pre', 'a', 'img', 'h1', 'h2', 'h3', 'h4', 'h5', 'h6',
      // button 保留:状态栏脚本的标签页切换依赖 <button>(事件经 jQuery 子集绑定,属性被清洗)
      'button',
      // 表单控件:开场界面(身份/版本/区域选择、协议勾选)依赖输入交互
      'form', 'label', 'input', 'select', 'option', 'optgroup', 'textarea', 'fieldset', 'legend',
    ],
    allowedAttributes: {
      // id 必须保留:状态栏脚本(jQuery 子集)用 #id 选择器定位元素,剥离后脚本静默空转;
      // style 值经 transformTags 清洗(声明级,与规则体同一管线)
      '*': ['class', 'title', 'aria-label', 'role', 'data-*', 'id', 'style'],
      a: ['href', 'title', 'target', 'rel', 'class', 'aria-label', 'style'],
      img: ['src', 'alt', 'title', 'class', 'width', 'height', 'style'],
      th: ['colspan', 'rowspan', 'scope', 'class', 'style'],
      td: ['colspan', 'rowspan', 'class', 'style'],
      form: ['action', 'method', 'class', 'id', 'style'],
      label: ['for', 'class', 'id', 'style'],
      input: ['type', 'name', 'value', 'placeholder', 'checked', 'disabled', 'min', 'max', 'maxlength', 'size', 'class', 'id', 'style'],
      select: ['name', 'multiple', 'disabled', 'size', 'class', 'id', 'style'],
      option: ['value', 'selected', 'disabled', 'class', 'id', 'style'],
      optgroup: ['label', 'disabled', 'class', 'id', 'style'],
      textarea: ['name', 'rows', 'cols', 'placeholder', 'disabled', 'maxlength', 'class', 'id', 'style'],
      fieldset: ['disabled', 'class', 'id', 'style'],
    },
    allowedSchemes: ['data', 'https'],
    allowedSchemesByTag: { img: ['data', 'https'] },
    allowProtocolRelative: false,
    transformTags: {
      // style 属性声明级清洗(与 <style> 规则体同一管线:剥 expression/javascript:/
      // 非白名单 url() 与 behavior 绑定;position 等布局声明放行,见 sanitizeScopedCss)
      '*': (tagName, attribs) => {
        if (typeof attribs.style !== 'string') return { tagName, attribs };
        const clean = sanitizeStyleAttribute(attribs.style);
        const next = { ...attribs };
        if (clean) next.style = clean;
        else delete next.style;
        return { tagName, attribs: next };
      },
      a: (_tagName, attribs) => ({
        tagName: 'a',
        attribs: { ...attribs, target: '_blank', rel: 'noopener noreferrer' },
      }),
    },
    disallowedTagsMode: 'discard',
  });
}

/** 移除 head/script/style 骨架,仅保留 body 内部可见内容 */
function extractBody(s: string): string {
  let t = s;
  const headMatch = /<head\b[\s\S]*?<\/head>/i.exec(t);
  if (headMatch) {
    const bodyMatch = /<body\b[^>]*>([\s\S]*?)<\/body>/i.exec(t);
    t = bodyMatch ? bodyMatch[1] : t.slice(headMatch[0].length);
  }
  // 兜底:无 head 时剥掉残余 script/style 骨架
  t = t.replace(/<script\b[\s\S]*?<\/script\s*>/gi, '');
  t = t.replace(/<style\b[\s\S]*?<\/style\s*>/gi, '');
  return t.trim();
}

/**
 * 单条选择器作用域化。作者 CSS 按整文档思维常以前导 body/html 为根(如
 * `body.theme-blue .tide-card`,wuwa 状态栏 35 处)——作用域容器就是「文档根」:
 * 前导 body/html 重写为容器自身,紧邻 body 的类/伪类/属性选择器落到容器上
 * (与沙箱 $('body') → 容器的特判对称;$('body').addClass('theme-blue') 因而生效)。
 */
function scopeSelector(s: string, attr: string): string {
  if (!s || s.startsWith(attr)) return s;
  const m = /^(?:html(?![\w-])\s+)?body(?![\w-])((?:[.:#][\w-]+(?:\([^)]*\))?|\[[^\]]*\])*)([\s\S]*)$/.exec(s);
  if (m) return `${attr}${m[1] ?? ''}${m[2] ?? ''}`;
  return `${attr} ${s}`;
}

/**
 * CSS 作用域化:每条规则加祖先前缀 `[data-kd-scope="scopeId"]`。
 * - 普通规则:选择器加前缀(已是前缀则跳过;前导 body/html 重写为容器,见 scopeSelector)
 * - @media/@supports:内部规则递归加前缀
 * - @keyframes:名字重命名(前缀 + 原哈希),并记录映射供引用替换
 * - @font-face/@import 等:原样保留
 */
export function scopeCss(css: string, scopeId: string): { css: string; keyframes: Map<string, string> } {
  const attr = `[data-kd-scope="${scopeId}"]`;
  const kfMap = new Map<string, string>();
  const out: string[] = [];

  // 按顶层块切分:selector{...} / @rule{...} / 顶层声明
  const blocks = splitCssBlocks(css);
  for (const block of blocks) {
    const { head, inner, isAt } = block;
    if (isAt) {
      const name = head.trim();
      if (/^@(media|supports|container|layer)\b/i.test(name)) {
        const sub = scopeCss(inner, scopeId);
        sub.keyframes.forEach((v, k) => kfMap.set(k, v));
        out.push(`${name}{${sub.css}}`);
      } else if (/^@keyframes\b/i.test(name)) {
        // 重命名 keyframes 名字
        const orig = name.replace(/^@keyframes\s+/i, '').trim();
        const renamed = `kd-${scopeId}-${orig}`;
        kfMap.set(orig, renamed);
        out.push(`@keyframes ${renamed}{${inner}}`);
      } else {
        // @font-face / @import / @page / @charset 等原样
        out.push(inner ? `${name}{${inner}}` : name);
      }
    } else {
      const sel = head.trim();
      if (!sel) continue;
      const scopedSel = sel
        .split(',')
        .map((s) => scopeSelector(s.trim(), attr))
        .join(', ');
      out.push(`${scopedSel} {${inner}}`);
    }
  }
  return { css: out.join('\n'), keyframes: kfMap };
}

/** 应用 keyframes 重命名到 CSS 文本(animation/animation-name 中的名字)。
 *  负向断言排除已带前缀的 keyframes 定义名,避免双重前缀。 */
export function applyKeyframeRename(css: string, map: Map<string, string>): string {
  if (map.size === 0) return css;
  let s = css;
  for (const [orig, renamed] of map) {
    const re = new RegExp(`(?<![\\w-])${escapeRegex(orig)}(?![\\w-])`, 'g');
    s = s.replace(re, renamed);
  }
  return s;
}

function escapeRegex(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

/** 切分 CSS 为顶层块:[{head, inner, isAt}] */
function splitCssBlocks(css: string): Array<{ head: string; inner: string; isAt: boolean }> {
  const blocks: Array<{ head: string; inner: string; isAt: boolean }> = [];
  let i = 0;
  const n = css.length;
  while (i < n) {
    // 跳过注释与空白
    if (css.startsWith('/*', i)) {
      const end = css.indexOf('*/', i + 2);
      i = end < 0 ? n : end + 2;
      continue;
    }
    if (/\s/.test(css[i])) {
      i++;
      continue;
    }
    // 收集 head(直到 '{' 或 ';',跳过引号内的分号)
    let headStart = i;
    let j = i;
    let headQuote: string | null = null;
    while (j < n) {
      const ch = css[j];
      if (headQuote) {
        if (ch === headQuote) headQuote = null;
      } else if (ch === '"' || ch === "'") {
        headQuote = ch;
      } else if (ch === '{' || ch === ';') {
        break;
      }
      j++;
    }
    if (j >= n) break;
    if (css[j] === ';') {
      // 顶层声明(如 @import 分号结尾)或空规则,原样保留
      blocks.push({ head: css.slice(headStart, j + 1), inner: '', isAt: css[headStart] === '@' });
      i = j + 1;
      continue;
    }
    // 有 '{':扫描匹配的 '}'(注意字符串)
    let depth = 1;
    let k = j + 1;
    let quote: string | null = null;
    while (k < n && depth > 0) {
      const ch = css[k];
      if (quote) {
        if (ch === quote) quote = null;
      } else if (ch === '"' || ch === "'") {
        quote = ch;
      } else if (ch === '{') {
        depth++;
      } else if (ch === '}') {
        depth--;
      }
      k++;
    }
    blocks.push({
      head: css.slice(headStart, j),
      inner: css.slice(j + 1, depth === 0 ? k - 1 : k),
      isAt: css[headStart] === '@',
    });
    i = depth === 0 ? k : k + 1;
  }
  return blocks;
}

export interface ScopedScriptHtml {
  /** 可渲染 HTML(含作用域化 style 内联 + body 内容) */
  html: string;
  /** 需执行的脚本源码(已剥 import) */
  scripts: string[];
  /** 容器唯一 scopeId */
  scopeId: string;
}

let scopeSeq = 0;

/**
 * 把脚本的 replaceString 转成作用域化 HTML + 待执行脚本。
 * 生成结构:
 *   <div data-kd-scope="{scopeId}">
 *     <style>{作用域化样式}</style>
 *     {body 可见内容}
 *   </div>
 * 脚本调用方在容器挂载后执行(先 setMvuHostContext,再 flushJqReady)。
 * seed 可选:传入后 scopeId 稳定为 `k{seed}`(消息渲染复用同一容器,避免脚本重复执行)。
 */
export function buildScopedScriptHtml(replaceString: string, seed?: string): ScopedScriptHtml {
  let s = replaceString;
  // 1) 剥 markdown 代码围栏
  const fence = /^```(?:html|html\.handlebars)?\s*\r?\n?/i;
  const fenceEnd = /\r?\n?```\s*$/;
  if (fence.test(s)) s = s.replace(fence, '');
  if (fenceEnd.test(s)) s = s.replace(fenceEnd, '');

  const styles = extractStyles(s);
  const scripts = extractScripts(s);
  // inline 事件先降级为 data-kd-on*(可存活过白名单),宿主绑监听转发沙箱求值
  const body = sanitizeVisibleHtml(demoteInlineHandlers(extractBody(s)));
  // 替换值经清洗后无可见内容且无脚本(如「状态栏代码块」把占位符包成围栏,
  // 围栏内标签被白名单丢弃)时返回空,调用方跳过注入,避免消息尾部出现空容器
  if (!body.trim() && scripts.length === 0) {
    return { html: '', scripts, scopeId: '' };
  }
  const scopeId = seed ? `k${seed}` : `k${Date.now().toString(36)}${(scopeSeq++).toString(36)}`;

  let cssText = '';
  for (const css of styles) {
    const scoped = scopeCss(sanitizeScopedCss(css), scopeId);
    cssText += applyKeyframeRename(scoped.css, scoped.keyframes) + '\n';
  }

  const html = `<div data-kd-scope="${scopeId}">${cssText ? `<style>${cssText}</style>` : ''}${body}</div>`;
  return { html, scripts, scopeId };
}

/**
 * 对消息文本应用角色卡正则脚本,返回可渲染 HTML。
 * 兼容旧签名(纯 HTML,不含脚本执行)——ChatWindow 改用 buildScopedScriptHtml 以获得脚本。
 * 1) 原文上按脚本链顺序把命中片段替换为占位符(控制字符,不受转义影响)
 * 2) 整体 HTML 转义(LLM 输出不可信)
 * 3) 占位符还原为脚本 replaceString(作者可信,清洗后保留 HTML)
/**
 * 酒馆楼层深度过滤:depth 0 = 最新一条消息,向历史递增。脚本带 min_depth/max_depth
 * 时仅对范围内楼层生效(如 wuwa 卡「删除远楼层开场标记」minDepth=2,只清理历史
 * 楼层,当前开场的占位符须留给渲染脚本)。depth 未传表示不过滤(兼容旧调用)。
 */
export function scriptAppliesAtDepth(script: RegexScript, depth: number | undefined): boolean {
  if (depth === undefined) return true;
  if (script.min_depth != null && depth < script.min_depth) return false;
  if (script.max_depth != null && depth > script.max_depth) return false;
  return true;
}

/**
 * 无效正则 / 空替换串 / 禁用的脚本跳过。
 */
export function renderScriptedHtml(text: string, scripts: RegexScript[], depth?: number, macros?: DisplayMacroCtx): string {
  // 第 1 步:原文占位符化
  const placeholders: Array<{ ph: string; value: string }> = [];
  let source = text;
  for (const script of scripts) {
    if (!scriptAppliesAtDepth(script, depth)) continue;
    if (!script.enabled) continue;
    if (!script.replace_string) continue;
    const parsed = splitJsRegex(script.find_regex);
    if (!parsed || !parsed.pattern) continue;
    let re: RegExp;
    try {
      re = new RegExp(parsed.pattern, parsed.flags.includes('g') ? parsed.flags : parsed.flags + 'g');
    } catch {
      continue; // 无效正则跳过
    }
    source = source.replace(re, (...args: unknown[]) => {
      const ph = `KDAI_PH_${placeholders.length}`;
      let value = expandReplaceRefs(script.replace_string, args);
      if (macros) value = expandDisplayMacros(value, macros);
      placeholders.push({ ph, value: cleanScriptHtml(value) });
      return ph;
    });
  }
  if (placeholders.length === 0) {
    // 无脚本命中:直接转义输出(纯文本)
    return escapeHtml(text);
  }
  // 第 2 步:整体转义(占位符为控制字符,不受影响)
  let html = escapeHtml(source);
  // 第 3 步:还原占位符(不二次转义)
  for (const { ph, value } of placeholders) {
    html = html.split(ph).join(value);
  }
  return html;
}

export interface ScopedRenderResult {
  /** 完整可 v-html 的 HTML(含全部作用域容器) */
  html: string;
  /** 待执行脚本块:scopeId 对应容器,scripts 为源码 */
  blocks: Array<{ scopeId: string; scripts: string[] }>;
}

/**
 * 状态栏「渲染脚本」判定:启用、有替换串、正则应 <StatusPlaceHolderImpl/>,
 * 且替换串里不含占位符本身(不含 = 直接把占位符渲染成界面;含 = 只是把占位符
 * 包回代码围栏的「包裹脚本」,如碧蓝卡「状态栏代码块」,不应抢在渲染脚本前)。
 * 渲染脚本必须排在包裹脚本之前执行,否则占位符先被包裹脚本吞掉,渲染脚本
 * 永远匹配不到(碧蓝卡「ui」手机界面空白)。
 */
function isStatusRenderScript(s: RegexScript): boolean {
  return (
    s.enabled &&
    !!s.replace_string &&
    /StatusPlaceHolderImpl/i.test(s.find_regex) &&
    !/StatusPlaceHolderImpl/i.test(s.replace_string)
  );
}

/**
 * 应用脚本后返回渲染结果:完整 html + 待执行脚本块。
 * 未命中任何脚本返回 null(由调用方走 markdown 渲染)。
 * seed 可选:稳定 scopeId(同消息重渲染复用同一容器,避免脚本重复执行)。
 * ChatWindow 一次性 v-html html;DOM 挂载后逐块 setMvuHostContext 并执行 scripts。
 */
export function renderScopedScripts(
  text: string,
  scripts: RegexScript[],
  seed?: string,
  depth?: number,
  macros?: DisplayMacroCtx,
): ScopedRenderResult | null {
  // 先剥离 mvu <UpdateVariable> 块(仅用于变量更新,不进入任何渲染)
  const { cleaned } = parseUpdateVariable(text);
  let source = cleaned;
  // 1) 隐藏类脚本(空替换串 + markdown_only)先剥除命中片段:<thinking>/剧情/信息/摘要等
  //    块不显示给用户,只保留开场剧情。仅处理 markdown_only=true 的脚本——markdown_only=false
  //    的「对AI隐藏状态栏」等仅注入提示词,显示层剥掉会把渲染脚本要用的
  //    <StatusPlaceHolderImpl/> 占位符一并移除,导致界面不渲染。
  for (const script of scripts) {
    if (!scriptAppliesAtDepth(script, depth)) continue;
    if (!script.enabled) continue;
    if (script.replace_string && script.replace_string.trim()) continue; // 只处理隐藏类
    if (!script.markdown_only) continue; // 显示层只应用 markdown_only 脚本
    const parsed = splitJsRegex(script.find_regex);
    if (!parsed || !parsed.pattern) continue;
    let re: RegExp;
    try {
      re = new RegExp(parsed.pattern, parsed.flags.includes('g') ? parsed.flags : parsed.flags + 'g');
    } catch {
      continue;
    }
    source = source.replace(re, '');
  }
  // 2) 渲染脚本(非空替换):渲染脚本优先,避免被包裹脚本抢占
  //   (渲染脚本把 <StatusPlaceHolderImpl/> 渲染成界面;包裹脚本只把它包回围栏)
  const ordered = [...scripts].sort(
    (a, b) => Number(isStatusRenderScript(b)) - Number(isStatusRenderScript(a)),
  );
  const placeholders: Array<{ ph: string; value: string }> = [];
  for (const script of ordered) {
    if (!scriptAppliesAtDepth(script, depth)) continue;
    if (!script.enabled) continue;
    if (!script.replace_string) continue;
    const parsed = splitJsRegex(script.find_regex);
    if (!parsed || !parsed.pattern) continue;
    let re: RegExp;
    try {
      re = new RegExp(parsed.pattern, parsed.flags.includes('g') ? parsed.flags : parsed.flags + 'g');
    } catch {
      continue;
    }
    source = source.replace(re, (...args: unknown[]) => {
      const ph = `KDAI_PH_${placeholders.length}`;
      let value = expandReplaceRefs(script.replace_string, args);
      if (macros) value = expandDisplayMacros(value, macros);
      placeholders.push({ ph, value });
      return ph;
    });
  }
  if (placeholders.length === 0) return null;
  // 只构建一次,保证 html 容器 scopeId 与 blocks 一致
  const built = placeholders.map((p, i) => ({
    index: i,
    scoped: buildScopedScriptHtml(p.value, seed ? `${seed}-${i}` : undefined),
  }));
  // 过滤空容器:替换值经清洗后为空(如包裹脚本把占位符包成围栏后,围栏内标签被
  // 白名单丢弃)时不再注入,避免消息尾部出现空 div;也不产生脚本块
  const valid = built.filter((b) => b.scoped.html.trim().length > 0);
  const blocks: Array<{ scopeId: string; scripts: string[] }> = valid.map((b) => ({
    scopeId: b.scoped.scopeId,
    scripts: b.scoped.scripts,
  }));
  for (const b of valid) {
    source = source.replace(placeholders[b.index].ph, `\u0000KDAI_BLOCK_${b.index}\u0000`);
  }
  // 正文段(非脚本)走 markdown 渲染,保留换行;脚本占位符按段拆分后独立注入 scoped 容器
  const segments = source.split(/(\u0000KDAI_BLOCK_\d+\u0000)/g);
  let html = '';
  for (const seg of segments) {
    const m = /^\u0000KDAI_BLOCK_(\d+)\u0000$/.exec(seg);
    if (m) {
      const item = built[Number(m[1])];
      if (item && item.scoped.html.trim().length > 0) html += item.scoped.html;
    } else if (seg) {
      html += renderMdSegment(seg);
    }
  }
  return { html, blocks };
}

/** 正文段渲染:markdown 渲染,保留换行段落;剥离残留脚本占位符 */
function renderMdSegment(seg: string): string {
  let s = seg;
  // 占位符若残留(替换异常)则静默丢弃,避免控制字符进入页面
  s = s.replace(/\u0000/g, '');
  if (!s.trim()) return '';
  return renderMarkdown(s);
}

/** 兼容旧调用:清洗脚本 HTML(不含 script,样式作用域化由 buildScopedScriptHtml 负责) */
export function cleanScriptHtml(replaceString: string): string {
  return buildScopedScriptHtml(replaceString).html;
}

/**
 * 角色卡脚本中是否存在把 <StatusPlaceHolderImpl/> 渲染为 HTML 状态栏的脚本
 * (启用、有替换串、正则应占位符)。存在时状态栏文本可借此渲染为 HTML 卡片。
 */
export function hasStatusPlaceholderScript(scripts: RegexScript[]): boolean {
  return scripts.some((s) => s.enabled && !!s.replace_string && /StatusPlaceHolderImpl/i.test(s.find_regex));
}

/**
 * 组装消息渲染文本:带状态栏且角色卡存在 HTML 状态栏脚本时,在正文后追加占位符,
 * 使状态栏被「状态栏」正则脚本识别渲染为 HTML 卡片(脚本用变量树填充动态数据);
 * 否则原样返回正文(状态栏走纯文本气泡 extra.status_bar)。
 * 正文已含 <StatusPlaceHolderImpl/>(如备用开场自带界面占位符)时不再追加,避免双卡片重复渲染。
 */
export function buildMessageRenderText(
  text: string,
  statusBarText: string | null | undefined,
  hasStatusScript: boolean,
): string {
  if (statusBarText && hasStatusScript && !/<StatusPlaceHolderImpl/i.test(text)) {
    return `${text}\n<StatusPlaceHolderImpl/>`;
  }
  return text;
}

/**
 * 应用「隐藏类」脚本(replace_string 为空的脚本,如「对 AI 隐藏状态栏」):
 * 把命中片段替换为空,用于 HTML 渲染关闭时剥掉 `<StatusPlaceHolderImpl/>` 等占位符,
 * 避免其裸露在 markdown 正文中。
 */
export function stripHiddenPlaceholders(text: string, scripts: RegexScript[], depth?: number): string {
  let out = text;
  for (const script of scripts) {
    if (!scriptAppliesAtDepth(script, depth)) continue;
    if (!script.enabled) continue;
    if (script.replace_string && script.replace_string.trim()) continue; // 只处理隐藏类
    const parsed = splitJsRegex(script.find_regex);
    if (!parsed || !parsed.pattern) continue;
    let re: RegExp;
    try {
      re = new RegExp(parsed.pattern, parsed.flags.includes('g') ? parsed.flags : parsed.flags + 'g');
    } catch {
      continue;
    }
    out = out.replace(re, '');
  }
  return out;
}

/**
 * 从消息文本提取「远程资源界面」加载目标。
 * 兼容酒馆角色卡常见开场模式 —— `$('body').load('https://…')` /
 * `$("body").load("https://…")`(作者把资源下载界面挂到开场消息):
 * 原版酒馆助手直接注入 body 执行,kedai 改为沙箱 iframe 隔离加载。
 * 仅接受 https URL(拒绝 http/javascript:/data: 等,防本地读取与注入)。
 * 返回 URL;未命中返回 null。
 */
export function extractBodyLoadUrl(text: string): string | null {
  if (!text) return null;
  // $('body').load('URL') / $("body").load("URL") / body.load(...) / #id.load(...):
  // 兼容 jQuery 选择器写法与引号变体,URL 一律经 isSafeResourceUrl(https) 把关
  const re = /\.load\s*\(\s*['"]([^'"]+)['"]\s*\)/i;
  const m = re.exec(text);
  if (!m) return null;
  const url = m[1].trim();
  return isSafeResourceUrl(url) ? url : null;
}

/** 资源界面 URL 白名单:仅 https,拒绝本地/数据/脚本协议 */
export function isSafeResourceUrl(url: string): boolean {
  try {
    const u = new URL(url);
    return u.protocol === 'https:' && !u.username && !u.password;
  } catch {
    return false;
  }
}

/**
 * 构建「远程资源界面」卡片 HTML(下载资源界面)。
 *
 * 加载流程(绕开作者服务器的 X-Frame-Options / CSP frame-ancestors 与父页面
 * CSP 对内联脚本的拦截):
 *   1) 父页面(带 API token)经后端 /api/resource/proxy 取作者页面 HTML
 *      (后端 https + SSRF 防护 + 禁重定向;服务端 fetch 不受 XFO 限制);
 *   2) iframe 用 src 加载独立宿主文档 /resource-frame.html(不继承父 CSP,
 *      允许作者页面脚本执行,见后端 resource_frame_headers);父页面把抓取的
 *      HTML 通过 postMessage 投递,宿主文档用 DOM 重建(appendChild)触发脚本
 *      执行——srcdoc/blob 都会继承父 CSP、document.write 会丢 module script,
 *      独立文档 + appendChild 是已验证可行的组合;
 *   3) iframe sandbox="allow-scripts" 无 allow-same-origin → opaque origin,
 *      作者页面脚本可运行(界面功能所需),但拿不到父页面 DOM/localStorage/token。
 * 本函数只生成卡片壳(标题栏 + iframe + 说明),投递由调用方 hydrate 完成。
 * seed 用于生成稳定容器 id(同一消息重渲染复用,避免 iframe 重复创建)。
 */
/**
 * 资源/面板 iframe nonce 稳定派生:同一 (url + seed) 每次渲染得到同一 nonce。
 * 早期用 Date.now+Math.random 生成,消息 computed 每次重算都会产出不同的
 * iframe src → v-html 变化 → iframe 被重建,资源页(吸血鬼卡 1.25MB)反复重新
 * 下载、下载进度全部丢失。稳定 nonce 使重渲染产出相同 HTML,v-html 不变则不
 * 触 DOM,iframe 得以存活;nonce 同时是 ready/boot 双向认证令牌。
 * (非安全随机:认证强度依赖父页面隔离,而非不可猜测性。)
 */
export function stableFrameNonce(input: string): string {
  // FNV-1a 64bit(拆两个 32bit 累加,避免 BigInt 依赖)
  let h1 = 0x811c9dc5;
  let h2 = 0x01000193;
  for (let i = 0; i < input.length; i++) {
    const c = input.charCodeAt(i);
    h1 = Math.imul(h1 ^ (c & 0xff), 0x01000193);
    h2 = Math.imul(h2 ^ (c >>> 8), 0x85ebca6b);
    h1 >>>= 0;
    h2 >>>= 0;
  }
  return `kd${h1.toString(36)}${h2.toString(36)}`;
}

export function buildRemoteResourceHtml(url: string, seed?: string): string {
  const id = seed ? `sv-res-${seed}` : `sv-res-${Date.now().toString(36)}${(scopeSeq++).toString(36)}`;
  const escUrl = escapeHtml(url);
  const urlAttr = encodeURIComponent(url);
  // nonce 按 (url+seed) 稳定派生(见 stableFrameNonce);放 URL fragment(不随 HTTP 请求发送)
  const nonce = stableFrameNonce(`${url}${seed ?? ''}`);
  return (
    `<div class="sv-resource-card" id="${escapeHtml(id)}" data-kd-resource="1" data-kd-resource-url="${escapeHtml(urlAttr)}" data-kd-resource-nonce="${escapeHtml(nonce)}">` +
    `<div class="sv-resource-card-head">` +
    `<span class="sv-resource-card-title">角色卡资源界面</span>` +
    `<span class="sv-resource-card-side">` +
    // 宽屏切换:作者页(如吸血鬼卡)的游戏界面为全屏设计,沙箱 iframe 无法把
    // 撑满样式注入宿主文档(见 resource_frame_template 的 parent shim),由宿主
    // 侧把整卡 fixed 撑满视口来补足;点击委托在 ChatWindow(scrollArea 全局委托)。
    `<button type="button" class="sv-resource-card-wide" data-kd-resource-wide="1">宽屏</button>` +
    `<a class="sv-resource-card-open" href="${escUrl}" target="_blank" rel="noopener noreferrer">在新窗口打开</a>` +
    `</span>` +
    `</div>` +
    `<iframe class="sv-resource-card-frame" data-kd-resource-frame="1" sandbox="allow-scripts allow-popups allow-forms allow-modals" ` +
    `referrerpolicy="no-referrer" src="/resource-frame.html#${escapeHtml(nonce)}" title="角色卡资源界面"></iframe>` +
    `<div class="sv-resource-card-note">资源页面由角色卡作者提供,已隔离加载;若未显示,请使用上方链接在新窗口打开。</div>` +
    `</div>`
  );
}
