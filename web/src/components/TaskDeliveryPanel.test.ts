// @vitest-environment jsdom
// TMR-6 交付校验卡片：四态渲染纪律（未绑定 / 摘要缺失 / 未声明 / 有声明含缺失）。
import { describe, expect, it } from 'vitest';
import { mount } from '@vue/test-utils';
import TaskDeliveryPanel from './TaskDeliveryPanel.vue';
import type { TaskDeliverySummary } from '../api';

function summary(overrides: Partial<TaskDeliverySummary> = {}): TaskDeliverySummary {
  return {
    checked: true,
    workspace_available: true,
    required: [
      { path: 'index.html', owner: '【主Agent-1】写入口', exists: true },
      { path: 'parts/ui.js', owner: '【主Agent-2】写分片', exists: false },
    ],
    missing: ['parts/ui.js'],
    changed: ['index.html'],
    changed_count: 3,
    ...overrides,
  };
}

function mountPanel(delivery: TaskDeliverySummary | null, workspaceBound = true) {
  return mount(TaskDeliveryPanel, {
    props: { delivery, workspaceBound },
  });
}

describe('TaskDeliveryPanel', () => {
  it('未绑定工作区：明说不做交付校验', () => {
    const w = mountPanel(null, false);
    expect(w.text()).toContain('未绑定工作区');
    expect(w.text()).not.toContain('缺失 0');
  });

  it('绑定但摘要缺失（旧服务端）：明说摘要不可用', () => {
    const w = mountPanel(null, true);
    expect(w.text()).toContain('交付摘要不可用');
  });

  it('未声明必需文件：只说明弱校验与变更数', () => {
    const w = mountPanel(
      summary({ checked: false, required: [], missing: [], changed_count: 3 }),
    );
    expect(w.text()).toContain('未声明必需文件');
    expect(w.text()).toContain('变更 3');
    expect(w.find('.sv-delivery-files').exists()).toBe(false);
  });

  it('有声明含缺失：计数、缺失项与 owner 都渲染，缺失项标 bad', () => {
    const w = mountPanel(summary());
    const text = w.text();
    expect(text).toContain('必需 2');
    expect(text).toContain('已生成 1');
    expect(text).toContain('缺失 1');
    expect(text).toContain('变更 3');
    const rows = w.findAll('.sv-delivery-files li');
    expect(rows).toHaveLength(2);
    expect(rows[0].text()).toContain('index.html');
    expect(rows[0].classes()).not.toContain('bad');
    expect(rows[1].text()).toContain('parts/ui.js');
    expect(rows[1].text()).toContain('【主Agent-2】写分片');
    expect(rows[1].classes()).toContain('bad');
  });

  it('工作区不可访问：警示缺失判定不可信', () => {
    const w = mountPanel(summary({ workspace_available: false, missing: [] }));
    expect(w.text()).toContain('工作区当前不可访问');
    expect(w.text()).toContain('缺失判定不可信');
  });

  it('旧服务端缺 content_issues/truncated：按无问题容错', () => {
    const w = mountPanel(summary());
    expect(w.text()).not.toContain('交付内容校验');
    expect(w.text()).not.toContain('未完成的子目标');
  });

  it('内容体检与未完成步骤：逐项展示警示', () => {
    const w = mountPanel(
      summary({
        content_issues: [
          'index.html:含 2 行 @@ 未替换槽位',
          'parts/main.js:文件为空(0B)',
        ],
        truncated: ['【主Agent-1】总装'],
      }),
    );
    const text = w.text();
    expect(text).toContain('未完成 1');
    expect(text).toContain('未完成的子目标');
    expect(text).toContain('【主Agent-1】总装');
    expect(text).toContain('index.html:含 2 行 @@ 未替换槽位');
    expect(text).toContain('parts/main.js:文件为空(0B)');
  });

  it('运行期自检问题：独立展示，旧服务端缺字段时静默', () => {
    const w = mountPanel(
      summary({
        runtime_issues: ['index.html:页面没有 canvas 画布(3D/可视化交付未渲染)'],
      }),
    );
    const text = w.text();
    expect(text).toContain('运行期自检');
    expect(text).toContain('页面没有 canvas 画布');
    expect(text).not.toContain('交付内容校验');

    expect(mountPanel(summary()).text()).not.toContain('运行期自检');
  });

  it('交付卫生提示：只展示提示，不冒充内容校验失败', () => {
    const w = mountPanel(
      summary({
        hygiene_issues: ['_check.js:未声明的临时/草稿文件(交付前应清理)'],
      }),
    );
    const text = w.text();
    expect(text).toContain('交付卫生');
    expect(text).toContain('_check.js');
    expect(text).not.toContain('交付内容校验');
  });
});
