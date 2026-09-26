import { defineConfig } from 'vite';
import vue from '@vitejs/plugin-vue';
import tailwindcss from '@tailwindcss/vite';

// 前端开发服务器:API 请求代理到 Rust 后端(端口 3001 为硬契约)
export default defineConfig({
  plugins: [vue(), tailwindcss()],
  server: {
    port: 5173,
    proxy: {
      '/api': { target: 'http://127.0.0.1:3001', changeOrigin: true }
    }
  },
  build: {
    outDir: 'dist',
    // 生产构建禁 sourcemap:防前端源码反编译(§8.3.2-T3);Vite 默认 false,此处显式固化防回归。
    sourcemap: false,
    rollupOptions: {
      output: {
        manualChunks(id) {
          if (id.includes('node_modules')) {
            // 画布库单列一块(2026-09-20 二维批次 3):`@vue-flow/*` 与它的传递依赖
            // (`d3-*`、`@vueuse/core`、`vue-demi`)路径里都含 "vue",若不先拦,
            // 会被下面的 vue 分支并进 **首屏预载** 的 vue-vendor —— 等于把整个画布库
            // 塞进首屏,同时击穿 vue-vendor 与首屏的体积预算(见 tools/check-bundle.mjs)。
            // 单列后本块只被执行流程编辑区里的画布异步组件引用,与首屏无关。
            if (
              id.includes('@vue-flow') ||
              id.includes('@vueuse') ||
              id.includes('vue-demi') ||
              /[\\/]d3-[a-z]/.test(id)
            ) {
              return 'flow-vendor';
            }
            if (id.includes('markdown-it') || id.includes('sanitize-html')) return 'content-rendering';
            if (id.includes('vue') || id.includes('pinia')) return 'vue-vendor';
            return 'vendor';
          }
          // 应用代码不设 manualChunks(2026-09-17 P-8 修复):改用 Rollup 默认分块。
          // 原实现在此把 SettingsHub/SettingsModal 等弹窗组件强制归组。但每个弹窗组件都
          // 静态 import 了 store.ts(顶层实例化全部子 store)/api/composable,这些模块
          // **同时被首屏入口引用**;强制归组把它们一并拖进 modal-* chunk,于是 index.js
          // 出现对 modal-* 的静态边 → index.html 发出 modulepreload → 首屏白拉弹窗实现。
          // 实测:仅删 SettingsHub/SettingsModal 两条会把耦合原样搬到 modal-worldbooks
          // (25.9 KB → 191 KB,比原 modal-settings 更大),故此处归组必须整体去掉。
          // 去掉后 Rollup 把入口共享模块并入 index chunk,弹窗实现留在各自的异步 chunk,
          // modals.ts 的动态 import() 懒加载语义天然保持(首屏预载实测已无 modal-*)。
          return undefined;
        }
      }
    }
  }
});
