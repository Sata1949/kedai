// web/eslint.config.js — 前端代码质量门禁(ESLint 9/10 flat config)。
//
// 分工:本配置只管「代码质量 / 常见错误」(未用变量、可疑写法、Vue 规则等);
// 类型逃逸 ratchet(any / as never / as unknown as / @ts-expect-error / 非空断言)
// 仍由 tools/check-frontend-lint.mjs 负责,两者并存、互不替代。
//
// 规则集刻意克制:目标是在**不改业务代码**的前提下 `eslint .` 零 error 通过。
// 对存量噪音只在下方 rules 里定向放松(设 warn 或关闭并写明理由),
// 不使用大面积 /* eslint-disable */ 注释。
import js from '@eslint/js';
import tseslint from 'typescript-eslint';
import pluginVue from 'eslint-plugin-vue';
import prettier from 'eslint-config-prettier';

export default tseslint.config(
  // 构建产物、依赖、静态资源不参与检查
  { ignores: ['dist/**', 'node_modules/**', 'coverage/**', 'public/**'] },

  // 1) JS 基础推荐规则
  js.configs.recommended,

  // 2) TypeScript 推荐规则(非 type-checked 版)。
  //    理由:type-checked 需 project service 并显著拖慢 lint;而本仓已有
  //    `vue-tsc --noEmit` 作为独立硬门禁(见 tools/check-all.ps1),类型层不缺覆盖。
  ...tseslint.configs.recommended,

  // 3) Vue SFC 推荐规则
  ...pluginVue.configs['flat/recommended'],

  // 4) .vue 的 <script lang="ts"> 交给 @typescript-eslint/parser
  {
    files: ['**/*.vue'],
    languageOptions: {
      parserOptions: {
        parser: tseslint.parser,
        ecmaVersion: 'latest',
        sourceType: 'module',
      },
    },
    rules: {
      // 本仓 .vue 一律为 <script setup lang="ts">。与 typescript-eslint 的
      // eslint-recommended 对 .ts 的处理保持一致:未定义标识符交给 TS/vue-tsc 判定
      // (浏览器/Node 全局无需在此登记),避免误报。
      'no-undef': 'off',
    },
  },

  // 5) 定向规则(克制;仅压存量噪音,不掩盖真实问题)
  {
    rules: {
      // 未用变量降为 warn:测试与解构中的存量未用绑定较多;命名以 _ 开头视为有意忽略。
      '@typescript-eslint/no-unused-vars': [
        'warn',
        {
          argsIgnorePattern: '^_',
          varsIgnorePattern: '^_',
          caughtErrors: 'none',
          ignoreRestSiblings: true,
        },
      ],
      // 单字组件名在本项目是既有约定(App.vue / Sidebar.vue),不与多词命名规范冲突。
      'vue/multi-word-component-names': 'off',

      // ---- 以下为「有意的实现方式」,按**文件范围**关闭并写明理由 ----
      // 声明式:不用行内 disable 注释,理由集中在此处,后续新增文件不会被静默放行。

      // no-useless-assignment / prefer-const / no-useless-escape / no-case-declarations
      // 的存量已于 2026-10-29 FT-2 批全部清偿(见变更史同章),故此处不再降级,
      // 保持 ESLint 默认(推荐规则集里的 error),由 lint 作为零告警门禁的一部分。
    },
  },

  // 5a) v-html:全仓 v-html 的分支值都经统一消毒管线(sanitize-html,见 render.ts
  //     的 sanitizeMessageHtml / cleanScriptHtml),属既定实现而非疏漏。按文件收窄到
  //     「确实渲染已消毒 HTML」的组件,不全局关闭——新组件若直接绑未消毒内容仍会报。
  {
    files: [
      'src/components/ChatMessageItem.vue',
      'src/components/task/MarkdownBlock.vue',
      'src/components/task/TaskApprovalPanel.vue',
      'src/components/task/TaskExecutionPanel.vue',
      'src/components/task/TaskResultPanel.vue',
    ],
    rules: { 'vue/no-v-html': 'off' },
  },

  // 5b) no-control-regex:render.ts 用 \u0000 作**脚本块的占位符定界符**
  //     (renderBlocks 把脚本替换成 \0KDAI_BLOCK_n\0,再按该正则拆段并剥离残留)。
  //     NUL 不会出现在正常正文里,故选它作定界符;这三个正则属有意实现。
  {
    files: ['src/render.ts'],
    rules: { 'no-control-regex': 'off' },
  },

  // 5c) no-fallthrough:两处 case 贯穿都是**刻意的合并分支**——
  //     taskStatus.ts 的 'pending' 贯穿到 'planned'(同走 pending 灰);
  //     stores/task.ts 的 status/plan/subtask/flow_bound 共用同一刷新路径。
  //     两处均已用注释说明,故按文件关闭。
  {
    files: ['src/taskStatus.ts', 'src/stores/task.ts'],
    rules: { 'no-fallthrough': 'off' },
  },

  // 5d) vue/no-side-effects-in-computed-properties:DevToolsModal 的 `shown` 计算属性
  //     在返回前把 expandedTs 复位为 null(「新增事件默认折叠」)。这是开发者工具面板的
  //     展示态重置,不参与响应式依赖,且该面板不承载业务状态;按文件关闭。
  {
    files: ['src/components/DevToolsModal.vue'],
    rules: { 'vue/no-side-effects-in-computed-properties': 'off' },
  },

  // 5e) @typescript-eslint/no-unused-vars:api/types/settings.ts 的三个 `SETTINGS_*_FIELDS`
  //     是**类型推导用的常量**(紧邻的 `type X = (typeof SETTINGS_..._FIELDS)[number]` 消费它们),
  //     而 ESLint 的非 type-aware 规则看不到类型位置的使用。按文件关闭该规则,不写
  //     行内 disable(计划 FT-2 明令「不得在源文件加 eslint-disable」)。
  {
    files: ['src/api/types/settings.ts'],
    rules: { '@typescript-eslint/no-unused-vars': 'off' },
  },

  // 6) 关闭与 Prettier 冲突的格式规则(必须放在最后)
  prettier,
);
