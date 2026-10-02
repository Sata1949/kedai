// 输入模态标记(2026-10-02 UIP-12,R-UIP1 裁决的候选方案落地)。
//
// 背景:Chromium 把 <select> 的点选也视为键盘交互,鼠标点击后 :focus-visible 持续命中,
// 「鼠标不显 / 键盘显」的纯 CSS 分型在 select 上不可达(实测见 UIP 章 R-UIP1)。本模块
// 用「全局输入模态标记」补齐这个信号:指针交互 → html[data-input-mode="pointer"],
// 键盘交互 → "keyboard"。**默认(尚无任何交互)不写属性 = 现状行为**。
//
// 消费点:styles/panels.css 按该标记收窄 select 的 :focus 粉框(仅指针模态);
// 文本框的焦点粉框不受影响(打字反馈保留,沿 UIP-7 口径)。
// 文档事件在「捕获阶段」监听:先于任何组件的 stopPropagation 生效。
const MODE_KEY = 'inputMode';

/**
 * 安装全局监听。在应用入口 mount 之前调用一次;返回卸载函数(测试用)。
 * 只在浏览器环境调用;模块导入本身无副作用(SSR/测试可安全 import)。
 */
export function installInputModality(): () => void {
  const root = document.documentElement;
  const set = (mode: 'pointer' | 'keyboard'): void => {
    if (root.dataset[MODE_KEY] === mode) return; // 仅变化时写,避免无谓的属性变更
    root.dataset[MODE_KEY] = mode;
  };
  const onPointerDown = (): void => set('pointer');
  const onKeyDown = (): void => set('keyboard');
  document.addEventListener('pointerdown', onPointerDown, { capture: true, passive: true });
  document.addEventListener('keydown', onKeyDown, { capture: true });
  return () => {
    document.removeEventListener('pointerdown', onPointerDown, { capture: true });
    document.removeEventListener('keydown', onKeyDown, { capture: true });
  };
}
