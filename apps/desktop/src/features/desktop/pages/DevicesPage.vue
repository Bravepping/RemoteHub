<script setup lang="ts">
import type { PageId } from '../navigation';
import AppIcon from '../AppIcon.vue';
import EmptyState from '../EmptyState.vue';
import type { RuntimeInfo } from '../../../bridge';
import LocalDeviceCard from '../LocalDeviceCard.vue';
import PanelCard from '../PanelCard.vue';
import V2PeerPairing from '../V2PeerPairing.vue';
defineProps<{ runtime?: RuntimeInfo; loading: boolean }>();
defineEmits<{ refresh: []; navigate: [page: PageId] }>();
</script>
<template>
  <LocalDeviceCard
    :runtime="runtime"
    :loading="loading"
    @refresh="$emit('refresh')"
  />
  <V2PeerPairing v-if="!loading && runtime?.v2_peer_pairing_enabled === true" />
  <PanelCard title="设备列表" icon="devices" label="尚未接入">
    <template #actions>
      <button disabled aria-describedby="device-unavailable">添加设备</button>
    </template>
    <EmptyState
      icon="devices"
      title="设备列表尚未接入"
      description="当前没有可以读取的设备数据。设备身份与在线服务接入后，将显示真实设备和授权状态。"
      heading="h3"
      ><p id="device-unavailable" class="empty-detail">
        添加设备功能将在设备服务接入后开放。
      </p>
      <a
        href="#/connect"
        class="outline-button"
        @click.prevent="$emit('navigate', 'connect')"
        >了解连接状态<AppIcon name="arrow" /></a
    ></EmptyState>
  </PanelCard>
</template>
