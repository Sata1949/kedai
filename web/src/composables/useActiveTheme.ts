import { onMounted, onUnmounted, ref, type Ref } from 'vue';
import { getActiveThemeId, subscribeThemes } from '../theme/registry';

/**
 * 组件侧读当前主题 id 的窄接口。
 * 主题注册表不是 Pinia store;这里用订阅桥把它变成 Vue ref,供 App 决定
 * 是否挂整窗环境层、ChatWindow/TaskBoard 决定是否保留 stage 环境层。
 */
export function useActiveThemeId(): Ref<string> {
  const activeId = ref(getActiveThemeId());
  let unsubscribe: (() => void) | undefined;

  onMounted(() => {
    unsubscribe = subscribeThemes((snapshot) => {
      activeId.value = snapshot.activeId;
    });
  });
  onUnmounted(() => unsubscribe?.());

  return activeId;
}
