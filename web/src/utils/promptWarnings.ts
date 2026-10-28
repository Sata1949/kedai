// 提示词预览告警码 → 中文提示(单一出处)。
// 后端 `GET /api/settings/prompt-preview` 只回结构化码(告警本体是码,不是文案),
// 展示位有两处(AgentSettingsSection 的「最终提示词预览」与 OptimizeModal 的「提示词检查」),
// 文案在本模块收敛,避免两处组件各写一份而分叉——与仓内「文案单一出处」惯例一致。

/** 已知码的中文提示。用 Map 而非对象字面量:未登记码走回退分支时不会被原型链键名污染。 */
const PROMPT_WARNING_TEXT = new Map<string, string>([
  [
    'template_missing_world_info',
    '当前自定义系统提示词模板未写 {{world_info}} 占位符：角色卡的常驻世界书条目不会注入' +
      '(引擎状态块除外)。这是「Custom Prompt 完整替换内置模板」的既有契约,不是错误;' +
      '需要常驻设定生效请在模板中加上该占位符。',
  ],
  [
    'perspective_unknown',
    '当前「视角」取值无法识别(只支持第一人称 / 第二人称 / 第三人称)：' +
      '本轮会按默认第三人称限知锚定;请重新选择后再保存。',
  ],
]);

/** 告警码的展示文案;未登记码原样透出(后端新增码不至于在前端静默丢信息)。 */
export function promptWarningText(code: string): string {
  return PROMPT_WARNING_TEXT.get(code) ?? code;
}
