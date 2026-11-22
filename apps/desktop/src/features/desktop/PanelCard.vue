<script setup lang="ts">
import AppIcon, { type IconName } from './AppIcon.vue';
defineProps<{ title: string; icon: IconName; label?: string }>();
</script>
<template>
  <section class="product-card" :aria-label="title">
    <div class="card-header">
      <h2><AppIcon :name="icon" />{{ title }}</h2>
      <div class="card-actions">
        <span v-if="label" class="subtle-label">{{ label }}</span>
        <slot name="actions" />
      </div>
    </div>
    <slot />
  </section>
</template>
