// sanitize.ts — 脚本注入 HTML 的白名单清洗配置(setter/append/游离元素落 DOM 共用)
import sanitizeHtml from 'sanitize-html';

/** 脚本注入 HTML 的白名单(setter/append 共用):含表格/图片/表单控件(角色卡界面结构所需);
 *  img/a 仅 https 与 data 协议,事件处理器仍剥离(脚本交互走 on() 绑定) */
const SCRIPT_HTML_WHITELIST: sanitizeHtml.IOptions = {
  allowedTags: [
    'span', 'b', 'strong', 'i', 'em', 'small', 'br', 'div', 'p',
    'h1', 'h2', 'h3', 'h4', 'h5', 'h6', 'ul', 'ol', 'li', 'dl', 'dt', 'dd',
    'table', 'thead', 'tbody', 'tfoot', 'tr', 'th', 'td',
    'img', 'a', 'button', 'label', 'select', 'option', 'optgroup', 'input', 'textarea',
    'details', 'summary', 'blockquote', 'code', 'pre', 'hr',
  ],
  allowedAttributes: {
    '*': ['class', 'aria-label', 'style', 'id', 'data-*', 'title', 'role'],
    th: ['colspan', 'rowspan', 'scope'],
    td: ['colspan', 'rowspan'],
    img: ['src', 'alt', 'width', 'height'],
    a: ['href', 'target', 'rel'],
    input: ['type', 'name', 'value', 'placeholder', 'checked', 'disabled', 'min', 'max', 'maxlength', 'size'],
    select: ['name', 'multiple', 'disabled', 'size'],
    option: ['value', 'selected', 'disabled'],
    optgroup: ['label', 'disabled'],
    textarea: ['name', 'rows', 'cols', 'placeholder', 'disabled', 'maxlength'],
    label: ['for'],
  },
  allowedSchemes: ['https', 'data'],
  allowedSchemesByTag: { img: ['https', 'data'] },
  allowProtocolRelative: false,
};

export { SCRIPT_HTML_WHITELIST };
