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
const FALLBACK_VERSION = '0.4.2';
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
 * 本轮依据(2026-10-12 0.4.2 批次):docs/功能-变更史.md 中 2026-10-07「0.4.1 正式版发布」之后
 * 起至 2026-10-11 各章(STIMP / PAD / 悬浮窗移植 / RPFLOW / IMPHUB / TUTSYNC / 世界书遵循修复批),
 * 逐条对应已入库的实现与修复,不写未落地项;仓外素材适配(PAD 的预设包)不列为产品能力。
 * 第 9 条 SENDFIX 随 0.4.1 修复重建(`edf62cb`)已发布,但当时该列表未同步,故在此补入。
 */
const changelog: string[] = [
  '综合导入 / 导出上线:一个文件可同时导入预设条目、执行流程、提示词、技能、预设正则等多段内容,逐段勾选、逐段执行,失败互不影响并给出导入回执;导出把当前资产打成一个综合导出件(不含 API 连接与密钥)',
  '附件管理上线:技能 / 流程 / 插件 / 预设正则 收进一个面板的四个页签,支持虚拟文件夹分组与移动;原先分散的「技能库」「插件」独立窗口和设置区里的「预设导入」已并入,不再有两处入口',
  '全局正则脚本库:正则不再只能随角色卡,可全局维护并作用于所有角色;与卡级脚本按「全局在前、卡级在后」合并',
  '深度 / Agent 生成流程四步化:草稿 → 正文 → 批判与修改 → 归档与词条同步;反思阶段强制定点修改而非整段重写;禁用词改为同义改写(不再机械删词把句子切碎);新增「词条同步」两档开关',
  '酒馆式消息 HTML 渲染:纯文字角色卡的回复现在也按白名单渲染结构标签(折叠块、状态块等),样式与事件属性一律剥除;顶栏「HTML」开关的作用面随之变宽',
  '草稿默认折叠不占屏;快速 / 深度 / Agent 档位选择会被记住,重启后还原',
  '角色卡脚本新增楼层控制接口:带悬浮窗、需要按楼层开关内容的卡现在能在应用内正常工作',
  '新手教程大修:首启引导从 14 步扩到 19 步,补上导入角色卡、综合导入 / 导出、附件管理、MCP 与能力包、发送失败处理;讲解里的「带我去设置」现在能直接打开独立面板;指向已删除功能的旧指引全部清掉',
  '发送失败现在看得见:对话区常驻错误条与「重试」,停止时显示「正在停止…」过渡态,失败不丢已写内容',
  '正确性修复:酒馆预设导入判定修复(自 0.2.0 起部分预设无法导入);复杂模式不再把空白楼层拼成一串空白塞进系统提示词;默认提示词与文学能力包、运行时工具段的三处冲突对齐',
  '关于页「使用教程」与新手教程改为同一份文案,不再各说一套;新增防漂移门禁,界面改名或删除入口时构建期即报警',
  '世界书触发修复:关键词只出现在开场白或角色回复里时也能正确触发条目(此前只扫描你的消息,大量设定因此不生效);命中的设定带统一说明头并整体置于引用边界内,遵循更稳定',
  '世界书面板新增「扫描窗口」独立编辑项:此前面板上的「扫描深度」实际是插入深度,真正的关键词扫描窗口无法在界面调整;两者现已分栏显示,不再混淆',
  '自定义系统提示词模板漏写 {{world_info}} 占位符时,提示词预览会给出可见提示(常驻世界书条目不会注入);常驻说明头改为有内容才出现,不再出现只有强制语气、没有内容的悬空句',
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
         下面的速览**逐句取自教程步骤表**(`utils/tutorialCopy.ts` 的 ABOUT_SECTIONS),
         与分步弹窗同一份文字——2026-10-10 TUTSYNC 前这里是一份手写的平行文案,
         界面改名后两处各自腐烂。改功能时只改步骤表。 -->
    <div class="sv-data-row about-block">
      <div class="info">
        <b>新手引导</b>
        <span>首次启动时会依次问「偏好模式」与「要不要讲解」，再分步介绍常用功能；讲解里的「带我去设置」会打开综合设置并定位到对应分区或独立面板。</span>
      </div>
      <button class="sv-btn ghost shrink-0 whitespace-nowrap" @click="store.onboardingOpen = true">重新打开新手引导</button>
    </div>

    <div v-for="section in ABOUT_SECTIONS" :key="section.heading" class="sv-data-row about-block">
      <div class="info">
        <b>{{ section.heading }}</b>
        <span v-for="(line, i) in aboutSectionLines(section)" :key="i" class="ob-fact">{{ line }}</span>
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
