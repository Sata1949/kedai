// dom-rpc.ts — DOM RPC 宿主端:选择器解析/jQuery 操作应用/状态镜像采集/游离元素落 DOM,及数据 op 分发(全局变量与 localStorage 持久化桥)
import sanitizeHtml from 'sanitize-html';
import { SCRIPT_HTML_WHITELIST, sanitizeScriptHtmlWithStyles } from './sanitize';
import {
  CHANNEL,
  type CreatedElementSpec,
  type JqOperation,
  type SandboxExecutionContext,
} from './protocol';

function safeSelector(value: unknown): string {
  if (typeof value !== 'string' || value.length > 1024 || /[\0\r\n]/.test(value)) {
    throw new Error('选择器无效');
  }
  return value;
}

function safeText(value: unknown): string {
  // 状态栏脚本单次 html() 可达百余 KB(wuwa 状态栏 161KB),20KB 上限会整段拒掉 → 界面空白
  if (typeof value !== 'string' || value.length > 512_000) throw new Error('文本无效');
  return value;
}

/**
 * 作者 HTML 进宿主 DOM 的清洗入口(html()/append()/appendCreated 共用):
 * 容器带 data-kd-scope(渲染块自带,卡级宿主由 cardScriptHost 设置)时 <style> 段
 * 经声明级清洗 + 容器作用域化后保留;无 scope 时退回纯白名单(不作用域化)。
 */
function sanitizeForContainer(container: HTMLElement, html: string): string {
  const scopeId =
    typeof container.getAttribute === 'function' ? container.getAttribute('data-kd-scope') : null;
  if (scopeId) return sanitizeScriptHtmlWithStyles(html, scopeId);
  return sanitizeHtml(html, SCRIPT_HTML_WHITELIST);
}

/** 按角色持久化的卡内全局变量(wuwa 状态栏 loadSettings/saveSettings 读写酒馆全局变量的落点) */
const cardGlobalsKey = (characterId: string): string => `kedai.card-globals.${characterId}`;

export function readCardGlobals(characterId: string | undefined): Record<string, unknown> {
  if (!characterId) return {};
  try {
    const raw = localStorage.getItem(cardGlobalsKey(characterId));
    const v: unknown = raw ? JSON.parse(raw) : null;
    return v && typeof v === 'object' && !Array.isArray(v) ? (v as Record<string, unknown>) : {};
  } catch {
    return {};
  }
}

function writeCardGlobals(characterId: string | undefined, globals: unknown): void {
  if (!characterId || !globals || typeof globals !== 'object' || Array.isArray(globals)) return;
  try {
    localStorage.setItem(cardGlobalsKey(characterId), JSON.stringify(globals));
  } catch {
    /* 配额异常忽略:设置持久化失败不阻断脚本 */
  }
}

/**
 * 沙箱 localStorage 的宿主持久化(对齐酒馆:卡脚本 localStorage 落在源级存储,重载后仍在,
 * 如 wuwa 协议 ww_agreement_accepted_v1 只需同意一次)。全角色共用一份,语义与 ST 同源一致。
 */
const SCRIPT_LOCAL_STORAGE_KEY = 'kedai.sandbox-localstorage.v1';

export function readScriptLocalStorage(): Record<string, string> {
  try {
    const raw = localStorage.getItem(SCRIPT_LOCAL_STORAGE_KEY);
    const v: unknown = raw ? JSON.parse(raw) : null;
    if (!v || typeof v !== 'object' || Array.isArray(v)) return {};
    const out: Record<string, string> = {};
    for (const [k, val] of Object.entries(v as Record<string, unknown>)) out[k] = String(val ?? '');
    return out;
  } catch {
    return {};
  }
}

export function applyScriptLocalStorageOp(op: string, key: unknown, value: unknown): void {
  const store = readScriptLocalStorage();
  if (op === 'set' && typeof key === 'string') store[key] = String(value ?? '');
  else if (op === 'remove' && typeof key === 'string') delete store[key];
  else if (op === 'clear') for (const k of Object.keys(store)) delete store[k];
  else return;
  try {
    localStorage.setItem(SCRIPT_LOCAL_STORAGE_KEY, JSON.stringify(store));
  } catch {
    /* 配额异常忽略 */
  }
}

function queryScoped(container: HTMLElement, selector: string): HTMLElement[] {
  const safe = safeSelector(selector);
  // 作者脚本把容器当文档用:$('body')/html 指代整个状态栏容器(沙箱无真实 body 可管)
  if (safe === 'body' || safe === 'html' || safe === 'html body') return [container];
  const positional = /:(first|last)\s*$/.exec(safe);
  const base = positional ? safe.slice(0, positional.index).trim() : safe;
  if (!base) throw new Error('选择器无效');
  const matches = Array.from(container.querySelectorAll<HTMLElement>(base));
  if (positional?.[1] === 'first') return matches.slice(0, 1);
  if (positional?.[1] === 'last') return matches.slice(-1);
  return matches;
}

/** selector-index 引用(first/last/eq/数字索引产生):按序号取匹配元素 */
function resolveRefElements(container: HTMLElement, ref: JqOperation['ref'], targets: Map<number, HTMLElement>): HTMLElement[] {
  if (ref.kind === 'selector') return queryScoped(container, ref.value);
  if (ref.kind === 'target') return targets.get(ref.id) ? [targets.get(ref.id)!] : [];
  if (ref.kind === 'selector-index') {
    const all = queryScoped(container, ref.value);
    const idx = ref.index < 0 ? all.length + ref.index : ref.index;
    const el = idx >= 0 && idx < all.length ? all[idx] : undefined;
    return el ? [el] : [];
  }
  return [];
}

/** 单元素状态(getter 镜像条目;text/html 截断防巨型回包) */
export function elementState(el: HTMLElement): Record<string, unknown> {
  const inp = el as HTMLInputElement;
  return {
    n: 1,
    // id/name 供沙箱侧事件 this 门面的 getAttribute 与选择器回查
    id: el.id || undefined,
    name: inp.name || undefined,
    text: (el.textContent ?? '').slice(0, 100_000),
    html: el.innerHTML.slice(0, 100_000),
    val: typeof inp.value === 'string' ? inp.value.slice(0, 20_000) : '',
    checked: !!inp.checked,
    disabled: !!inp.disabled,
    classes: Array.from(el.classList).slice(0, 64),
    // 滚动/几何度量:协议卡 checkBottom($card[0].scrollTop+clientHeight>=scrollHeight-8)
    // 与定位脚本 outerHeight 等读这些真值,缺了则 NaN 比较恒 false(勾选框永远 locked)
    scrollTop: el.scrollTop,
    scrollLeft: el.scrollLeft,
    scrollHeight: el.scrollHeight,
    scrollWidth: el.scrollWidth,
    clientHeight: el.clientHeight,
    clientWidth: el.clientWidth,
    offsetHeight: el.offsetHeight,
    offsetWidth: el.offsetWidth,
  };
}

/** 集合元素快照(jQuery .each/数字索引 getter 真实化;上限 50 条、字段截断防巨型回包) */
function gatherItems(els: HTMLElement[]): Array<Record<string, unknown>> {
  return els.slice(0, 50).map((el) => {
    const inp = el as HTMLInputElement;
    return {
      text: (el.textContent ?? '').slice(0, 500),
      html: el.innerHTML.slice(0, 2000),
      val: typeof inp.value === 'string' ? inp.value.slice(0, 500) : '',
      checked: !!inp.checked,
      disabled: !!inp.disabled,
      classes: Array.from(el.classList).slice(0, 32),
      id: el.id || undefined,
      name: inp.name || undefined,
      tag: el.tagName.toLowerCase(),
    };
  });
}

/** 选择器状态(jQuery getter 语义:集合大小 + 首元素状态 + items;无效选择器不抛出) */
function gatherSelectorState(container: HTMLElement, selector: string): Record<string, unknown> {
  try {
    const els = queryScoped(container, selector);
    const first = els[0];
    if (!first) return { n: 0 };
    return { ...elementState(first), n: els.length, items: gatherItems(els) };
  } catch {
    return { n: 0 };
  }
}

/** 容器内全部表单控件状态(协议勾选/输入读取的目标;上限 200 防巨型容器) */
export function gatherControls(container: HTMLElement): Array<Record<string, unknown>> {
  const out: Array<Record<string, unknown>> = [];
  const els = Array.from(container.querySelectorAll<HTMLElement>('input, select, textarea, button')).slice(0, 200);
  for (const el of els) {
    const inp = el as HTMLInputElement;
    out.push({
      id: el.id || undefined,
      name: inp.name || undefined,
      tag: el.tagName.toLowerCase(),
      val: typeof inp.value === 'string' ? inp.value.slice(0, 2000) : '',
      checked: !!inp.checked,
      disabled: !!inp.disabled,
    });
  }
  return out;
}

/** 容器内带 id 元素的轻量类状态表(沙箱 attr('id') 复合类选择器回查的索引底;
 *  boot 注入 + 每批 ack 顺带,键为 '#id',值仅 id/classes 两字段,上限 300) */
export function gatherIdMap(container: HTMLElement, into: Record<string, unknown> = {}): Record<string, unknown> {
  try {
    if (typeof container.querySelectorAll !== 'function') return into;
    const els = container.querySelectorAll<HTMLElement>('[id]');
    const cap = Math.min(els.length, 300);
    for (let i = 0; i < cap; i++) {
      const el = els[i];
      if (!el.id) continue;
      const key = `#${el.id}`;
      if (!(key in into)) into[key] = { id: el.id, classes: Array.from(el.classList).slice(0, 32), n: 1 };
    }
  } catch {
    /* 容器异常:返回已收集部分 */
  }
  return into;
}

/** 批处理回包镜像:批内涉及的选择器状态(操作已应用后的最新值)+ 全量表单控件 */
export function buildStateMirror(
  container: HTMLElement,
  ops: JqOperation[],
): { selectors: Record<string, unknown>; controls: Array<Record<string, unknown>> } {
  const selectors: Record<string, unknown> = {};
  for (const op of ops) {
    if (op?.ref?.kind === 'selector') {
      const key = String(op.ref.value ?? '');
      if (key && !(key in selectors)) selectors[key] = gatherSelectorState(container, key);
    } else if (op?.ref?.kind === 'selector-index') {
      const ref = op.ref;
      const key = `${String(ref.value ?? '')}@@${ref.index}`;
      if (!(key in selectors)) {
        try {
          const all = queryScoped(container, ref.value);
          const idx = ref.index < 0 ? all.length + ref.index : ref.index;
          const el = idx >= 0 && idx < all.length ? all[idx] : undefined;
          selectors[key] = el ? { ...elementState(el), n: all.length } : { n: all.length };
        } catch {
          selectors[key] = { n: 0 };
        }
      }
    }
  }
  // 顺带并入 id 轻量表:沙箱 attr('id') 复合类选择器回查的索引底(结构变化后保持新鲜)
  gatherIdMap(container, selectors);
  return { selectors, controls: gatherControls(container) };
}

/** 游离元素落 DOM(appendCreated):清洗 HTML、绑定沙箱侧延迟注册的事件、递归子元素 */
function buildCreatedElement(
  spec: CreatedElementSpec,
  targetWindow: Window | null,
  nonce: string,
  targets: Map<number, HTMLElement>,
  nextTargetId: () => number,
  registerEventListener: (element: HTMLElement, eventName: string, listener: EventListener) => void,
  container: HTMLElement,
): HTMLElement {
  const tmp = document.createElement('div');
  tmp.innerHTML = sanitizeForContainer(container, String(spec.html ?? ''));
  const el = (tmp.firstElementChild as HTMLElement | null) ?? document.createElement('div');
  if (spec.addClass) el.classList.add(...String(spec.addClass).split(/\s+/).filter(Boolean));
  if (typeof spec.text === 'string') el.textContent = spec.text;
  else if (typeof spec.innerHtml === 'string') el.innerHTML = sanitizeForContainer(container, spec.innerHtml);
  for (const h of Array.isArray(spec.handlers) ? spec.handlers : []) {
    if (!targetWindow || !h || !Number.isFinite(h.jqId)) continue;
    const jqId = Number(h.jqId);
    // 与 applyJq 'on' 一致:空格分隔的多事件名拆开逐个绑定
    const eventNames = String(h.evt ?? '').split(/\s+/).filter(Boolean);
    if (eventNames.length === 0) continue;
    const targetId = nextTargetId();
    targets.set(targetId, el);
    for (const eventName of eventNames) {
      const listener = (): void => {
        const data = Object.fromEntries(Object.entries(el.dataset));
        targetWindow.postMessage(
          {
            channel: CHANNEL,
            nonce,
            type: 'jq-event',
            jqId,
            event: { type: eventName },
            target: { kind: 'target', id: targetId, data, state: elementState(el) },
            mirror: { controls: gatherControls(container) },
          },
          '*',
        );
      };
      el.addEventListener(eventName, listener);
      registerEventListener(el, eventName, listener);
    }
  }
  for (const child of Array.isArray(spec.children) ? spec.children : []) {
    if (child && child.kind === 'created') {
      el.appendChild(buildCreatedElement(child, targetWindow, nonce, targets, nextTargetId, registerEventListener, container));
    }
  }
  return el;
}

/** jQuery 风格 DOM 操作(经 RPC 转发到宿主容器;事件 target 仅使用本次执行内的受控句柄) */
export function applyJq(
  container: HTMLElement,
  ref: JqOperation['ref'],
  method: string,
  args: unknown[],
  targetWindow: Window | null,
  nonce: string,
  targets: Map<number, HTMLElement>,
  nextTargetId: () => number,
  registerEventListener: (element: HTMLElement, eventName: string, listener: EventListener) => void,
): unknown {
  const els = resolveRefElements(container, ref, targets);
  const first = els[0];
  switch (method) {
    case 'count':
      return els.length;
    case 'probe':
      // 镜像探测:不改动 DOM;该选择器的状态随本批回包镜像(buildStateMirror)带回沙箱
      return true;
    case 'text':
      if (args.length === 0) return first?.textContent ?? '';
      for (const el of els) el.textContent = safeText(args[0]);
      return true;
    case 'html': {
      if (args.length === 0) return first?.innerHTML ?? '';
      const html = safeText(args[0]);
      for (const el of els) {
        el.innerHTML = sanitizeForContainer(container, html);
      }
      return true;
    }
    case 'css':
      if (args.length === 1) return first ? first.style.getPropertyValue(String(args[0])) : '';
      for (const el of els) el.style.setProperty(String(args[0]), String(args[1] ?? ''));
      return true;
    case 'addClass':
      for (const el of els) el.classList.add(...String(args[0] ?? '').split(/\s+/).filter(Boolean));
      return true;
    case 'removeClass':
      for (const el of els) el.classList.remove(...String(args[0] ?? '').split(/\s+/).filter(Boolean));
      return true;
    case 'toggleClass': {
      const force = args.length > 1 && typeof args[1] === 'boolean' ? (args[1] as boolean) : undefined;
      for (const el of els) {
        for (const cls of String(args[0] ?? '').split(/\s+/).filter(Boolean)) {
          if (force === undefined) el.classList.toggle(cls);
          else el.classList.toggle(cls, force);
        }
      }
      return true;
    }
    case 'hasClass':
      return first ? first.classList.contains(String(args[0] ?? '')) : false;
    case 'val':
      if (args.length === 0) return (first as HTMLInputElement | undefined)?.value ?? '';
      for (const el of els) (el as HTMLInputElement).value = String(args[0] ?? '');
      return true;
    case 'prop': {
      // jQuery prop:checkbox/select 的 checked/disabled 等布尔属性
      const name = String(args[0] ?? '');
      const value = args[1];
      if (name === 'checked') {
        for (const el of els) (el as HTMLInputElement).checked = Boolean(value);
      } else if (name === 'disabled') {
        for (const el of els) (el as HTMLInputElement).disabled = Boolean(value);
      } else if (name === 'value') {
        for (const el of els) (el as HTMLInputElement).value = String(value ?? '');
      } else if (value === undefined) {
        return first ? (first as HTMLInputElement)[name as keyof HTMLInputElement] : undefined;
      } else {
        // name 为脚本传入的动态属性名:keyof HTMLInputElement 的键集含 DOM lib 只读属性,直接索引赋值触发 TS2540;
        // 断言为可变视图仅是类型层面放行(对齐项目内 as unknown as Record 惯例),运行时仍是 el[name] = value
        for (const el of els) (el as unknown as Record<string, unknown>)[name] = value;
      }
      return true;
    }
    case 'attr':
      if (args.length === 1) return first ? first.getAttribute(String(args[0])) ?? '' : '';
      for (const el of els) el.setAttribute(String(args[0]), String(args[1] ?? ''));
      return true;
    case 'trigger': {
      // 触发事件(change 等);宿主已注册的监听器(经 on RPC)会收到 dispatchEvent
      const eventName = String(args[0] ?? '');
      for (const el of els) el.dispatchEvent(new Event(eventName, { bubbles: true }));
      return true;
    }
    case 'append': {
      // 追加 HTML(select 填充 option、容器追加节点等);与 innerHTML 一样经 sanitize 清洗
      const html = safeText(args[0]);
      for (const el of els) {
        el.insertAdjacentHTML('beforeend', sanitizeForContainer(container, html));
      }
      return true;
    }
    case 'appendCreated': {
      // $('<div>') 游离元素落 DOM(wuwa 行动页 opt-list 构建路径)
      const spec = args[0] as CreatedElementSpec;
      if (!spec || spec.kind !== 'created') return true;
      for (const el of els) {
        el.appendChild(buildCreatedElement(spec, targetWindow, nonce, targets, nextTargetId, registerEventListener, container));
      }
      return true;
    }
    case 'focus':
      for (const el of els) (el as HTMLElement).focus();
      return true;
    case 'remove':
      for (const el of els) el.remove();
      return true;
    case 'empty':
      for (const el of els) el.innerHTML = '';
      return true;
    case 'off': {
      // 解绑事件:宿主监听器统一由 executeSandboxedCharacterScript 的 cleanup 在容器
      // 卸载/切换时释放;此处仅确认操作合法,避免脚本重复绑定累积(每次执行容器独立)。
      return true;
    }
    case 'on': {
      // jQuery 语义:'scroll wheel' 这类空格分隔的多事件名要拆开逐个绑定
      // (此前整串当单个事件名 addEventListener,永不触发,协议卡滚动解锁因此失效)
      const eventNames = String(args[0] ?? '').split(/\s+/).filter(Boolean);
      const jqId = Number(args[1]);
      if (!targetWindow || eventNames.length === 0 || !Number.isFinite(jqId)) return true;
      for (const el of els) {
        const targetId = nextTargetId();
        targets.set(targetId, el);
        for (const eventName of eventNames) {
          const listener = (): void => {
            const data = Object.fromEntries(Object.entries(el.dataset));
            targetWindow.postMessage(
              {
                channel: CHANNEL,
                nonce,
                type: 'jq-event',
                jqId,
                event: { type: eventName },
                // 事件时刻的目标状态(getter 真实化:勾选/取值/滚动度量读到事件当时的真值)
                target: { kind: 'target', id: targetId, data, state: elementState(el) },
                // 全量表单控件状态:协议脚本在回调里读其它控件($('#agree') 等)
                mirror: { controls: gatherControls(container) },
              },
              '*',
            );
          };
          el.addEventListener(eventName, listener);
          registerEventListener(el, eventName, listener);
        }
      }
      return true;
    }
    default:
      throw new Error(`不兼容的角色卡脚本操作: ${method}`);
  }
}

export function applyRpc(
  context: SandboxExecutionContext,
  op: string,
  args: unknown[],
): unknown {
  // 全局变量保存(状态栏 saveSettings):无选择器参数,必须先于 selector 提取分支
  if (op === 'global-save') {
    writeCardGlobals(context.characterId, args[0]);
    return true;
  }
  const selector = safeSelector(args[0]);
  const target = context.container.querySelector<HTMLElement>(selector);
  if (!target) throw new Error(`未找到状态栏元素: ${selector}`);
  if (op === 'setText') {
    target.textContent = safeText(args[1]);
    return true;
  }
  if (op === 'setHtml') {
    const html = safeText(args[1]);
    target.innerHTML = sanitizeHtml(html, {
      allowedTags: ['span', 'b', 'strong', 'i', 'em', 'small', 'br'],
      allowedAttributes: { '*': ['class', 'aria-label'] },
      allowedSchemes: [],
    });
    return true;
  }
  if (op === 'setAttribute') {
    const name = String(args[1] ?? '');
    if (!['class', 'title', 'aria-label', 'data-value'].includes(name)) throw new Error('属性不在白名单');
    target.setAttribute(name, safeText(args[2]));
    return true;
  }
  throw new Error(`不兼容的角色卡脚本操作: ${op}`);
}
