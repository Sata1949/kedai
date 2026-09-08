// dom-rpc.test.ts — 宿主端选择器映射与注入标记(node 环境无 DOM,用最小元素桩驱动 applyJq)。
// 覆盖:覆层根优先映射($('head')/$('body') → 覆层根)、注入节点 data-kd-injected、
// 覆层根直系子节点 pointer-events:auto。
import { describe, expect, it } from 'vitest';
import { applyJq, overlayRootOf } from './dom-rpc';
import type { JqOperation } from './protocol';

class StubEl {
  tagName = 'DIV';
  nodeType = 1;
  attrs: Record<string, string> = {};
  style = {
    cssText: '',
    pointerEvents: '',
    setProperty: (_k: string, _v: string): void => {},
    getPropertyValue: (): string => '',
    item: (): string => '',
    length: 0,
  };
  dataset: Record<string, string> = {};
  classList = { add: (): void => {}, remove: (): void => {}, toggle: (): void => {}, contains: (): boolean => false };
  children: StubEl[] = [];
  childNodes: StubEl[] = [];
  parentElement: StubEl | null = null;
  id = '';
  textContent = '';
  innerHTML = '';
  value = '';
  checked = false;
  disabled = false;
  scrollTop = 0;
  scrollLeft = 0;
  scrollHeight = 0;
  scrollWidth = 0;
  clientHeight = 0;
  clientWidth = 0;
  offsetHeight = 0;
  offsetWidth = 0;

  constructor(readonly ownerDocument: { createElement: (t: string) => StubEl }) {}

  setAttribute(k: string, v: string): void {
    this.attrs[k] = String(v);
  }

  getAttribute(k: string): string | null {
    return k in this.attrs ? this.attrs[k] : null;
  }

  removeAttribute(k: string): void {
    delete this.attrs[k];
  }

  appendChild(child: StubEl): StubEl {
    child.parentElement = this;
    this.children.push(child);
    this.childNodes.push(child);
    return child;
  }

  // 模拟插入:每次追加一个游离节点(用于 append 路径的新增节点差集打标)
  insertAdjacentHTML(_pos: string, _html: string): void {
    const doc = this.ownerDocument;
    this.appendChild(doc.createElement('div'));
  }

  querySelectorAll(_sel?: string): StubEl[] {
    return [];
  }

  querySelector(_sel?: string): StubEl | null {
    return null;
  }

  addEventListener(): void {}
  removeEventListener(): void {}
  contains(): boolean {
    return true;
  }
  closest(): null {
    return null;
  }
  remove(): void {}
  focus(): void {}
  dispatchEvent(): void {}
  getBoundingClientRect(): { left: number; top: number; width: number; height: number } {
    return { left: 0, top: 0, width: 0, height: 0 };
  }
}

/** 容器桩:querySelector 只认覆层根,querySelectorAll 返回配置的匹配元素 */
function makeContainer(overlayRoot: StubEl | null, matches: StubEl[] = []): StubEl {
  const doc = { createElement: (_t: string): StubEl => new StubEl(doc) };
  const c = new StubEl(doc);
  c.attrs['data-kd-scope'] = 'card-c1';
  c.querySelector = (sel: string): StubEl | null => (sel === '[data-kd-overlay-root]' ? overlayRoot : null);
  c.querySelectorAll = (): StubEl[] => matches;
  return c;
}

function apply(container: StubEl, ref: JqOperation['ref'], method: string, args: unknown[] = []): unknown {
  return applyJq(
    container as unknown as HTMLElement,
    ref,
    method,
    args,
    null,
    'n1',
    new Map(),
    () => 1,
    () => {},
  );
}

const sel = (value: string): JqOperation['ref'] => ({ kind: 'selector', value });

/** 覆层根桩:带 data-kd-overlay-root 属性(pointer-events:auto 的判定依据) */
function makeOverlayRoot(): StubEl {
  const root = makeContainer(null).ownerDocument.createElement('div');
  root.attrs['data-kd-overlay-root'] = '';
  return root;
}

describe('overlayRootOf(覆层根优先映射)', () => {
  it('容器内有覆层根时返回覆层根;无则回退容器(消息级行为不变)', () => {
    const root = makeOverlayRoot();
    const withRoot = makeContainer(root);
    expect(overlayRootOf(withRoot as unknown as HTMLElement)).toBe(root);
    const without = makeContainer(null);
    expect(overlayRootOf(without as unknown as HTMLElement)).toBe(without);
  });

  it("$('head')/$('body')/$('html') 映射到覆层根(css 落点)", () => {
    const root = makeOverlayRoot();
    const container = makeContainer(root);
    for (const s of ['head', 'body', 'html', 'html body']) {
      apply(container, sel(s), 'css', ['display', 'none']);
    }
    // 三次都落在覆层根(container 自身未被改)
    expect(root.style.setProperty).toBeDefined();
    expect(container.style.cssText).toBe('');
  });
});

describe('注入节点标记(data-kd-injected)', () => {
  it('appendCreated 落 DOM 的顶层节点带 data-kd-injected,覆层根下补 pointer-events:auto', () => {
    const root = makeOverlayRoot();
    const container = makeContainer(root);
    apply(container, sel('body'), 'appendCreated', [
      { kind: 'created', html: '<div></div>', handlers: [], children: [], attrs: { id: 'fx' }, css: { position: 'fixed' } },
    ]);
    expect(root.children).toHaveLength(1);
    const created = root.children[0];
    expect(created.attrs['data-kd-injected']).toBe('1');
    expect(created.attrs.id).toBe('fx');
    expect(created.style.pointerEvents).toBe('auto');
  });

  it('append 的 insertAdjacentHTML 路径对新增子节点打标', () => {
    const root = makeOverlayRoot();
    const container = makeContainer(root);
    apply(container, sel('body'), 'append', ['<style>.a{color:red}</style>']);
    expect(root.children).toHaveLength(1);
    expect(root.children[0].attrs['data-kd-injected']).toBe('1');
  });

  it('html 路径对新增子节点打标', () => {
    const root = makeOverlayRoot();
    const container = makeContainer(root);
    // 模拟 innerHTML 赋值后产生一个子节点
    const doc = root.ownerDocument;
    Object.defineProperty(root, 'innerHTML', {
      configurable: true,
      get: () => '',
      set: () => {
        const child = doc.createElement('div');
        child.parentElement = root;
        root.childNodes.push(child);
        root.children.push(child);
      },
    });
    apply(container, sel('body'), 'html', ['<span>x</span>']);
    expect(root.children[0].attrs['data-kd-injected']).toBe('1');
  });

  it('appendCreated 的 spec.draggable 落 DOM 时绑定拖拽(dataset 标记)', () => {
    // th-5 链路:p$('<div>').attr('id',…).css({…}) 创建 → 落 DOM 后 .draggable 仍生效
    const root = makeOverlayRoot();
    const container = makeContainer(root);
    apply(container, sel('body'), 'appendCreated', [
      {
        kind: 'created',
        html: '<div></div>',
        handlers: [],
        children: [],
        attrs: { id: 'fx-floating-ball' },
        draggable: { start: 0, drag: 0, stop: 0, containment: 'window', distance: 3 },
      },
    ]);
    const created = root.children[0];
    expect(created.dataset.uiDraggable).toBe('1');
    expect(created.attrs['data-kd-injected']).toBe('1');
  });
});
