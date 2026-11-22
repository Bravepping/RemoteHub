<script setup lang="ts">
import type { ReadStatus } from '../../state/desktop';
import AppIcon from './AppIcon.vue';
defineProps<{ status: ReadStatus; message: string }>();
defineEmits<{ cancel: []; retry: [] }>();
</script>
<template>
  <section
    class="read-feedback"
    :class="{ 'read-failed': status === 'error' }"
    aria-label="本机信息读取状态"
  >
    <div
      class="read-feedback-message"
      :role="status === 'error' ? 'alert' : 'status'"
      aria-atomic="true"
    >
      <AppIcon :name="status === 'ready' ? 'shield' : 'info'" />
      <p>{{ message }}</p>
    </div>
    <button
      v-if="status === 'loading' || status === 'refreshing'"
      class="outline-button"
      @click="$emit('cancel')"
    >
      取消本次读取
    </button>
    <button
      v-else-if="
        status === 'error' || status === 'cancelled' || status === 'idle'
      "
      class="outline-button"
      @click="$emit('retry')"
    >
      重试读取
    </button>
  </section>
</template>
