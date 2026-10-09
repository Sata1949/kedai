import { describe, it, expect } from 'vitest';
import { renderMarkdown, renderMarkdownWithHtml, stripHtmlComments, stripMvuBlocks } from './markdown';

describe('renderMarkdown', () => {
  it('基础 markdown 渲染', () => {
    const html = renderMarkdown('**粗体** 与 `代码`');
    expect(html).toContain('<strong>粗体</strong>');
    expect(html).toContain('<code>代码</code>');
  });

  it('代码块渲染', () => {
    const html = renderMarkdown('```js\nconst a = 1;\n```');
    expect(html).toContain('<pre><code class="language-js">');
    expect(html).toContain('const a = 1;');
  });

  it('原始 HTML 被净化(html:false)', () => {
    const html = renderMarkdown('<script>alert(1)</script>**x**');
    expect(html).not.toContain('<script>');
    expect(html).toContain('<strong>x</strong>');
  });

  it('链接新窗口', () => {
    const html = renderMarkdown('[链接](https://example.com)');
    expect(html).toContain('target="_blank"');
    expect(html).toContain('rel="noopener noreferrer"');
  });

  it('剥离 mvu UpdateVariable 块', () => {
    const html = renderMarkdown(
      '正文\n<UpdateVariable>\n_.set("a", 1, 2);\n</UpdateVariable>\n继续',
    );
    expect(html).not.toContain('UpdateVariable');
    expect(html).not.toContain('_.set');
    expect(html).toContain('正文');
    expect(html).toContain('继续');
  });

  it('空串/纯块返回空', () => {
    expect(renderMarkdown('')).toBe('');
    expect(renderMarkdown('<UpdateVariable>_.set("a",1,2)</UpdateVariable>')).toBe('');
  });

  it('剥离 <status_current_variable> 段标签(内容保留)', () => {
    const html = renderMarkdown('<status_current_variable>好感度: 88</status_current_variable>正文');
    expect(html).not.toContain('status_current_variable');
    expect(html).toContain('好感度: 88');
    expect(html).toContain('正文');
  });

  it('status_current_variable 大小写不敏感', () => {
    const html = renderMarkdown('<STATUS_CURRENT_VARIABLE>内容</STATUS_CURRENT_VARIABLE>');
    expect(html).not.toContain('STATUS_CURRENT_VARIABLE');
    expect(html).toContain('内容');
  });
});

describe('stripMvuBlocks', () => {
  it('仅剥离块', () => {
    expect(stripMvuBlocks('a<UpdateVariable>x</UpdateVariable>b')).toBe('ab');
  });

  it('同时剥离 UpdateVariable 块与 status_current_variable 标签', () => {
    expect(stripMvuBlocks('a<UpdateVariable>x</UpdateVariable><status_current_variable>y</status_current_variable>b')).toBe('ayb');
  });
});

// RPFLOW 提交 3:酒馆式 HTML 渲染路径的文本层工具
describe('stripHtmlComments', () => {
  it('配对注释剥除,注释外文本保留(含多行)', () => {
    expect(stripHtmlComments('前<!-- 注释 -->后')).toBe('前后');
    expect(stripHtmlComments('a<!--多行\n注释-->b')).toBe('ab');
    expect(stripHtmlComments('无注释')).toBe('无注释');
  });

  it('未闭合尾注释剥到结尾(流式跨帧防闪字面)', () => {
    expect(stripHtmlComments('正文<!-- 未闭合注释')).toBe('正文');
    expect(stripHtmlComments('前<!-- 已闭合 -->中<!-- 未闭合')).toBe('前中');
    expect(stripHtmlComments('<!-- 从注释开头')).toBe('');
  });
});

describe('renderMarkdownWithHtml(酒馆式 HTML 渲染路径)', () => {
  it('原始 HTML 标签保留(html:true;清洗由调用方白名单负责)', () => {
    const html = renderMarkdownWithHtml('<details><summary>详情</summary>正文</details>');
    expect(html).toContain('<details>');
    expect(html).toContain('<summary>详情</summary>');
    expect(html).not.toContain('&lt;details');
  });

  it('注释先剥,不进入渲染(含未闭合尾注释)', () => {
    const html = renderMarkdownWithHtml('前<!-- 隐藏 -->后');
    expect(html).not.toContain('隐藏');
    expect(html).toContain('前');
    expect(html).toContain('后');
    expect(renderMarkdownWithHtml('正文<!-- 未闭合')).not.toContain('未闭合');
  });

  it('markdown 语法仍生效;协议块与段标签仍剥离', () => {
    const html = renderMarkdownWithHtml('**粗体**\n<UpdateVariable>_.set("a",1,2)</UpdateVariable>');
    expect(html).toContain('<strong>粗体</strong>');
    expect(html).not.toContain('UpdateVariable');
    expect(renderMarkdownWithHtml('<status_current_variable>好感:88</status_current_variable>')).toContain('好感:88');
  });

  it('链接新窗口(与默认实例一致);空串返回空', () => {
    const html = renderMarkdownWithHtml('[链接](https://example.com)');
    expect(html).toContain('target="_blank"');
    expect(html).toContain('rel="noopener noreferrer"');
    expect(renderMarkdownWithHtml('')).toBe('');
  });
});
