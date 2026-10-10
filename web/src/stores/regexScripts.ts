// 全局正则库 store(IMPHUB ⑥,2026-10-09):跨角色生效的显示层正则,
// 持久化 data/regex_scripts.json(整体读写;启停/删除/导入都走整体写)。
//
// 键名带 `regexScripts` 前缀(与 resources store 的 loadWorldBooks 同惯例):
// 门面 store.ts 逐键转发,storeFacade.test.ts 的「子 store 键全在门面上」元测试
// 依赖键名逐一对应,通用名(load/save/scripts)会与其它子 store 撞名或依赖改名转发。
//
// 加载语义:`ensureRegexScriptsLoaded()` 幂等(会话内只拉一次;失败不置 loaded、
// 下次调用重试,吞掉错误只记日志)——它被角色选择链路(授权哈希)与聊天渲染链路
// 共同依赖,失败不能挡住选角/渲染;需要把失败暴露给用户的场景(附件管理)走
// `loadRegexScripts()`(抛错)。
import { defineStore } from 'pinia';
import { ref } from 'vue';
import * as api from '../api';

export const useRegexScriptsStore = defineStore('app.regexScripts', () => {
  /** 全局正则库(整体) */
  const regexScripts = ref<api.RegexScript[]>([]);
  /** 是否已成功加载过(避免每次角色切换都重新拉取;失败保持 false 以便重试) */
  const regexScriptsLoaded = ref(false);

  /** 拉取全局正则库(整体替换);失败抛错由调用方决定提示方式,且不置 loaded(可重试) */
  async function loadRegexScripts(): Promise<void> {
    regexScripts.value = await api.getRegexScripts();
    regexScriptsLoaded.value = true;
  }

  /** 幂等加载(并发调用共享同一次在飞请求);失败只记日志、不置 loaded(下次重试) */
  let inflight: Promise<void> | null = null;
  async function ensureRegexScriptsLoaded(): Promise<void> {
    if (regexScriptsLoaded.value) return;
    if (inflight) return inflight;
    inflight = loadRegexScripts()
      .catch((e) => {
        console.error('加载全局正则库失败', e);
      })
      .finally(() => {
        inflight = null;
      });
    return inflight;
  }

  /** 整体保存(启停/删除/导入后回填;`skipped` 为后端宽进跳过的条目数) */
  async function saveRegexScripts(next: unknown[]): Promise<{ count: number; skipped: number }> {
    const res = await api.saveRegexScripts(next);
    regexScripts.value = res.scripts;
    regexScriptsLoaded.value = true;
    return { count: res.count, skipped: res.skipped };
  }

  return {
    regexScripts,
    regexScriptsLoaded,
    loadRegexScripts,
    ensureRegexScriptsLoaded,
    saveRegexScripts,
  };
});
