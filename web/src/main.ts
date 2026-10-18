// 前端入口
import { createApp } from 'vue';
import { createPinia } from 'pinia';
import App from './App.vue';
import { installGlobalErrorHandlers } from './globalErrorHandlers';
import { installInputModality } from './inputModality';
import { announceUiReady, initTheme, installThemeApi } from './theme/registry';
import { applyPerfTier, resolvePerfTier } from './visual/perfTier';
import { useUiPrefsStore } from './stores/uiPrefs';
// 展示字体(拉丁字标/数字用,本地打包离线可用)
import '@fontsource/archivo-black';
import './style.css';

const app = createApp(App);
const pinia = createPinia();
app.use(pinia);

// 输入模态标记(UIP-12):html[data-input-mode] 供 styles/panels.css 收窄 select 焦点样式;
// 尽早安装(首帧即有标记能力),无标记时行为与现状逐字一致。
installInputModality();

// 性能档位:首帧前写 html[data-perf-tier],主题 CSS 与 WebGL 共用同一判定。
const perfTier = resolvePerfTier({
  cores: navigator.hardwareConcurrency,
  deviceMemory: (navigator as Navigator & { deviceMemory?: number }).deviceMemory,
  mobile: typeof window.matchMedia === 'function' && window.matchMedia('(max-width: 767px)').matches,
});
applyPerfTier(document.documentElement, perfTier);

// 主题插件:对外暴露 window.KedaiUI,并在首帧前同步应用已保存的主题令牌。
// 主题 CSS/字体 chunk 只在该主题首次激活时按需加载(不膨胀首屏)。
installThemeApi();
void initTheme();

// 全局错误兜底(计划批次 5.1):必须在 mount 之前注册,否则首屏渲染期的异常会漏掉。
// 经 useUiPrefsStore(pinia) 显式取 store——main.ts 里还没有活跃的 pinia 实例上下文,
// 不能直接调 useUiPrefsStore()。异常只写内存态横幅,不落库、不含聊天正文。
const uiPrefs = useUiPrefsStore(pinia);
installGlobalErrorHandlers({
  app,
  onError: (message) => {
    uiPrefs.globalError = message;
  },
});

app.mount('#app');

// 挂载完成后再广播「前端插件 API 已就绪」;早于 mount 派发时外部插件可能还没有监听器。
announceUiReady();
