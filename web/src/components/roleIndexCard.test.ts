// 角色索引卡片的源码级视觉契约（UIFIX-8，2026-11-02）。
//
// 为什么用源码文本而不是渲染断言：这几条都是**尺寸与显隐**，jsdom 无布局引擎、
// vitest 也不加载 scoped 样式（同 sidebarTaskLayout.test.ts 的口径）。它们容易在
// 后续微调里被无声改回去，而退化只有肉眼能发现：
//   · 序号原本只有 11px，「大号数字」名不副实；
//   · 箭头原本常显，三个角色排下来满屏箭头，视觉噪音压过名字；
//   · 选中态原本整行淡粉，看起来像一块色块而不是「被点名的那一行」。
//
// 另附一条红线：ChatWindow.vue 只剩个位数行余量（check-file-size 阈值 1000），
// 所以这里的样式改动必须保持精简——用例把它钉住，防止后续继续往里堆。
import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';

const chatWindow = readFileSync(new URL('./ChatWindow.vue', import.meta.url), 'utf8');

describe('UIFIX-8 角色索引卡片契约', () => {
  it('序号提到海报级字号（不再是 11px 的「假大号数字」）', () => {
    // 用 [^}]* 限定在规则块内,避免跨到文件后段其它 font-size
    expect(chatWindow).toMatch(/\.sv-start-char-idx \{[^}]*font-size: var\(--text-md\);/);
    expect(chatWindow).not.toMatch(/\.sv-start-char-idx \{[^}]*font-size: var\(--text-2xs\);/);
  });

  it('头像 40px,选中时用外扩黑框 + 粉晕标识(不动 color)', () => {
    expect(chatWindow).toContain('.sv-start-char .sv-avatar { width: 40px; height: 40px;');
    expect(chatWindow).toMatch(
      /\.sv-start-char\.active \.sv-avatar\.char \{[^}]*outline: var\(--bw\) solid var\(--sv-ink\);[^}]*box-shadow: 0 0 0 var\(--space-1\) var\(--sv-pink\);/,
    );
    // **不得**改头像的 color:content.css 有 `.sv-avatar.char{color:#fff !important}`,
    // 组件 scoped 又禁 !important,改 color 只会得到白底白字的空方框(首版实测)。
    expect(chatWindow).not.toMatch(/\.sv-start-char\.active \.sv-avatar\.char \{[^}]*color:/);
  });

  it('选中态是白底 + 左侧黑线，不再是整行淡粉', () => {
    expect(chatWindow).toMatch(
      /\.sv-start-char:hover,\s*\n\s*\.sv-start-char\.active \{[^}]*background: var\(--sv-white\);[^}]*box-shadow: inset 2px 0 0 var\(--sv-ink\);/,
    );
    // 淡粉不得再出现在选中规则里
    expect(chatWindow).not.toMatch(/\.sv-start-char\.active \{[^}]*--sv-pink-light/);
  });

  it('箭头常态隐去，悬停/选中才出现', () => {
    expect(chatWindow).toMatch(/\.sv-start-char-arrow \{[^}]*opacity: 0;/);
    expect(chatWindow).toMatch(/\.sv-start-char:hover \.sv-start-char-arrow \{[^}]*opacity: 1;/);
    expect(chatWindow).toMatch(/\.sv-start-char\.active \.sv-start-char-arrow \{[^}]*opacity: 1;/);
  });

  it('名字升到 15px/900（与序号同为主视觉层次）', () => {
    expect(chatWindow).toMatch(/\.sv-start-char-name \{[^}]*font-size: var\(--text-md\);[^}]*font-weight: 900;/);
  });
});
