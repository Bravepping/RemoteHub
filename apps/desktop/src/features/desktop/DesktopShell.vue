<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue';
import type { RuntimeInfo } from '../../bridge';
import AppIcon from './AppIcon.vue';
import HomePage from './pages/HomePage.vue';
import DevicesPage from './pages/DevicesPage.vue';
import ConnectPage from './pages/ConnectPage.vue';
import SessionsPage from './pages/SessionsPage.vue';
import SettingsPage from './pages/SettingsPage.vue';
import { pages, pageFromHash, pageLink, type PageId } from './navigation';
import './desktop.css';
import './p103.css';

defineProps<{ runtime?: RuntimeInfo; loading: boolean; error: string }>();
defineEmits<{ refresh: [] }>();
const active = ref<PageId>(pageFromHash(window.location.hash));
const current = computed(() => pages.find((p) => p.id === active.value)!);
const heading = ref<HTMLElement>();
const originalTitle = document.title;
function applyPage(page: PageId) {
  if (active.value === page) return;
  active.value = page;
  void nextTick(() => heading.value?.focus({ preventScroll: true }));
}
function navigate(page: PageId) {
  window.location.hash = pageLink(page);
  applyPage(page);
}
function hashChanged() {
  applyPage(pageFromHash(window.location.hash));
}
function focusContent() {
  document.getElementById('main-content')?.focus({ preventScroll: true });
}
watch(
  current,
  (page) => {
    document.title = `RemoteHub · ${page.title}`;
  },
  { immediate: true },
);
onMounted(() => window.addEventListener('hashchange', hashChanged));
onUnmounted(() => {
  window.removeEventListener('hashchange', hashChanged);
  document.title = originalTitle;
});
</script>
<template>
  <div class="product-shell">
    <a href="#main-content" class="skip-link" @click.prevent="focusContent"
      >跳到页面内容</a
    >
    <aside class="product-sidebar">
      <div class="product-brand">
        <span class="brand-mark"><AppIcon name="devices" /></span
        ><span>RemoteHub<small>你的电脑，触手可及</small></span>
      </div>
      <nav aria-label="主导航">
        <a
          v-for="page in pages"
          :key="page.id"
          :href="pageLink(page.id)"
          :aria-current="active === page.id ? 'page' : undefined"
          @click.prevent="navigate(page.id)"
          ><AppIcon :name="page.icon" /><span>{{ page.title }}</span></a
        >
      </nav>
      <div class="sidebar-footer">
        <span class="status-dot" />{{
          runtime
            ? runtime.remote_access_enabled
              ? '远程状态尚未接入'
              : '远程访问关闭'
            : '本机能力未连接'
        }}
        <p>仅在双方同意后连接</p>
      </div>
    </aside>
    <main id="main-content" class="product-main" tabindex="-1">
      <header class="page-header">
        <div>
          <p class="page-eyebrow">REMOTEHUB <span>/</span> 桌面客户端</p>
          <h1 ref="heading" tabindex="-1">{{ current.title }}</h1>
          <p class="page-subtitle">{{ current.subtitle }}</p>
        </div>
        <span class="local-badge"><AppIcon name="shield" />本机工作区</span>
      </header>
      <div v-if="error" class="capability-notice" role="alert">
        <AppIcon name="info" />
        <p>{{ error }}</p>
      </div>
      <HomePage
        v-if="active === 'home'"
        :runtime="runtime"
        :loading="loading"
        @refresh="$emit('refresh')"
        @navigate="navigate"
      />
      <DevicesPage
        v-else-if="active === 'devices'"
        :runtime="runtime"
        :loading="loading"
        @refresh="$emit('refresh')"
        @navigate="navigate"
      />
      <ConnectPage v-else-if="active === 'connect'" @navigate="navigate" />
      <SessionsPage v-else-if="active === 'sessions'" @navigate="navigate" />
      <SettingsPage
        v-else
        :runtime="runtime"
        :loading="loading"
        @refresh="$emit('refresh')"
      />
      <footer class="product-footer">
        <AppIcon name="shield" /><span
          >连接由你决定 · 当前不会自动发起远程访问</span
        >
      </footer>
    </main>
  </div>
</template>
