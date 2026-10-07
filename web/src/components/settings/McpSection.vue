<script setup lang="ts">
// 设置区:MCP 服务(批次 6.2,L3 隔离;默认关;PLGM 3.2 加管理面)。
// 总开关 mcp_enabled + 服务器列表(name/command/args 空格分隔/启用 toggle/删除)+ 新增行表单。
// 状态徽章与「重连/停止/启动」走 GET /api/mcp/servers 与 POST /api/mcp/servers/{name}/{restart|stop|start}
// (restart 读当前扁平设置快照——改配置后点「重连」即生效,不必重启应用)。
// 设置读写走 settings 既有通道(store.saveSettings patch),参照 GenParamsSection 模式。
import { onMounted, ref } from 'vue';
import { useAppStore } from '../../store';
import { storeToRefs } from 'pinia';
import type { McpServerConfig } from '../../api/types';
import * as mcpApi from '../../api/mcp';

withDefaults(defineProps<{
  /** 是否显示(embedded 模式按 activeSection 切换;standalone 恒 true) */
  show?: boolean;
}>(), {
  show: true,
});

const store = useAppStore();
const { mcpEnabled, mcpServers } = storeToRefs(store);

// ===== 运行状态(PLGM 3.2;辅助信息,拉取失败静默保留旧值,不阻塞设置编辑) =====
const statuses = ref<Record<string, mcpApi.McpServerStatus>>({});
const acting = ref<string | null>(null);

async function loadStatus(): Promise<void> {
  try {
    const r = await mcpApi.listMcpServers();
    const map: Record<string, mcpApi.McpServerStatus> = {};
    for (const s of r.servers) map[s.name] = s;
    statuses.value = map;
  } catch {
    // 状态只影响徽章展示;后端不可达/未启用时保持旧值
  }
}
function statusOf(name: string): mcpApi.McpServerStatus | undefined {
  return statuses.value[name];
}
const STATE_LABEL: Record<string, string> = {
  running: '运行中',
  failed: '失败',
  stopped: '已停止',
  disabled: '已禁用',
};
function stateLabel(state?: string): string {
  return (state && STATE_LABEL[state]) || '未装配';
}
function stateClass(state?: string): string {
  return state === 'running' ? 'done' : state === 'failed' ? 'fail' : state === 'stopped' ? 'pending' : '';
}

/** 重连 / 停止 / 启动(动作后刷新状态;失败展示原因) */
async function act(name: string, action: 'restart' | 'stop' | 'start'): Promise<void> {
  if (acting.value) return;
  acting.value = name;
  saveMsg.value = '';
  try {
    const fn =
      action === 'restart'
        ? mcpApi.restartMcpServer
        : action === 'stop'
          ? mcpApi.stopMcpServer
          : mcpApi.startMcpServer;
    await fn(name);
    await loadStatus();
  } catch (e) {
    saveMsg.value = `操作失败:${(e as Error).message}`;
  } finally {
    acting.value = null;
  }
}

// ===== 新增行表单(本地草稿,添加后清空) =====
const newName = ref('');
const newCommand = ref('');
const newArgs = ref('');

/** args 空格分隔文本 ↔ 数组(列表行内联编辑用) */
function argsText(s: McpServerConfig): string {
  return (s.args ?? []).join(' ');
}
function setArgs(s: McpServerConfig, v: string): void {
  s.args = v.split(/\s+/).filter(Boolean);
}

function addServer(): void {
  const name = newName.value.trim();
  const command = newCommand.value.trim();
  if (!name || !command) return; // 缺名/缺命令不添加(后端同样会清理)
  mcpServers.value.push({
    name,
    command,
    args: newArgs.value.split(/\s+/).filter(Boolean),
    enabled: true,
  });
  newName.value = '';
  newCommand.value = '';
  newArgs.value = '';
}

function removeServer(index: number): void {
  mcpServers.value.splice(index, 1);
}

// ===== 保存(全量替换语义;保存后点该行「重连」生效) =====
const saving = ref(false);
const saveMsg = ref('');

async function saveNow(): Promise<void> {
  saving.value = true;
  saveMsg.value = '';
  try {
    await store.saveSettings({
      mcp_enabled: mcpEnabled.value,
      mcp_servers: mcpServers.value,
    });
    saveMsg.value = '已保存,点该行「重连」即生效';
    setTimeout(() => (saveMsg.value = ''), 2500);
  } catch (e) {
    saveMsg.value = `保存失败:${(e as Error).message}`;
  } finally {
    saving.value = false;
  }
}

onMounted(() => void loadStatus());
</script>

<template>
  <div v-show="show" class="sv-field">
    <div class="sv-field-label"><span class="sv-supreme blue" /> MCP 服务</div>
    <div class="sv-datalist">
      <div class="sv-data-row">
        <div class="info">
          <b>启用 MCP 服务</b>
          <span>连接本机 MCP stdio 服务器,其工具以 mcp_ 前缀注入 Agent(未知工具默认按危险级需授权)。改配置后点该行「重连」即生效</span>
        </div>
        <button type="button" class="sv-btn ghost" :aria-pressed="mcpEnabled" @click="mcpEnabled = !mcpEnabled">
          {{ mcpEnabled ? '已开启' : '已关闭' }}
        </button>
      </div>

      <div v-for="(s, i) in mcpServers" :key="`${s.name}-${i}`" class="sv-data-row" style="align-items: flex-start">
        <div class="info" style="flex: 1; min-width: 0">
          <div class="sv-inp-row">
            <label class="sv-inp-tag">名称</label>
            <input v-model="s.name" class="sv-input" placeholder="如 filesystem" title="服务器名,注册工具前缀 mcp_{名称}_{工具}" />
          </div>
          <div class="sv-inp-row">
            <label class="sv-inp-tag">命令</label>
            <input v-model="s.command" class="sv-input" placeholder="如 npx" title="可执行命令" />
          </div>
          <div class="sv-inp-row">
            <label class="sv-inp-tag">参数</label>
            <input
              :value="argsText(s)"
              class="sv-input"
              placeholder="空格分隔,如 -y @modelcontextprotocol/server-filesystem"
              title="命令行参数(空格分隔)"
              @input="setArgs(s, ($event.target as HTMLInputElement).value)"
            />
          </div>
          <!-- 运行状态 + 启停/重连(PLGM 3.2) -->
          <div class="flex items-center gap-2" style="margin-top: var(--space-1-5); flex-wrap: wrap">
            <span class="sv-badge" :class="stateClass(statusOf(s.name)?.state)">{{ stateLabel(statusOf(s.name)?.state) }}</span>
            <span v-if="statusOf(s.name)?.state === 'running'" class="sv-note-mini">工具 {{ statusOf(s.name)?.tool_count }} 个</span>
            <button type="button" class="sv-btn ghost sv-btn-sm" :disabled="acting === s.name" @click="act(s.name, 'restart')">重连</button>
            <button
              v-if="statusOf(s.name)?.state === 'running'"
              type="button"
              class="sv-btn ghost sv-btn-sm"
              :disabled="acting === s.name"
              @click="act(s.name, 'stop')"
            >停止</button>
            <button
              v-else
              type="button"
              class="sv-btn ghost sv-btn-sm"
              :disabled="acting === s.name || !s.name.trim() || !s.command.trim()"
              @click="act(s.name, 'start')"
            >启动</button>
          </div>
          <p v-if="statusOf(s.name)?.last_error" class="sv-note-mini err" style="margin-top: var(--space-1)">{{ statusOf(s.name)?.last_error }}</p>
        </div>
        <div style="display: grid; gap: var(--space-1-5); justify-items: end">
          <button type="button" class="sv-btn ghost" :aria-pressed="s.enabled" @click="s.enabled = !s.enabled">
            {{ s.enabled ? '启用中' : '已停用' }}
          </button>
          <button type="button" class="sv-btn ghost sv-btn-sm" @click="removeServer(i)">删除</button>
        </div>
      </div>

      <div class="sv-data-row" style="align-items: flex-start">
        <div class="info" style="flex: 1; min-width: 0">
          <b>新增服务器</b>
          <div class="sv-inp-row">
            <label class="sv-inp-tag">名称</label>
            <input v-model="newName" class="sv-input" placeholder="唯一名称" />
          </div>
          <div class="sv-inp-row">
            <label class="sv-inp-tag">命令</label>
            <input v-model="newCommand" class="sv-input" placeholder="可执行命令" />
          </div>
          <div class="sv-inp-row">
            <label class="sv-inp-tag">参数</label>
            <input v-model="newArgs" class="sv-input" placeholder="空格分隔参数(可空)" />
          </div>
        </div>
        <button type="button" class="sv-btn ghost" :disabled="!newName.trim() || !newCommand.trim()" @click="addServer">
          添加
        </button>
      </div>

      <div class="sv-btn-row">
        <button class="sv-btn ghost sv-btn-fill" :disabled="saving" @click="saveNow">
          {{ saving ? '保存中...' : '保存 MCP 设置' }}
        </button>
        <button type="button" class="sv-btn ghost sv-btn-sm" :disabled="!!acting" @click="loadStatus">刷新状态</button>
        <div v-if="saveMsg" class="sv-feedback ok sv-feedback-flex">{{ saveMsg }}</div>
      </div>
      <p class="sv-note">
        改开关/服务器列表后点「保存 MCP 设置」,再点该行「重连」即生效(无需重启应用);「停止/启动」为显式动作,
        不受该行「启用/停用」影响。服务器启动失败只影响该台,失败原因显示在该行下方。
      </p>
    </div>
  </div>
</template>
