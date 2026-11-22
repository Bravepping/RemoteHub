<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue';
import { getRuntimeInfo, type RuntimeInfo } from './bridge';
import DesktopShell from './features/desktop/DesktopShell.vue';
import ExperimentWorkspace from './features/experiments/ExperimentWorkspace.vue';

const runtime = ref<RuntimeInfo>();
const loading = ref(false);
const error = ref('');
let disposed = false;
const experiment = computed(() =>
  Boolean(
    runtime.value?.p004_enabled ||
    runtime.value?.p004_lan_enabled ||
    runtime.value?.p006_enabled,
  ),
);
async function refresh() {
  if (loading.value || disposed) return;
  loading.value = true;
  error.value = '';
  try {
    const value = await getRuntimeInfo();
    if (!disposed) runtime.value = value;
  } catch {
    if (!disposed) {
      runtime.value = undefined;
      error.value =
        '暂时无法读取本机能力。请在桌面客户端中重新检测；浏览器预览无法连接本机核心。';
    }
  } finally {
    if (!disposed) loading.value = false;
  }
}
onMounted(refresh);
onUnmounted(() => {
  disposed = true;
});
</script>
<template>
  <ExperimentWorkspace
    v-if="experiment && runtime"
    :initial-runtime="runtime"
  />
  <DesktopShell
    v-else
    :runtime="runtime"
    :loading="loading"
    :error="error"
    @refresh="refresh"
  />
</template>
