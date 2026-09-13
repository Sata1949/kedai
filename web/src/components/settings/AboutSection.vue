<script setup lang="ts">
// 设置区:关于 + 文字教程。
// 关于文案由项目方提供(版本/作者/代传/协议/更新内容),原样呈现;
// 教程为面向新用户的功能速览,与实现保持同步(改功能时同步此文件)。
// 与 UiSection 同级的纯展示分区,无业务状态依赖。
import { onMounted, ref } from 'vue';
import { health } from '../../api/health';

const props = withDefaults(defineProps<{
  /** 是否显示(embedded 模式按 activeSection 切换;standalone 恒 true) */
  show?: boolean;
}>(), {
  show: true,
});

/**
 * 版本号:优先取后端编译期注入的真实版本(`/api/health` 的 `version`),
 * 取不到时回退下面的常量。这样关于页显示的就是**实际运行版本**,不会再像 0.2.1 那样
 * 发版后忘记手改而写死旧号(本次 0.3.0 即为此)。
 * 常量同时是离线/后端不可达时的兜底显示值,发版时可随 bump 一起改。
 */
const FALLBACK_VERSION = '0.3.0-A-beta';
const version = ref(FALLBACK_VERSION);
onMounted(async () => {
  try {
    const info = await health();
    if (info.version) version.value = info.version;
  } catch {
    // 后端不可达:保留兜底版本号,不影响关于页展示
  }
});

/** 更新内容:与项目方给定的文案逐条一致(本版依据 docs/0.3.0-A-beta-变更说明.md) */
const changelog: string[] = [
  '新增命令执行能力:Agent 可调用命令完成任务(默认关闭;删除、提权等危险命令逐条确认,不享受任何授权豁免)',
  'Android 端完成适配:新增 ROOT / Shizuku / 沙箱三档执行层,并修复手机端设置面板窄屏溢出',
  '任务模式支持命令执行:可运行 ls / echo 等普通命令;删除、提权等危险命令仍拒绝执行',
  '修复角色卡生成长文本被截断导致的「丢格式」:提高卡片生成上限,截断时自动加倍重试,失败也返回可读内容而非整体报错',
  '修复换机或重装后保存设置可能清空已存 API Key 的问题(现在保留原密钥并提示重新填写)',
  '修复极少数情况下生成中断后,该会话一直提示「正在生成中」的问题',
  '修复生成过程中保存设置会卡住的问题;补齐会话与任务列表索引,数据较多时更流畅',
  '提升稳定性与安全性:世界书条目数随编辑实时更新,升级存在漏洞的文本清洗组件,新增依赖漏洞与双端版本一致性自动检查',
];
</script>

<template>
  <div v-show="props.show" class="sv-field">
    <div class="sv-field-label"><span class="sv-supreme blue" /> 关于</div>

    <div class="sv-data-row about-block">
      <div class="info">
        <!-- 版本号由 /api/health 动态读取,其值本身已含预发布后缀(如 0.3.0-beta),
             故此处不再硬写 "beta",否则会显示成「0.3.0-beta beta」。 -->
        <b>kedai {{ version }} Android</b>
        <span>作者:十七凌云(Sata1949)</span>
        <span>代传:TK(SpaceRelay)</span>
        <span>遵循 MIT 协议</span>
        <span>感谢您在百忙之中支持此软件。</span>
      </div>
    </div>

    <div class="sv-field-label sub">更新内容</div>
    <div class="sv-data-row about-block">
      <div class="info">
        <span v-for="(item, i) in changelog" :key="i" class="about-line">
          <i class="about-num">{{ i + 1 }}.</i>{{ item }}
        </span>
      </div>
    </div>

    <div class="sv-field-label sub">使用教程</div>

    <div class="sv-data-row about-block">
      <div class="info">
        <b>角色扮演模式</b>
        <span>在左侧角色列表选择角色卡 → 顶栏「开场」可切换多条开场白。</span>
        <span>若卡片界面只显示文字、没有样式:打开顶栏「HTML」开关(按角色卡记忆)。</span>
        <span>若界面能显示但下拉、按钮点了没反应:开启顶栏「JS」授权(会先弹出风险确认,仅对当前角色与当前脚本版本生效)。</span>
        <span>多个「开场」= 多条开场白,切换会清空当前会话并重新开始。</span>
      </div>
    </div>

    <div class="sv-data-row about-block">
      <div class="info">
        <b>任务模式</b>
        <span>顶栏切换到「任务」,输入目标即可;支持 solo / multi / plan / team / custom 等多种执行模式(见「执行流程」)。</span>
        <span>复杂目标建议用 plan 或 team:plan 先出计划、确认后执行;team 会分派子目标并做审计。</span>
        <span>执行过程与工具调用可在右侧 Agent 面板查看。</span>
      </div>
    </div>

    <div class="sv-data-row about-block">
      <div class="info">
        <b>记忆与世界书</b>
        <span>世界书条目按关键词自动激发;角色卡内嵌世界书随卡导入。</span>
        <span>「向量化模型」开启后可做语义召回,记忆库面板可查看与蒸馏长期记忆。</span>
      </div>
    </div>

    <div class="sv-data-row about-block">
      <div class="info">
        <b>提示词与授权</b>
        <span>「Agent 设置」可编辑角色扮演与任务两套提示词;「提示词注入」配置常驻楼层。</span>
        <span>「授权」三档(严格 / 宽松 / 放行)决定工具执行前的确认策略,涉及系统路径写删恒需授权。</span>
      </div>
    </div>

    <div class="sv-data-row about-block">
      <div class="info">
        <b>快捷键与输入</b>
        <span>Enter 发送,Shift+Enter 换行;输入「/」触发命令联想。</span>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* 关于页排版修复(2026-09-13):
   全局 `.sv-data-row .info span` 只设了颜色与字号,**没有 display:block**——其它设置区的
   `.info` 每个只放一个 span 所以看不出问题,而本页每段有多行说明,行内元素会全部挤成
   一行(`<b>` 因全局有 display:block 才独占一行,更显错乱)。
   同时全局 `.sv-data-row` 是 `align-items:center`,多行文本会垂直居中显得上不齐。
   这里用 scoped 局部修正:块级堆叠 + 顶部对齐 + 行距/间距,不改全局样式避免影响其它分区。 */
.about-block {
  align-items: flex-start;
}
.about-block .info {
  display: flex;
  flex-direction: column;
  gap: 6px;
  line-height: 1.55;
}
.about-block .info b {
  margin-bottom: 2px;
}
/* 更新内容每行带序号:序号固定宽度对齐,正文可换行 */
.about-line {
  display: flex;
  gap: 6px;
}
.about-num {
  flex: none;
  font-variant-numeric: tabular-nums;
  opacity: 0.85;
}
</style>
