<script setup lang="ts">
import { computed, onMounted, onUnmounted } from 'vue';
import { storeToRefs } from 'pinia';
import { useDesktopStore } from './state/desktop';
import DesktopShell from './features/desktop/DesktopShell.vue';
import ExperimentWorkspace from './features/experiments/ExperimentWorkspace.vue';

const desktop = useDesktopStore();
const { runtime, loading, errorMessage: error } = storeToRefs(desktop);
const experiment = computed(() =>
  Boolean(
    runtime.value?.p004_enabled ||
    runtime.value?.p004_lan_enabled ||
    runtime.value?.p006_enabled,
  ),
);
const refresh = () => desktop.refresh();
onMounted(refresh);
onUnmounted(() => desktop.cancelRead());
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
