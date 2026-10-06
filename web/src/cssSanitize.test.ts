// @vitest-environment jsdom
// FE-7 / FE-8(2026-10-06 前端修复线收口批):CSS 作用域越界守卫与 image-set URL 白名单。
// 本文件此前不存在(cssSanitize.ts 属零测试文件)——先补测试再收紧,收紧不得误伤
// 既有卡(仓库有「只有文字没有界面」历史事故)。
import { describe, expect, it } from 'vitest';
import { cssUrlsSafe, sanitizeCssDeclarations, scopeCss } from './cssSanitize';

describe('FE-7 作用域越界守卫(scopeCss)', () => {
  it('最小复现:兄弟组合器确会跳出容器,子组合器不会(浏览器语义核实)', () => {
    document.body.innerHTML =
      '<div data-kd-scope="s"><span class="in">内</span></div><span class="out">外</span>';
    // `[attr] ~ .out`:兄弟关系落在**容器之外**——这正是越界可被用来遮挡宿主界面/
    // 点击劫持的机制(FE-7 要拒绝的形态)
    expect(document.querySelectorAll('[data-kd-scope="s"] ~ .out').length).toBe(1);
    // `[attr] > .in`:子关系仍属容器子树,不越界(FE-7 保留该形态)
    expect(document.querySelectorAll('[data-kd-scope="s"] > .in').length).toBe(1);
    expect(document.querySelectorAll('[data-kd-scope="s"] > .out').length).toBe(0);
  });

  it('以 ~ / + 开头的选择器被拒(整条规则丢弃)', () => {
    const tilde = scopeCss('~ .sv-msg { color: red }', 's1');
    expect(tilde.css).not.toContain('~');
    expect(tilde.css.trim()).toBe('');
    const plus = scopeCss('+ div { color: red }', 's2');
    expect(plus.css).not.toContain('+ div');
    expect(plus.css.trim()).toBe('');
  });

  it('body ~ .x 经 body 重写后同样被拦(检查的是重写后文本,不是原始选择器)', () => {
    const out = scopeCss('body ~ .x { color: red }', 's3');
    expect(out.css).not.toContain('~');
    expect(out.css.trim()).toBe('');
  });

  it('逗号列表中仅越界项被剔除,合法项保留并正常作用域化', () => {
    const out = scopeCss('.a, ~ .b, .c { color: red }', 's4');
    expect(out.css).toContain('[data-kd-scope="s4"] .a');
    expect(out.css).toContain('[data-kd-scope="s4"] .c');
    expect(out.css).not.toContain('~');
    expect(out.css).not.toContain('.b');
  });

  it('合法选择器不受影响:后代 / body 重写 / 子组合器 / @media 递归内一致生效', () => {
    const out = scopeCss(
      '.a { color: red } body.theme-blue .tide-card { color: #fff } > .inputbar { color: #000 }',
      's5',
    );
    expect(out.css).toContain('[data-kd-scope="s5"] .a');
    expect(out.css).toContain('[data-kd-scope="s5"].theme-blue .tide-card');
    // `>` 保留:前缀后仍是容器子元素(见上方最小复现)
    expect(out.css).toContain('[data-kd-scope="s5"] > .inputbar');
    const media = scopeCss('@media (min-width: 600px) { .ok { color: red } ~ .bad { color: blue } }', 's6');
    expect(media.css).toContain('[data-kd-scope="s6"] .ok');
    expect(media.css).not.toContain('~');
    expect(media.css).not.toContain('.bad');
  });
});

describe('FE-8 image-set URL 白名单(cssUrlsSafe / sanitizeCssDeclarations)', () => {
  it('image-set 内裸字符串 URL 逐条过白名单', () => {
    expect(cssUrlsSafe('image-set("http://evil.test/x.png" 1x)')).toBe(false);
    expect(cssUrlsSafe('image-set("https://ok.test/x.png" 1x)')).toBe(true);
    expect(cssUrlsSafe('-webkit-image-set("https://ok.test/x.png" 1x)')).toBe(true);
    expect(cssUrlsSafe('image-set("data:image/png;base64,AA" 1x)')).toBe(true);
  });

  it('url() 形态与混排形态同样被拦(既有的 url() 路径不回归)', () => {
    expect(cssUrlsSafe('image-set(url(http://evil.test/x.png) 1x)')).toBe(false);
    expect(cssUrlsSafe('image-set(url(https://ok.test/x.png) 1x, "http://evil.test/y.png" 2x)')).toBe(false);
    expect(cssUrlsSafe('url(https://ok.test/x.png), image-set("https://ok.test/y.png" 1x)')).toBe(true);
  });

  it('声明级清洗:越界 image-set 整条被剔,合法 image-set 原样保留', () => {
    expect(sanitizeCssDeclarations('background: image-set("http://evil.test/x.png" 1x)')).toBe('');
    expect(sanitizeCssDeclarations('background: image-set("https://ok.test/x.png" 1x)')).toBe(
      'background: image-set("https://ok.test/x.png" 1x)',
    );
  });
});
