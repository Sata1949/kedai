<script setup lang="ts">
// 设置区:关于 + 文字教程。
// 关于文案由项目方提供(版本/作者/协议/更新内容),原样呈现;
// 教程速览**不在本文件手写**:逐段逐句由 `utils/tutorialCopy.ts` 的 ABOUT_SECTIONS 派生自
// 教程步骤表,与分步弹窗共用同一份文字(2026-10-10 TUTSYNC OB-11;此前这里是平行手写文案,
// 界面改名后就烂了)。改功能只改步骤表。
// 与 UiSection 同级的纯展示分区,无业务状态依赖。
import { onMounted, ref } from 'vue';
import { useAppStore } from '../../store';
import { health } from '../../api/health';
import { ABOUT_SECTIONS, aboutSectionLines } from '../../utils/tutorialCopy';

const store = useAppStore();

const props = withDefaults(
  defineProps<{
    /** 是否显示(embedded 模式按 activeSection 切换;standalone 恒 true) */
    show?: boolean;
  }>(),
  {
    show: true,
  },
);

/**
 * 版本号:优先取后端编译期注入的真实版本(`/api/health` 的 `version`),
 * 取不到时回退下面的常量。这样关于页显示的就是**实际运行版本**,不会再像 0.2.1 那样
 * 发版后忘记手改而写死旧号(本次 0.3.0 即为此)。
 * 常量同时是离线/后端不可达时的兜底显示值,发版时由 tools/bump-version.ps1 一并改写,
 * 并由 build.ps1 的 Assert-VersionConsistency 作为第 8 处声明校验(改漏即构建报错)。
 */
const FALLBACK_VERSION = '0.4.5';
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
 * 本轮依据(2026-11-05 0.4.5 批次):docs/功能-变更史/ 中 2026-10-12「0.4.2 升版」之后
 * 至 2026-11-04 各章(ANDUI / REVFIX / UNDOFIX / UIQ-4 / GLASS / WBCONC / TMR / SKY /
 * TPROF / POV / CARD / USERPERSONA / DB-7 / UIPOL / UIVIS / ATTACH / TMSHOW / ATTFIX 等),
 * 逐条对应已入库的实现与修复,不写未落地项;纯内部工程改造(构建提速、CI、测试夹具、
 * 文档拆分)不列为用户可见内容。0.4.3 / 0.4.4 未发版,本列表是 0.4.2 之后的累计更新。
 */
const changelog: string[] = [
  '任务附件:任务模式左下角改为「导入附件」,任意文件可多选交给任务(单任务最多 10 个、单文件 25MB);附件落在任务工作目录内、目标里列出相对路径,模型按需读取;未创建任务时先进入「将随任务提交的附件」清单,可逐个移除或清空;任务详情新增「任务附件」区,可逐个删除',
  '任务执行过程三档显示:任务详情顶栏可在「步骤 / 全消息 / 压缩」之间切换——步骤只看计划时间线,全消息逐条列出思考、工具调用与结果,压缩按阶段分组折叠并显示分类计数;设置 ›「界面」可设默认档,单个任务的覆盖互不影响;断线或丢帧后轨迹会自动补拉,不再缺一段',
  '任务参数档位:设置 ›「API 与模型」的「任务参数档位」三档(保守 / 任务优先 / 极限),默认任务优先——任务输出上限 65536、工具预算 128K(与任务输入窗口取小,阶段压缩)、子代理 4 层 / 16 并发、步骤墙钟 3600 秒与空闲 2400 秒安全闸、团队总预算默认不限;下拉会跟随你当前的档位,老配置可用「应用档位」一键采用',
  '设置分区合并:原「API 连接」与「模型与生成」合并为「API 与模型」——改完模型和密钥不用再切到另一个二级项找温度与输出上限,任务参数档位也在同一页;旧入口与教程落点自动落到新分区',
  '任务可靠性:并行执行的任务按规划声明文件归属,避免主 Agent 互相覆盖;成果卡新增「交付校验」(必需 / 已生成 / 缺失 / 变更),步骤内容出现占位符或空文件会被标为「部分完成」而不是冒充成功;无文件进展会在第 8 轮提醒;上游失败可重试,团队预算按主 / 步分配',
  '任务交付确定性:新增静态接线检查(未导出的导入、本地引用、单文件外链)与单文件内联工具,总装阶段能把分片合成一个可运行文件;运行期用无头浏览器探针检查页面是否真的能打开、场景是否为空',
  '任务模式提示词去角色扮演化:任务侧工具描述不再出现「本角色」「正文」等角色扮演词汇;legacy 任务续跑时不会再被误告「没有读文件工具」',
  '用户设定:设置 ›「对话与提示词」新增「用户设定」,可配置称呼与用户角色描述;启用后 {{user}} / {{user_name}} 替换为称呼,新增 {{user_description}};仅角色扮演生效,任务模式仍称「用户」',
  '人称锚定:视角选择会真正约束叙述人称、镜头归属与台词人称,并禁止替用户角色代言;生成与反思使用同一套锚定,上文人称漂移会在本轮纠正',
  '角色卡默认词补齐:默认提示词现在注入角色描述、性格、场景与对话示例;兜底模板同步对齐,导入的卡不会再只读到一个 description 字段',
  '世界书并发安全:执行中修改世界书不再静默删掉窗口期新增的条目;显式删除更可靠;角色卡内嵌世界书同步修复;保存到已删除的书会报错而不是假成功',
  '预设与插件导入修复:酒馆预设按 prompt_order 的启用位判定(此前部分预设条目的启用状态与 ST 里相反);插件导入重名会整批拒绝并如实报错,不再留下无人注册的文件',
  '界面终稿与主题:角色扮演「编辑部」空态、任务台两栏空态与状态矩阵 / 进度轨 / 成果卡;彩窗主题真透明、启动预挂载与主题预载、按设备性能自适应档位,切主题不再卡顿;拼接玻璃按石棂 + 菱形拼片重做,双主题观感统一',
  '界面一致性与排版:顶栏开关统一规格、表单标签列对齐、工具栏换行与折叠面板收口、用户消息按 markdown 渲染、标点与主区骨架统一',
  'Android 窄屏修复:任务抽屉可整滚、退出键不再被底导航盖住、MCP 开关与聊天记录标题 / 标签不再溢出、模态底部不再被盖、契约编辑器窄屏竖排',
  '双数据目录提示:检测到另一套数据目录也有用户数据时,应用顶部常驻提示条给出路径与证据,可手动关闭;检测失败不再伪装成「没有分叉」',
  '启动器与启动检测:识别增量构建产物,一次启动最多一个弹窗(启动测试版 / 重建便携版 / 直接启动当前便携版);单实例防止重复弹窗;非英文 Windows 的端口占用解析修复',
  '数据安全与回退:记忆快照精确绑定到真实写入行,恢复失败会保留快照;「回到所选时点」语义修正;文件快照超限前不再读正文;资源代理改为边下边限 5MB',
];
</script>

<template>
  <div v-show="props.show" class="sv-field">
    <div class="sv-field-label"><span class="sv-supreme blue" /> 关于</div>

    <div class="sv-data-row about-block">
      <div class="info">
        <!-- 版本号由 /api/health 动态读取,其值本身已含预发布后缀(如 0.3.0-beta),
             故此处不再硬写 "beta",否则会显示成「0.3.0-beta beta」。
             平台后缀同理不写死:模板里硬写 "Android" 会让 Windows 端也显示成 Android
             (2026-10-07 0.4.1 批次移除该字样),本行只显示版本号。 -->
        <b>kedai {{ version }}</b>
        <span>作者：十七凌云(Sata1949)</span>
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
         下面的速览**逐句取自教程步骤表**(`utils/tutorialCopy.ts` 的 ABOUT_SECTIONS),
         与分步弹窗同一份文字——2026-10-10 TUTSYNC 前这里是一份手写的平行文案,
         界面改名后两处各自腐烂。改功能时只改步骤表。 -->
    <div class="sv-data-row about-block">
      <div class="info">
        <b>新手引导</b>
        <span
          >首次启动时会依次问「偏好模式」与「要不要讲解」，再分步介绍常用功能；讲解里的「带我去设置」会打开综合设置并定位到对应分区或独立面板。</span
        >
      </div>
      <button class="sv-btn ghost shrink-0 whitespace-nowrap" @click="store.onboardingOpen = true">
        重新打开新手引导
      </button>
    </div>

    <div v-for="section in ABOUT_SECTIONS" :key="section.heading" class="sv-data-row about-block">
      <div class="info">
        <b>{{ section.heading }}</b>
        <span v-for="(line, i) in aboutSectionLines(section)" :key="i" class="ob-fact">{{
          line
        }}</span>
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
