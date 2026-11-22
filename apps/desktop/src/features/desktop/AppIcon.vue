<script setup lang="ts">
const paths = {
  home: ['m3 10 9-7 9 7', 'M5 9v11h5v-6h4v6h5V9'],
  devices: ['M3 4h18v12H3z', 'M8 21h8m-4-5v5'],
  link: [
    'm10 13 4-4',
    'M8 15H6a4 4 0 0 1-3-7l3-3a4 4 0 0 1 6 0',
    'M16 9h2a4 4 0 0 1 3 7l-3 3a4 4 0 0 1-6 0',
  ],
  sessions: ['M4 4h16v12H9l-5 4z', 'M8 8h8m-8 4h5'],
  settings: [
    'm9 3-1 3-3 1-2 3 2 3 1 3 3 1 3 2 3-2 3-1 1-3 2-3-2-3-3-1-1-3z',
    'M15 12a3 3 0 1 1-6 0 3 3 0 0 1 6 0',
  ],
  shield: ['M12 3 4 6v6c0 4 4 7 8 9 4-2 8-5 8-9V6z', 'm8 12 3 3 5-6'],
  arrow: ['M4 12h16m-6-6 6 6-6 6'],
  refresh: [
    'M20 7v5h-5',
    'M4 17v-5h5',
    'M5 8a8 8 0 0 1 13-3l2 3M4 16l2 3a8 8 0 0 0 13-3',
  ],
  clock: ['M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0', 'M12 7v5l3 2'],
  info: ['M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0', 'M12 11v6m0-10v1'],
} as const;
export type IconName = keyof typeof paths;
defineProps<{ name: IconName }>();
</script>
<template>
  <svg
    width="24"
    height="24"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    stroke-width="1.7"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
    focusable="false"
  >
    <path v-for="(d, i) in paths[name]" :key="i" :d="d" />
  </svg>
</template>
