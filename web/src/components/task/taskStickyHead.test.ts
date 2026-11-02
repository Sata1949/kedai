// 任务台常驻表头的源码级布局契约（UIFIX-8，2026-11-02）。
//
// jsdom 无布局引擎、vitest 也不加载 scoped 样式，尺寸类回归只能用源码文本守。
// 沿用 sidebarTaskLayout.test.ts 的同一口径（.css 走 node:fs，.vue 用 ?raw）。
//
// 这里钉住的是**两条一改就悄悄退化、且只有肉眼能发现**的性质：
//   1. 档位条必须与标题/状态矩阵在**同一个** sticky 容器内 —— 一旦有人把档位条摆回
//      TaskTranscript 或另起一个 sticky，滚两屏它就消失（这正是本批要修的原缺陷）；
//   2. 表头高度有硬上限 —— 它是每帧都吃视口高度的固定成本，实测首版 166px/720px
//      视口 = 23%，压缩到 144px 后约 20%。后续往里加行必须同时确认没超这条线。
import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';

const readText = (rel: string): string => readFileSync(new URL(rel, import.meta.url), 'utf8');

const statusPanel = readText('./TaskStatusPanel.vue');
const taskBoard = readText('../TaskBoard.vue');
const transcript = readText('./TaskTranscript.vue');

describe('UIFIX-8 常驻表头契约', () => {
  it('档位条由表头插槽承载，且不再渲染在 TaskTranscript 内部', () => {
    // TaskBoard 把 TaskTranscriptModes 注入 TaskStatusPanel 的 modes 插槽
    expect(taskBoard).toContain('<TaskTranscriptModes />');
    expect(taskBoard).toMatch(/<template #modes>/);
    // TaskTranscript 里不得再出现档位按钮（否则两处都能切档，且滚走的那份还会误导）
    expect(transcript).not.toContain('sv-transcript-mode');
  });

  it('标题 / 状态矩阵 / 档位插槽同属一个 sticky 容器', () => {
    // .sv-task-sticky 必须是唯一 sticky，且包住三者
    expect(statusPanel).toMatch(/\.sv-task-sticky \{[\s\S]*?position: sticky;/);
    // 旧的「只给标题 sticky」写法不得回归
    expect(statusPanel).not.toMatch(/\.sv-task-head \{[^}]*position: sticky/);
    expect(statusPanel).toContain('sv-task-sticky-modes');
  });

  it('表头是实色纸面（sticky 若透明，滚动内容会从背后透出）', () => {
    expect(statusPanel).toMatch(/\.sv-task-sticky \{[^}]*background: var\(--sv-paper\);/);
  });

  it('表头高度受控：内边距与矩阵行高三处都是紧凑档，防止无声膨胀', () => {
    expect(statusPanel).toMatch(/\.sv-task-sticky \{[^}]*padding: var\(--space-1-5\) 0 var\(--space-2\);/);
    expect(statusPanel).toMatch(/\.sv-task-status-tag \{[^}]*min-height: 32px;/);
    expect(statusPanel).toMatch(/\.sv-task-head-actions \.sv-btn \{ min-height: var\(--control-h-sm\);/);
  });

  it('标题上方有整宽分隔线（原只有左侧 96px 红黑段带，没有横向分隔）', () => {
    expect(statusPanel).toMatch(/\.sv-task-sticky \{[^}]*border-top: var\(--bw-thin\) solid var\(--sv-line-strong\);/);
  });
});
