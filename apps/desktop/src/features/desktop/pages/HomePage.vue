<script setup lang="ts">
import type { RuntimeInfo } from '../../../bridge';
import type { PageId } from '../navigation';
import AppIcon from '../AppIcon.vue';
import EmptyState from '../EmptyState.vue';
import LocalDeviceCard from '../LocalDeviceCard.vue';
import PanelCard from '../PanelCard.vue';
defineProps<{ runtime?: RuntimeInfo; loading: boolean }>();
defineEmits<{ refresh: []; navigate: [page: PageId] }>();
</script>
<template>
  <LocalDeviceCard
    :runtime="runtime"
    :loading="loading"
    @refresh="$emit('refresh')"
  />
  <div class="overview-grid">
    <PanelCard
      class="device-overview"
      title="设备"
      icon="devices"
      label="尚未接入"
    >
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
    </PanelCard>
    <PanelCard class="connection-overview" title="连接状态" icon="link">
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
    </PanelCard>
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
