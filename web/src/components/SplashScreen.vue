<script setup lang="ts">
// 启动动画:Logo + 文字逐字淡入
import { onMounted, ref } from 'vue';
import { useAppStore } from '../store';

const store = useAppStore();
const fading = ref(false);

onMounted(() => {
  // 1.2s 后开始淡出,0.4s 淡出动画,总计 1.6s
  setTimeout(() => {
    fading.value = true;
    setTimeout(() => {
      store.splashDone = true;
    }, 400);
  }, 1200);
});

const letters = 'KEDAI'.split('');
</script>

<template>
  <div class="sv-splash" :class="{ 'fade-out': fading }">
    <div class="sv-splash-content">
      <img class="sv-splash-logo" src="/logo.png" alt="Kedai" draggable="false" />
      <h1 class="sv-splash-title">
        <span
          v-for="(letter, i) in letters"
          :key="i"
          :style="{ '--i': i }"
        >{{ letter }}</span>
      </h1>
    </div>
  </div>
</template>

<style scoped>
/* UIP-14:逐字延迟从 JS 内联裸值(0.3 + i*0.08s)改为令牌求值——JS 侧只传序号 --i。
   规则放 scoped(MAINTENANCE D-5:新样式不进全局域;全局域有行数 ratchet)。
   0.3→0.32s 为同档归一(UIP-6 口径);步长 80ms = 2×--stagger-step(与 hub-sub-item
   nth-child 的 ×2 写法同先例),逐字节奏保持原值。 */
.sv-splash-title span {
  animation-delay: calc(var(--dur-slower) + var(--stagger-step) * 2 * var(--i, 0));
}
</style>
