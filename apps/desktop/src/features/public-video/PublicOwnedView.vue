<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core';
import { onBeforeUnmount, onMounted, ref } from 'vue';
import PublicVideoPanel from './PublicVideoPanel.vue';
import { windowAttachment, type WindowAttachment } from './publicSessionWindow';

const attached = ref<WindowAttachment>();
const failed = ref(false);
let alive = true;

onMounted(async () => {
  try {
    const response = await invoke('public_session_window_attach');
    if (alive) attached.value = windowAttachment(response);
  } catch {
    if (alive) failed.value = true;
  }
});
onBeforeUnmount(() => {
  alive = false;
});
</script>

<template>
  <main class="owned-view">
    <PublicVideoPanel
      v-if="attached"
      :available="true"
      :control-available="true"
      :window-owner="attached"
    />
    <p v-else role="status">
      {{
        failed
          ? '远控窗口未能接管本次会话。请关闭窗口，检查邀请后重试。'
          : '正在接管本次远控会话…'
      }}
    </p>
  </main>
</template>

<style scoped>
.owned-view {
  min-height: 100dvh;
  padding: 0.75rem;
  color: #edf5ff;
  background: #0b1522;
}
</style>
