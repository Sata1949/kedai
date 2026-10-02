<script setup lang="ts">
// 设置区:关于 + 文字教程。
// 关于文案由项目方提供(版本/作者/协议/更新内容),原样呈现;
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
const FALLBACK_VERSION = '0.4.1-alpha';
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
 * 本轮依据(2026-10-01 0.4.1-alpha 批次):docs/功能-变更史.md 中 2026-09-27
 * 「0.4.0-alpha 发布」之后起至 2026-10-01 各章,
 * 逐条对应已入库的实现与修复,不写未落地项。
 */
const changelog: string[] = [
  '新手引导上线:首次启动分步介绍常用功能,支持「带我去设置」直达对应分区;关于页可随时重新打开',
  '编码工作区全流程:建任务可选工作区目录并即时探测画像;任务详情可查文件变更(单文件看 diff / 回滚、整任务全部回滚、导出 patch)',
  '编码能力包增强:内置编码流程预设开包即可一键选用;新增结构化补丁工具 fs_patch;执行者模板自动点名项目约定文件',
  '编码执行加固:取消任务能真正中止正在执行的命令(含 Windows 子进程清理);单条命令上限 30 分钟且输出保留头尾;长对话压缩后仍保留编码工作状态',
  '任务模式修复:空回自动重试与收尾定向提醒(截断自愈);输出预算下限抬到 4096,避免推理 token 耗尽预算;规划器新增工作区侦察快照',
  '内置画布示范流程「多路调研示范流程」:多路并行调研 + 反思汇合 + 严格终稿,画布视图可见二维分支结构',
  '接口三方言:连接配置支持 OpenAI / Responses / Anthropic 三种接口格式,Base URL 端点后缀自动剥离',
  '界面与排版修复:代码块长行换行、深底标签对比度、编辑框自适应,以及设置导航、连接卡片、模型下拉的观感问题',
  '安全修复:升级 markdown-it 与 rustls,修复两处上游安全漏洞',
  '对外发布:产品定名 KedaiAgent,补齐 MIT 许可证文件(LICENSE),代码仓库转公开',
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
        <span>绑定工作区后:建任务时即可看到工作区画像;任务详情可查文件变更——单文件看 diff / 回滚、整任务全部回滚、导出 patch。</span>
      </div>
    </div>

    <div class="sv-data-row about-block">
      <div class="info">
        <b>自定义流程</b>
        <span>在设置区「Agent 执行流程」编排流程:列表视图默认,画布视图可视化编辑(节点 = 一次模型调用,连线 = 先后依赖)。</span>
        <span>任务模式选「自定义流程」后可绑定某份流程(绑定即冻结,之后改流程不影响该任务);流程模式可选「对比」,把名单内流程作为工具交给模型自主调用。</span>
        <span>节点可单独配置连接、上下文上限、工具轮次、超时/重试与严格/宽松档位;流程可整体导出/导入。</span>
        <span>开启编码能力包后,内置流程库会多出编码流程预设,可在任务里一键选用。</span>
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
  gap: var(--space-1-5);
  line-height: 1.55;
}
.about-block .info b {
  margin-bottom: 2px;
}
/* 更新内容每行带序号:序号固定宽度对齐,正文可换行 */
.about-line {
  display: flex;
  gap: var(--space-1-5);
}
.about-num {
  flex: none;
  font-variant-numeric: tabular-nums;
  opacity: 0.85;
}
</style>
