<script setup lang="ts">
import AppIcon, { type IconName } from './AppIcon.vue';
withDefaults(
  defineProps<{
    icon: IconName;
    title: string;
    description: string;
    compact?: boolean;
    heading?: 'h2' | 'h3';
  }>(),
  { compact: false, heading: 'h2' },
);
</script>
<template>
  <div class="empty-state" :class="{ compact }">
    <div class="empty-illustration"><AppIcon :name="icon" /></div>
    <component :is="heading">{{ title }}</component>
    <p>{{ description }}</p>
    <slot />
  </div>
</template>
