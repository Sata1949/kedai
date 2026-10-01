<script setup lang="ts">
// 任务**逐任务选用连接**选择器(B 批 B1):Sidebar「下达目标」区使用。
//
// 为什么独立成组件:与 TaskModeSelect / TaskFlowSelect 同款先例——Sidebar 引用的
// /logo.png 在测试环境下无法解析,抽出来后选择器可单独挂载测试;选择持久化在 task store
// (taskConnectionId,localStorage),刷新后保持。
//
// 语义(后端为准,这里只做呈现与即时提示):
//  - 选择 = 该任务**所有** LLM 调用的缺省连接(节点级 `connection_id` 优先);
//  - **与执行模式无关**:它绑的是 provider 而不是编排,故 legacy 到 custom 六模式都可见
//    (TaskFlowSelect 只在 custom 下出现,两者可见性判据不同,不是漏加门控);
//  - 缺省(空)= 跟随设置的默认连接,与 B 批之前逐字节一致;非空才随创建请求下发;
//  - 连接列表只取**已落库**的设置(复用 useFlowConnections,与流程节点选择器同一份口径);
//  - **引用失效口径**:任务是本机的——持久化的 id 不在当前连接列表里即重置为「默认连接」
//    并给一行**显式**提示(不静默)。这与流程节点编辑器的「保位 + 运行期报错」刻意不同:
//    流程可跨机搬运(引用指向别的机器),任务是本机对象,创建期后端就会 400,
//    所以在这里就地把死引用清掉比让用户撞一次失败更友好。
import { computed, onMounted, ref, watch } from 'vue';
import { storeToRefs } from 'pinia';
import { useAppStore } from '../store';
import { useFlowConnections } from '../composables/useFlowConnections';

const store = useAppStore();
const { taskConnectionId, settingsOpen } = storeToRefs(store);

/** 选择器恒可见(与 task_mode 无关),故 show 传一个恒真值把加载时机交给 onMounted */
const conn = useFlowConnections(computed(() => true));

/** 失效重置提示(空串 = 无提示);不持久化,刷新后自然消失 */
const resetNotice = ref('');

/** 当前引用在连接列表里的命中项(空串 = 跟随默认连接) */
const selected = computed(() =>
  taskConnectionId.value ? conn.options.value.find((o) => o.id === taskConnectionId.value) : undefined,
);

/** 引用存在但已停用:创建期后端会 400,故即时提示(不重置——连接还在,用户可能只是忘了启用) */
const disabledNotice = computed(() =>
  selected.value && !selected.value.enabled
    ? `所选连接「${selected.value.name}」已停用，创建任务会被拒绝，请启用它或改选其它连接。`
    : '',
);

/**
 * 拉连接列表并校验持久化的引用。**只在拉取成功时**判定失效——
 * 拉取失败(服务端异常/离线)不等于引用失效,此时保留原选择,避免静默改绑。
 * 提示先清后置:它描述的是**本次**校验结果,不该在用户改选后仍挂着
 * (另见下方对 taskConnectionId 的清理,两者覆盖「重拉」与「手动改选」两条路径)。
 */
async function loadAndValidate(): Promise<void> {
  await conn.load();
  if (conn.error.value) return;
  resetNotice.value = '';
  const id = taskConnectionId.value;
  if (id && !conn.options.value.some((o) => o.id === id)) {
    taskConnectionId.value = '';
    resetNotice.value = '原选择的连接已不存在，已重置为默认连接。';
  }
}

/**
 * 用户手动改选后撤下重置提示(只在选中了具体连接时撤:
 * 重置本身会把值写成空串,若不分条件就会把刚给出的提示立刻抹掉)。
 */
watch(taskConnectionId, (id) => {
  if (id) resetNotice.value = '';
});

onMounted(() => {
  void loadAndValidate();
});

/**
 * 设置面板关闭时重拉一次:用户刚在「连接配置」里增删连接,回到侧栏的选择器不该
 * 还列着已删除的连接(与 useFlowConnections 的「先改连接再切回流程不吃陈旧值」
 * 同一口径;本组件常驻侧栏,没有 show 的 false→true 可用,改用设置面板的关闭沿)。
 */
watch(settingsOpen, (open, prev) => {
  if (prev && !open) void loadAndValidate();
});
</script>

<template>
  <div>
    <div class="sv-inp-row">
      <select
        v-model="taskConnectionId"
        class="sv-select"
        title="本任务所有模型调用走哪一套 API 连接(节点级连接优先);默认 = 跟随「任务模式默认连接」,该连接也未配置时用默认连接。创建任务时后端会校验所选连接存在且已启用"
      >
        <option value="">连接:默认（跟随任务模式默认连接）</option>
        <option v-for="c in conn.options.value" :key="c.id" :value="c.id" :disabled="!c.enabled">
          连接:{{ c.name }}{{ c.model ? `｜${c.model}` : '' }}{{ c.enabled ? '' : '(已停用)' }}
        </option>
      </select>
    </div>
    <!-- 失效重置与停用两条提示互斥:重置已把引用清空,停用提示此时不再成立 -->
    <p v-if="resetNotice" class="sv-note flow-tool-warn">{{ resetNotice }}</p>
    <p v-else-if="disabledNotice" class="sv-note flow-tool-warn">{{ disabledNotice }}</p>
    <p v-if="conn.error.value" class="sv-note flow-tool-warn">{{ conn.error.value }}</p>
  </div>
</template>
