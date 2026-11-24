<script setup lang="ts">
import { isTauri } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { computed, onMounted, onUnmounted } from 'vue';
import { storeToRefs } from 'pinia';
import { useDesktopStore } from './state/desktop';
import DesktopShell from './features/desktop/DesktopShell.vue';
import ExperimentWorkspace from './features/experiments/ExperimentWorkspace.vue';
import PublicOwnedView from './features/public-video/PublicOwnedView.vue';

const ownedWindow = (() => {
  try {
    return (
      isTauri() &&
      /^remote-session-[a-f0-9]{32}$/.test(getCurrentWindow().label)
    );
  } catch {
    return false;
  }
})();
const desktop = useDesktopStore();
const { runtime, loading, status, feedback } = storeToRefs(desktop);
const experiment = computed(() =>
  Boolean(
    !runtime.value?.p3_enabled &&
    (runtime.value?.p004_enabled ||
      runtime.value?.p004_lan_enabled ||
      runtime.value?.p006_enabled),
  ),
);
const refresh = () => desktop.refresh();
onMounted(() => {
  if (!ownedWindow) void refresh();
});
onUnmounted(() => desktop.reset());
</script>
<template>
  <PublicOwnedView v-if="ownedWindow" />
  <ExperimentWorkspace
    v-else-if="experiment && runtime"
    :initial-runtime="runtime"
  />
  <DesktopShell
    v-else
    :runtime="runtime"
    :loading="loading"
    :read-status="status"
    :feedback="feedback"
    @refresh="refresh"
    @cancel-read="desktop.cancelRead()"
  />
</template>
