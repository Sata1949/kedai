// 模态框的键盘边界：进入、循环、关闭、返回；只处理最上层弹窗。
import { nextTick, onBeforeUnmount, onMounted, type Ref } from 'vue';

export function useDialogFocus(root: Ref<HTMLElement | null>, close: () => void): void {
  let previous: HTMLElement | null = null;
  let mounted = false;

  function isTopmost(): boolean {
    const dialog = root.value;
    if (!dialog?.isConnected) return false;
    const masks = document.querySelectorAll('.sv-modal-mask');
    const top = masks[masks.length - 1];
    return !top || top.contains(dialog);
  }

  function focusables(): HTMLElement[] {
    const dialog = root.value;
    if (!dialog) return [];
    return [...dialog.querySelectorAll<HTMLElement>('button, a[href], input, select, textarea, [tabindex]')]
      .filter((element) => {
        if (element.tabIndex < 0 || element.matches(':disabled, input[type="hidden"]')) return false;
        for (let node: HTMLElement | null = element; node; node = node.parentElement) {
          const style = getComputedStyle(node);
          if (node.hidden || node.inert || style.display === 'none' || style.visibility === 'hidden') return false;
          if (node === dialog) break;
        }
        return true;
      });
  }

  function focusFirst(): void {
    (focusables()[0] ?? root.value)?.focus();
  }

  function onFocus(event: FocusEvent): void {
    if (isTopmost() && event.target instanceof Node && !root.value?.contains(event.target)) focusFirst();
  }

  function onKeydown(event: KeyboardEvent): void {
    if (!isTopmost() || event.defaultPrevented || event.isComposing) return;
    if (event.key === 'Escape') {
      event.preventDefault();
      close();
      return;
    }
    if (event.key !== 'Tab') return;
    const list = focusables();
    const first = list[0];
    const last = list[list.length - 1];
    if (!first || !last) {
      event.preventDefault();
      root.value?.focus();
    } else if (!root.value?.contains(document.activeElement) || (event.shiftKey && document.activeElement === first)) {
      event.preventDefault();
      (event.shiftKey ? last : first).focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  }

  onMounted(async () => {
    mounted = true;
    previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    document.addEventListener('keydown', onKeydown);
    document.addEventListener('focusin', onFocus);
    await nextTick();
    if (mounted && isTopmost()) focusFirst();
  });
  onBeforeUnmount(() => {
    mounted = false;
    document.removeEventListener('keydown', onKeydown);
    document.removeEventListener('focusin', onFocus);
    // 切到另一个面板且焦点已交接时不抢回；原入口被移除时也不强制聚焦。
    if ((root.value?.contains(document.activeElement) || document.activeElement === document.body) && previous?.isConnected) previous.focus();
  });
}
