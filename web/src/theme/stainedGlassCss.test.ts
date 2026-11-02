// GLASS-PANE-1 彩窗主题拼片契约（2026-11-02）。jsdom 无布局引擎、vitest `css:false`
// 不加载样式，拼片这类「观感 + 可读性」的边界无法用组件测试断言；沿用 ANDUI 批
// （styles/mobileLayout.test.ts）同一口径，用源码文本守住几条「一改就悄悄回退」的规则。
// 注意 `.css` 的 `?raw` 在本仓返回空串，故一律走 node:fs。
import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';

const css = readFileSync(new URL('./stained-glass.css', import.meta.url), 'utf8');

/** 去掉块注释：注释里会提到选择器名与 var(--kg-panes)，不剥会污染所有断言。 */
const code = css.replace(/\/\*[\s\S]*?\*\//g, '');

/** 取出所有 `选择器 { 声明 }` 块，并记录它所在的 @media 条件（没有则为空串）。
 *  必须记录媒体上下文：`.sv-sidebar { position }` 只有在桌面断点里才安全，
 *  只看选择器文本会把正确写法误判成违规。 */
const blocks = (source: string): Array<{ selector: string; body: string; media: string }> => {
  const out: Array<{ selector: string; body: string; media: string }> = [];
  // 逐个 @media 段处理：先按 `@media ... {` 切开，记录每个位置所属的条件
  const mediaRe = /@media([^{]+)\{/g;
  const ranges: Array<{ start: number; end: number; cond: string }> = [];
  let mm: RegExpExecArray | null;
  while ((mm = mediaRe.exec(source)) !== null) {
    // 从 `{` 之后到与之配对的 `}` —— 主题 CSS 里 @media 只有一层嵌套，数花括号即可
    let depth = 1;
    let i = mm.index + mm[0].length;
    while (i < source.length && depth > 0) {
      if (source[i] === '{') depth += 1;
      else if (source[i] === '}') depth -= 1;
      i += 1;
    }
    ranges.push({ start: mm.index + mm[0].length, end: i - 1, cond: `@media${mm[1]}`.trim() });
  }
  const mediaAt = (pos: number): string => {
    const hit = ranges.find((r) => pos >= r.start && pos < r.end);
    return hit ? hit.cond : '';
  };

  const re = /([^{}]+)\{([^{}]*)\}/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(source)) !== null) {
    out.push({ selector: m[1].trim(), body: m[2], media: mediaAt(m.index) });
  }
  return out;
};

const allBlocks = blocks(code);

/** 所有「用了拼片」的块的选择器。拼片令牌是 `--kg-panes-band`
 *  （560×80 的横向窗楣条带，真实彩窗的「石棂 + 菱形小片」构造）。
 *  内容层禁用规则对它一体适用。 */
const panesSelectors = allBlocks
  .filter((b) => b.body.includes('var(--kg-panes-band)'))
  .map((b) => b.selector.replace(/\s+/g, ' '));

describe('GLASS-PANE-1 拼接玻璃契约', () => {
  it('1. 拼片令牌存在且是内联 data URI（不引外部资源，避免多一次请求）', () => {
    expect(code).toMatch(/--kg-panes-band:\s*url\("data:image\/svg\+xml,/);
  });

  it('2. 拼片只允许出现在「装饰层」与「强调层」，选择器在白名单内', () => {
    // 白名单只保留「确实承担结构语义」的三处：顶栏窗楣（装饰层）+ 两处强调层。
    // 空态装饰条已被用户裁定撤掉（口径：装饰条太丑）——拼片只做真实结构，不做填充物。
    // 新增用点必须先想清楚它属于哪一层，再回来扩这张表；不要为了让测试过而放宽。
    const ALLOWED = [
      'html[data-theme=\'stained-glass\'] .sv-topbar', // 装饰层：横向窗楣
      'html[data-theme=\'stained-glass\'] .sv-char-item.active', // 强调层：角色卡选中态
      'html[data-theme=\'stained-glass\'] .sv-btn.primary:not(:disabled),\nhtml[data-theme=\'stained-glass\'] .sv-btn-send:not(:disabled)', // 强调层：主按钮
    ];
    for (const sel of panesSelectors) {
      const ok = ALLOWED.some((allowed) => sel === allowed || sel === allowed.replace(/\n/g, ' '));
      expect(ok, `拼片用在了未登记的选择器上：${sel}`).toBe(true);
    }
    // 同时确认白名单确实都在用（防止白名单腐烂成空话）
    expect(panesSelectors.length, '拼片用点数量异常').toBe(ALLOWED.length);
  });

  it('3. 内容层严禁铺拼片（拼片当背景必然压字，这是七轮原型验出的硬约束）', () => {
    // 这些容器里都有正文/表单，拼片的铅条会横穿文字。
    const CONTENT_LAYER = [
      '.sv-main',
      '.sv-sidebar',
      '.sv-msg-bubble',
      '.sv-inputbox',
      '.sv-modal',
      '.sv-hub-content',
      '.sv-start-copy',
      '.sv-task-detail',
    ];
    for (const sel of panesSelectors) {
      for (const guard of CONTENT_LAYER) {
        expect(sel.includes(guard), `内容层 ${guard} 被铺了拼片：${sel}`).toBe(false);
      }
    }
  });

  it('4. 主题不使用 !important（除非将来显式登记）', () => {
    expect(code).not.toContain('!important');
  });

  it('5. .sv-sidebar 的 position 只出现在桌面媒体查询内（否则打穿移动端抽屉）', () => {
    // 无条件声明会让 position:fixed 的覆盖式抽屉回到文档流，主区被挤到 55px。
    // 只查元素本身：::before/::after 的 position:absolute 是窗楣窗台的正常写法，不在此列。
    const sidebarPos = allBlocks.filter((b) => {
      if (!b.selector.includes('.sv-sidebar') || b.selector.includes('::')) return false;
      return /(^|[\s;{])position\s*:/.test(b.body);
    });
    expect(sidebarPos.length, '侧栏 position 规则不应消失（桌面态需要它做定位基准）').toBeGreaterThan(0);
    for (const b of sidebarPos) {
      expect(b.media, `侧栏 position 必须写在 @media (min-width: 768px) 内：${b.selector}`).toContain(
        'min-width: 768px',
      );
    }
  });

  it('6. 空态列定义落在真正的 grid 容器上（.sv-start 本身不是 grid）', () => {
    const gridDecl = allBlocks.filter((b) => b.body.includes('grid-template-columns'));
    for (const b of gridDecl) {
      expect(
        /\.sv-(start|task-start)-grid/.test(b.selector),
        `grid-template-columns 打在非 grid 容器上会是空转：${b.selector}`,
      ).toBe(true);
    }
    // .sv-start 自己的块里不能再出现列定义
    const startBlock = allBlocks.filter(
      (b) => /^html\[data-theme='stained-glass'\] \.sv-start,/.test(b.selector),
    );
    for (const b of startBlock) {
      expect(b.body).not.toContain('grid-template-columns');
      expect(b.body).not.toMatch(/^\s*gap\s*:/m);
    }
  });

  it('7. 顶栏统计格有暗底（防回退成「白底 + 暖白字」的 1.15:1）', () => {
    // 基础样式 .sv-topbar-stats 是 background: var(--sv-white) #fdf8ec，
    // 而 `.sv-topbar *` 把文字刷成 #f0e8d6 —— 两者叠加只有 1.15:1。
    const stats = allBlocks.find(
      (b) => b.selector.includes('.sv-topbar .sv-topbar-stats') && !b.selector.includes('menu'),
    );
    expect(stats, '缺少顶栏统计格的暗底规则').toBeTruthy();
    expect(stats!.body).toMatch(/background-color:\s*rgba\(9,\s*14,\s*32,/);
  });

  it('8. 零引用的 --kg-* 令牌已清除（令牌表不留死条目）', () => {
    for (const dead of ['--kg-lead-soft', '--kg-sapphire-lit', '--kg-emerald', '--kg-amethyst']) {
      expect(code, `${dead} 已零引用，应从令牌表删除`).not.toContain(dead);
    }
    // 反向：声明出来的每个令牌都必须至少被用一次，避免再攒出新的死条目
    const declared = [...code.matchAll(/^\s*(--kg-[a-z-]+):/gm)].map((m) => m[1]);
    expect(declared.length).toBeGreaterThan(5);
    for (const name of declared) {
      expect(code.includes(`var(${name})`), `${name} 已声明但从未被引用`).toBe(true);
    }
  });
});
