// 测试专用最小类型声明(2026-10-02 UIP-10)。本仓不安装 @types/node(功能源码不需要),
// 而样式契约类测试必须读取 .css 源码(vitest `css:false` 下 `?raw` 对 .css 返回空串)。
// 这里只给用到的 node:fs API 最小声明:新增此类测试直接 `import { readFileSync } from 'node:fs'`,
// 不必再写显式压错注释(该计数已因此下调,见 tools/check-frontend-lint.mjs)。
declare module 'node:fs' {
  export function readFileSync(path: string | URL, encoding: 'utf8'): string;
}
