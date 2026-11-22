<script setup lang="ts">
import type { RuntimeInfo } from '../../../bridge';
import type { PageId } from '../navigation';
import AppIcon from '../AppIcon.vue';
import EmptyState from '../EmptyState.vue';
defineProps<{ runtime?: RuntimeInfo; loading: boolean }>();
defineEmits<{ refresh: []; navigate: [page: PageId] }>();
</script>
<template>
  <section class="local-card" aria-labelledby="local-title">
    <div class="computer-tile"><AppIcon name="devices" /></div>
    <div class="local-description">
      <span class="section-label">本机状态</span>
      <h2 id="local-title">
        <span
          class="status-dot"
          :class="{ connected: runtime && !loading }"
        />{{
          loading
            ? '正在读取本机能力…'
            : runtime
              ? '本机核心已连接'
              : '本机能力未连接'
        }}
      </h2>
      <p role="status">
        {{
          runtime
            ? '仅表示本机核心可响应，远程连接尚未开放。'
            : '启动桌面客户端后，可在这里查看真实本机信息。'
        }}
      </p>
      <span class="muted-chip">{{
        runtime?.remote_access_enabled === false
          ? '远程访问关闭'
          : '远程服务尚未接入'
      }}</span>
    </div>
    <button
      class="outline-button"
      :disabled="loading"
      @click="$emit('refresh')"
    >
      <AppIcon name="refresh" />{{ loading ? '读取中…' : '重新检测' }}
    </button>
  </section>
  <div class="overview-grid">
    <section class="product-card device-overview">
      <div class="card-header">
        <h2><AppIcon name="devices" />设备</h2>
        <span class="subtle-label">尚未接入</span>
      </div>
      <EmptyState
        icon="devices"
        title="你的设备，将在这里相见"
        description="设备身份与在线服务尚未接入。接入后，这里只会显示真实的已授权设备。"
        heading="h3"
        compact
        ><a
          href="#/devices"
          class="outline-button"
          @click.prevent="$emit('navigate', 'devices')"
          >查看设备<AppIcon name="arrow" /></a
      ></EmptyState>
    </section>
    <section class="product-card connection-overview">
      <div class="card-header">
        <h2><AppIcon name="link" />连接状态</h2>
      </div>
      <div class="connection-placeholder">
        <span class="connection-orbit"><AppIcon name="link" /></span>
        <h3>还未建立连接</h3>
        <p>连接能力尚未接入，不会自动联系其他电脑。</p>
      </div>
      <dl class="connection-facts">
        <div>
          <dt>连接路径</dt>
          <dd>暂无会话</dd>
        </div>
        <div>
          <dt>连接延迟</dt>
          <dd>暂不可用</dd>
        </div>
        <div>
          <dt>画面统计</dt>
          <dd>暂无会话</dd>
        </div>
      </dl>
      <a
        href="#/connect"
        class="text-link"
        @click.prevent="$emit('navigate', 'connect')"
        >前往连接页面<AppIcon name="arrow"
      /></a>
    </section>
  </div>
  <div class="home-note">
    <AppIcon name="shield" />
    <div>
      <h2>每次连接，知情且可控</h2>
      <p>连接能力接入后，仍需对端同意。当前仅提供本机信息和页面导航。</p>
    </div>
    <a
      href="#/settings"
      class="text-link"
      @click.prevent="$emit('navigate', 'settings')"
      >查看安全状态<AppIcon name="arrow"
    /></a>
  </div>
</template>
