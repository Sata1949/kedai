<script setup lang="ts">
// 设置区:关于 + 文字教程。
// 关于文案由项目方提供(版本/作者/代传/协议/更新内容),原样呈现;
// 教程为面向新用户的功能速览,与实现保持同步(改功能时同步此文件)。
// 与 UiSection 同级的纯展示分区,无业务状态依赖。
import { onMounted, ref } from 'vue';
import { useAppStore } from '../../store';
import { health } from '../../api/health';

const store = useAppStore();

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
 * 常量同时是离线/后端不可达时的兜底显示值,发版时由 tools/bump-version.ps1 一并改写,
 * 并由 build.ps1 的 Assert-VersionConsistency 作为第 8 处声明校验(改漏即构建报错)。
 */
const FALLBACK_VERSION = '0.4.0-alpha';
const version = ref(FALLBACK_VERSION);
onMounted(async () => {
  try {
    const info = await health();
    if (info.version) version.value = info.version;
  } catch {
    // 后端不可达:保留兜底版本号,不影响关于页展示
  }
});

/**
 * 更新内容:与项目方给定的文案逐条一致。
 * 本轮依据(2026-09-27 0.4.0-alpha 批次):docs/功能-变更史.md 中 2026-09-17
 * 「二维自定义任务流批次 1/2」起至 2026-09-27「0.4.0-alpha 发布」各章,
 * 逐条对应已入库的实现与修复,不写未落地项。
 */
const changelog: string[] = [
  '自定义流程全面落地:设置区「Agent 执行流程」支持列表与画布两种编辑方式,节点、连线与静态子图齐备,流程可保存复用',
  '流程与任务贯通:任务可绑定流程(绑定即冻结,后续改流程不影响该任务),并可开「对比」模式把名单内流程作为工具交给模型自主调用',
  '流程可搬运:批量导出(含依赖闭包)+ 原子导入合并,支持覆盖/合并两种模式,导入失败不留半成品',
  '节点级精细控制:每个节点可单独配置连接、工具轮次上限、上下文上限、超时与重试、严格/宽松档位',
  '多套 API 连接:可保存多套连接配置(凭据加密存储),任务与节点可分别指定使用哪一套',
  '任务更稳:长任务新增步骤墙钟预算与语义熔断、空闲自愈;被取消或超时也会兜底保留已完成的部分成果,不再整单丢失',
  '编码类任务:任务可绑定工作区,新增文件读写/搜索工具族(fs_*),路径越界由闸门拦截',
  '缺陷修复:修复任务执行时乱弹 cmd 窗口、长时间不返回,以及 Android 端命令执行桥接缺陷(JNI 类缓存未登记 + 方法签名不符)',
  '工程质量:巨型源文件拆分(无行为变更);Rust 工具链口径收敛到单一出处;CI 新增 Linux 可移植性档;文档计数类硬口径纳入机检',
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

    <!-- 新手引导入口(2026-09-27 教程引导批次):首启会自己弹一次,这里供随时重看。
         下面的六段速览是纯文字版本,与引导里的步骤同源(改功能时两处一起改)。 -->
    <div class="sv-data-row about-block">
      <div class="info">
        <b>新手引导</b>
        <span>首次启动时会依次问「偏好模式」与「要不要讲解」，再分步介绍常用功能；讲解里的「带我去设置」会打开综合设置并定位到对应分区。</span>
      </div>
      <button class="sv-btn ghost shrink-0 whitespace-nowrap" @click="store.onboardingOpen = true">重新打开新手引导</button>
    </div>

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
        <span>编码类目标可给任务绑定工作区(项目目录):任务用文件读写/搜索工具在目录内工作,越界路径会被拦截。</span>
      </div>
    </div>

    <div class="sv-data-row about-block">
      <div class="info">
        <b>自定义流程</b>
        <span>在设置区「Agent 执行流程」编排流程:列表视图默认,画布视图可视化编辑(节点 = 一次模型调用,连线 = 先后依赖)。</span>
        <span>任务模式选「自定义流程」后可绑定某份流程(绑定即冻结,之后改流程不影响该任务);流程模式可选「对比」,把名单内流程作为工具交给模型自主调用。</span>
        <span>节点可单独配置连接、上下文上限、工具轮次、超时/重试与严格/宽松档位;流程可整体导出/导入。</span>
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
