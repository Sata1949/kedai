// 主区懒加载骨架(UIVIS-3/UI-C2,2026-10-31)。
//
// 背景:ChatWindow / TaskBoard 是 defineAsyncComponent 产物(App.vue 里按 appMode 二选一),
// chunk 下载期间 `defineAsyncComponent` 返回空——主区 .sv-main 内**没有任何可见子元素**,
// 表现为「切模式后一大片空白」,弱网/弱机上尤其明显(审计实测:过渡结束后才出现内容)。
//
// 做法:给这两条懒加载统一挂 loadingComponent。骨架用最小实现而不是 SkeletonBlock:
// 主区要给的是「这里马上有一屏内容」的占位轮廓(标题行 + 若干正文行),SkeletonBlock 是
// 字段级(12px 细条)形态,铺满主区会显得像加载失败。
//
// 为什么不做成 lazyModal 的通用能力:lazyModal 服务的是**弹窗/浮层**(打开即有内容),
// 冷启动时弹窗还没打开,给它们配 loading 骨架没有可见收益;主区是首屏必挂载的组件,
// 才是唯一「空白会被人看到」的位置。保持 lazyModal 语义不变,避免牵动 16 个弹窗的行为。
import { defineAsyncComponent, type AsyncComponentLoader, type Component } from 'vue';
import MainLoadingSkeleton from './components/MainLoadingSkeleton.vue';

/**
 * 主区懒加载包装:chunk 加载期间显示 MainLoadingSkeleton。
 *
 * 刻意**不**复用 lazyModal 的失败重试与全局错误条:主区两组件都是首屏必需,加载失败时
 * 页面本来就没法用,重试一次的价值远低于保持简单;错误仍会经 Vue 的 errorHandler
 * 走 globalError 兜底(App.vue 已注册),不会被吞掉。
 */
export function lazyMain<T extends Component>(loader: AsyncComponentLoader<T>): T {
  return defineAsyncComponent({
    loader,
    loadingComponent: MainLoadingSkeleton,
    // 骨架至少留 180ms:chunk 命中 HTTP 缓存时加载只有几毫秒,瞬时闪一块骨架比不闪更差
    delay: 180,
  }) as T;
}
