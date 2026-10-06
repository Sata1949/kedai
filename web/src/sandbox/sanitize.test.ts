// sanitizeScriptHtmlWithStyles 测试:运行期 <style> 通道($().html()/append()/appendCreated
// 落 DOM)。背景:SCRIPT_HTML_WHITELIST 43 标签不含 style,飞讯终端等脚本运行期
// $().append('<style>…') 注入的样式被整段剥掉 → UI 无样式裸渲染。样式段须走与
// render.ts 静态管道同一管线(声明级清洗 → 容器作用域化 → keyframes 重命名回写)。
import { describe, expect, it } from 'vitest';
import sanitizeHtml from 'sanitize-html';
import { SCRIPT_HTML_WHITELIST, sanitizeScriptHtmlWithStyles } from './sanitize';

describe('sanitizeScriptHtmlWithStyles(运行期 <style> 通道)', () => {
  it('<style> 段保留:声明级清洗后选择器带 [data-kd-scope] 前缀,非样式段走白名单', () => {
    const out = sanitizeScriptHtmlWithStyles(
      '<div class="fx-term">x</div><style>.fx-term{color:red;position:fixed;z-index:50}</style>',
      'card-c1',
    );
    expect(out).toContain('<style>');
    expect(out).toContain('[data-kd-scope="card-c1"] .fx-term');
    // 布局声明放行(与静态管道同一保真口径:剥光会让卡片「只有文字没有界面」)
    expect(out).toContain('position: fixed');
    expect(out).toContain('z-index: 50');
    expect(out).toContain('<div class="fx-term">x</div>');
  });

  it('样式段剥 expression()/@import/非白名单 url(http:),放行 https 与 data:image', () => {
    const out = sanitizeScriptHtmlWithStyles(
      '<style>.a{width:expression(alert(1))} @import "https://fonts.example/x.css";' +
        '.b{background:url(http://evil.test/x.png)}' +
        '.c{background:url(https://cdn.test/x.png);mask:url("data:image/svg+xml;base64,AA")}</style>',
      's1',
    );
    expect(out).not.toMatch(/expression|@import/i);
    expect(out).not.toContain('http://evil.test');
    expect(out).toContain('https://cdn.test/x.png');
    expect(out).toContain('data:image/svg+xml');
  });

  it('<script> 仍剥除、事件属性仍剥除;inline style 属性保留(经声明级清洗)', () => {
    const out = sanitizeScriptHtmlWithStyles(
      '<div onclick="hack()" style="color:red">y</div><script>alert(1)</script>',
      's1',
    );
    expect(out).not.toContain('<script');
    expect(out).not.toContain('alert(1)');
    expect(out).not.toContain('onclick');
    // 合法声明照常保留(清洗/序列化口径见下方 FE-6 块;此处输入已是管线规范形态)
    expect(out).toContain('style="color:red"');
  });

  it('keyframes 重命名并回写同段 CSS 引用(与 render.ts 静态管道同口径:映射只作用于 CSS 文本)', () => {
    const out = sanitizeScriptHtmlWithStyles(
      '<style>@keyframes pulse-glow{from{opacity:0}to{opacity:1}} .card{animation:pulse-glow 2s infinite}</style>',
      's2',
    );
    expect(out).toContain('@keyframes kd-s2-pulse-glow');
    expect(out).toContain('animation: kd-s2-pulse-glow 2s infinite');
    expect(out).not.toContain('kd-s2-kd-s2');
  });

  it('多段样式与非样式段按原顺序重组;空样式段(全被剥)不产出空标签', () => {
    const out = sanitizeScriptHtmlWithStyles(
      '<p>前</p><style>.a{color:red}</style><div>中</div><style>@import "x";</style><span>后</span>',
      's3',
    );
    const idxP = out.indexOf('<p>前</p>');
    const idxStyle = out.indexOf('<style>');
    const idxDiv = out.indexOf('<div>中</div>');
    const idxSpan = out.indexOf('<span>后</span>');
    expect(idxP).toBeGreaterThanOrEqual(0);
    expect(idxStyle).toBeGreaterThan(idxP);
    expect(idxDiv).toBeGreaterThan(idxStyle);
    expect(idxSpan).toBeGreaterThan(idxDiv);
    // 第二段样式整体被剥(@import),不产生空 <style></style>
    expect(out.indexOf('<style>', idxStyle + 1)).toBe(-1);
  });

  it('body 前导选择器重写为容器自身(与静态管道 scopeSelector 同语义)', () => {
    const out = sanitizeScriptHtmlWithStyles(
      '<style>body.theme-blue .tide-card{color:#fff}</style>',
      's4',
    );
    expect(out).toContain('[data-kd-scope="s4"].theme-blue .tide-card');
  });
});

// FE-6(2026-10-06 前端修复线收口批):style 属性补声明级清洗。
// 此前本通道(运行期注入)把 style 属性整体放行,而静态通道(render.ts)走
// sanitizeStyleAttribute——同一份作者 CSS,写在 <style> 里被清洗、写在 style=""
// 里不清洗(两通道口径不一致)。收紧前先立保真基线:布局声明与白名单 url()
// 必须逐条保留,否则既有卡「只有文字没有界面」。
//
// 断言口径说明:sanitize-html 2.x 默认 `parseStyleAttributes: true`,会用自带的
// postcss 把 style 属性**解析后重新序列化**(`prop:value` 以 `;` 连接,无空格);
// 该阶段在本批之前就已存在(与静态通道同库同配置),不是本批引入的形态变化。
// 因此保真基线的判据是「声明集合与值逐条不变」,而不是与输入字面同字节。
describe('FE-6 style 属性声明级清洗(运行期注入通道)', () => {
  it('保真基线:布局声明逐条保留(值不变,不受清洗影响)', () => {
    const out = sanitizeScriptHtmlWithStyles(
      '<div style="position: fixed; z-index: 50; inset: 0; pointer-events: none">x</div>',
      'f1',
    );
    expect(out).toContain('style="position:fixed;z-index:50;inset:0;pointer-events:none"');
  });

  it('保真基线:https url() 与 data:image 声明保留,声明顺序不变', () => {
    const out = sanitizeScriptHtmlWithStyles(
      '<div style="background-image: url(https://cdn.test/x.png); border-radius: 8px">x</div>' +
        '<span style="mask: url(\'data:image/svg+xml;base64,AA\')">y</span>',
      'f2',
    );
    expect(out).toContain('style="background-image:url(https://cdn.test/x.png);border-radius:8px"');
    expect(out).toContain("mask:url('data:image/svg+xml;base64,AA')");
  });

  it('输入已是管线规范形态时逐字节不变(含无空格紧凑写法)', () => {
    const out = sanitizeScriptHtmlWithStyles('<div style="color:#fff;top:0">x</div>', 'f3');
    expect(out).toContain('style="color:#fff;top:0"');
  });

  it('非白名单 url(http:)/expression()/behavior 的声明整条被剥(此前整体放行)', () => {
    const out = sanitizeScriptHtmlWithStyles(
      '<div style="background: url(http://evil.test/x.png); color: red">a</div>' +
        '<div style="width: expression(alert(1))">b</div>' +
        '<div style="behavior: url(x.htc)">c</div>',
      'f4',
    );
    expect(out).not.toContain('evil.test');
    expect(out).not.toContain('expression');
    expect(out).not.toContain('behavior');
    // 合法声明不受影响:同一属性上的 color 保留
    expect(out).toContain('color:red');
  });

  it('整条 style 被剥后不留空壳属性;无 scope 回退路径(dom-rpc)同样清洗', () => {
    const out = sanitizeScriptHtmlWithStyles('<div style="behavior: url(x.htc)">y</div>', 'f5');
    expect(out).not.toContain('style=');
    const noScope = sanitizeHtml(
      '<div style="background: url(http://evil.test/x.png)">z</div>',
      SCRIPT_HTML_WHITELIST,
    );
    expect(noScope).not.toContain('evil.test');
    expect(noScope).not.toContain('style=');
  });
});
