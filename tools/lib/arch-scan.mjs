// `check-arch.mjs` 的纯扫描函数。独立成模块的理由:这些判定此前埋在 1,100 行的
// 顺序执行脚本里,只能靠「改真实仓库再跑门禁」验证;抽出来后可由
// `tools/check-arch.test.mjs` 用 fixture 直接覆盖(v-model / 动态 import 等假阴性
// 曾真实出现过,见 C-2-0)。
//
// 纪律:本模块只做字符串/路径解析,不读文件、不访问仓库、不打印——
// 保持「输入源码文本 → 输出命中」的可测试形状。
import { dirname, join, sep } from 'node:path';

/** 找出组件内 store 绑定名:const X = useYStore() / const X = store(门面) /
 *  const { a, b: c } = storeToRefs(store)(解构后的 ref 也是同一响应式源)。 */
export function storeBindings(src) {
  const names = new Map();
  for (const m of src.matchAll(/(?:const|let)\s+([A-Za-z_$][\w$]*)\s*=\s*use[A-Z][\w]*Store\(\)/g)) {
    names.set(m[1], 'store');
  }
  // 门面 store.ts 的 useAppStore() 也匹配上面;补 `= store` 形态(别名导入)
  for (const m of src.matchAll(/(?:const|let)\s+([A-Za-z_$][\w$]*)\s*=\s*store\b/g)) {
    names.set(m[1], 'facade');
  }
  // storeToRefs 解构:const { taskRunMode, taskFlowId: flowId } = storeToRefs(store)
  // v-model / ref.value = 都会写回同一 store state,旧扫描完全看不见。
  for (const m of src.matchAll(/(?:const|let)\s*\{([^}]*)\}\s*=\s*storeToRefs\s*\(/g)) {
    for (const raw of m[1].split(',')) {
      const part = raw.trim();
      if (!part) continue;
      const [orig, alias] = part.split(':').map((s) => s.trim());
      const local = alias || orig;
      if (/^[A-Za-z_$][\w$]*$/.test(local)) names.set(local, 'ref');
    }
  }
  return names;
}

/** 找出 store state 的写入形态:
 *  - `<store>.<field> =` / `<ref>.value =`
 *  - `v-model="<store>.<field>"` / `v-model="<ref>"`(Vue 会写回 .value)
 *  排除 == / === / 箭头 / 对象字面量键。 */
export function directWrites(src, bindings) {
  const hits = [];
  for (const [bind, kind] of bindings) {
    if (kind === 'ref') {
      const re = new RegExp(`\\b${bind}\\.value\\s*(?:=[^=]|\\+=|-=|\\*=|\\|=)`, 'g');
      for (const m of src.matchAll(re)) {
        hits.push({ field: bind, kind, index: m.index, syntax: `${bind}.value =` });
      }
    } else {
      const re = new RegExp(`\\b${bind}\\.([A-Za-z_$][\\w$]*)\\s*(?:=[^=]|\\+=|-=|\\*=|\\|=)`, 'g');
      for (const m of src.matchAll(re)) {
        hits.push({ field: m[1], kind, index: m.index, syntax: `${bind}.${m[1]} =` });
      }
    }
  }
  // v-model 没有显式赋值号,旧扫描漏检;v-model 可带 .modifier / :arg。
  for (const [bind, kind] of bindings) {
    if (kind === 'ref') {
      const re = new RegExp(`v-model(?:\\.[\\w-]+)*(?::[\\w-]+)?=["']${bind}["']`, 'g');
      for (const m of src.matchAll(re)) {
        hits.push({ field: bind, kind, index: m.index, syntax: `${m[0]}` });
      }
    } else {
      const re = new RegExp(
        `v-model(?:\\.[\\w-]+)*(?::[\\w-]+)?=["']${bind}\\.([A-Za-z_$][\\w$]*)["']`,
        'g',
      );
      for (const m of src.matchAll(re)) {
        hits.push({ field: m[1], kind, index: m.index, syntax: m[0] });
      }
    }
  }
  return hits;
}

/** 行内/上一行的 `arch-allow: 理由` 显式豁免(必须写理由,空理由不算)。 */
export function hasArchAllow(src, index) {
  const lineStart = src.lastIndexOf('\n', index - 1) + 1;
  const lineEnd = src.indexOf('\n', index);
  const current = src.slice(lineStart, lineEnd === -1 ? src.length : lineEnd);
  const prevStart = src.lastIndexOf('\n', lineStart - 2) + 1;
  const prev = src.slice(prevStart, Math.max(prevStart, lineStart - 1));
  return /arch-allow\s*:\s*\S/.test(current) || /arch-allow\s*:\s*\S/.test(prev);
}

/** 文件级豁免:设置表单这类整文件都是「草稿 ref + queueSettingsSave」的场景,
 *  逐字段加 action 只会增加转发层。必须写理由,且只认文件头 20 行。 */
export function hasFileArchAllow(src) {
  // 理由必须与冒号在同一行:旧正则 `\s*:\s*\S` 的 `\s` 可跨行,
  // 会把「arch-allow-file:」下一行的任意代码误当成理由(fixture 自测抓出)。
  return /arch-allow-file[ \t]*:[ \t]*\S/.test(src.split('\n', 20).join('\n'));
}

/** 采集前端模块说明符:静态 `from '…'` 与动态 `import('…')` 走同一入口。
 *  动态 import 与静态 from 一样会跨代;旧扫描只认 from,utils→components 的
 *  懒加载边因此完全不可见(2026-10-24 审计实测)。顺序保持「先静态后动态」,
 *  与旧实现的两轮循环一致。 */
export function collectFrontendModuleSpecs(src) {
  const specs = [];
  for (const m of src.matchAll(/from\s+['"]([^'"]+)['"]/g)) specs.push(m[1]);
  for (const m of src.matchAll(/import\s*\(\s*['"]([^'"]+)['"]\s*\)/g)) specs.push(m[1]);
  return specs;
}

/** 把说明符解析为 `web/src` 下的首段模块名(保留扩展名,由调用方决定是否剥离)。
 *  - `@/x` → x 的首段
 *  - `./x` → 相对当前文件目录的首段(根文件仍为根文件)
 *  - `../x` 等上跳路径 → null(不视作同层依赖)
 *  - 非相对/非别名说明符 → null
 *  返回 null 表示「不参与跨代边采集」,不是错误。 */
export function resolveFrontendSpecifier(relPath, spec) {
  let to = null;
  if (spec.startsWith('@/')) to = spec.slice(2).split('/')[0];
  else if (spec.startsWith('.')) {
    const joined = join(dirname(relPath), spec).split(sep).join('/');
    const seg = joined.split('/')[0];
    // ./x 在根目录下解析为根文件 x;../x 等上跳路径不视作同层依赖
    to = seg === '.' || seg === '..' ? null : seg;
  }
  return to;
}
