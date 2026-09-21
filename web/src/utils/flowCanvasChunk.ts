// 画布 chunk 的懒加载入口(单独成模块,理由见下)。
//
// 画布组件重(含 @vue-flow 及其 d3/@vueuse 传递依赖),只在切到画布视图时动态 import;
// 抽出这个函数是为了让「**加载失败**」这条路径可测:测试把本模块替换成必然失败的实现即可,
// 而直接 mock 组件模块(`vi.mock('./AgentFlowCanvas.vue', () => { throw … })`)会让 vitest
// 把工厂抛错记成 run 级 unhandled error —— 整轮测试都会因此失败。
//
// 返回类型由 `import()` 推断,不写成 `Promise<Component>`:后者会丢掉画布组件的 props
// 类型检查(vue-tsc 是硬门禁,不能为了测试放宽类型)。

/** 动态 import 画布视图组件 */
export function loadFlowCanvas() {
  return import('../components/settings/AgentFlowCanvas.vue');
}
