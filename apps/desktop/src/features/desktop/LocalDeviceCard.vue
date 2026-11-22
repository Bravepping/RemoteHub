<script setup lang="ts">
import type { RuntimeInfo } from '../../bridge';
import AppIcon from './AppIcon.vue';
defineProps<{ runtime?: RuntimeInfo; loading: boolean }>();
defineEmits<{ refresh: [] }>();
</script>
<template>
  <section class="local-card" aria-label="本机能力卡片" :aria-busy="loading">
    <div class="computer-tile"><AppIcon name="devices" /></div>
    <div class="local-description">
      <span class="section-label">本机 · 能力信息</span>
      <h2>
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
          loading
            ? '正在等待本机核心回应；读取信息不会开启远程访问。'
            : runtime
              ? '仅表示本机核心可响应，不代表设备在线或已获远控授权。'
              : '启动桌面客户端后，可在这里查看真实本机信息。'
        }}
      </p>
      <div class="local-card-tags">
        <span
          v-if="!loading && runtime?.identity?.status === 'ready'"
          class="muted-chip"
          >本机标识 {{ runtime.identity.device_id }}</span
        >
        <span v-if="runtime && !loading" class="muted-chip"
          >{{ runtime.os }} · {{ runtime.arch }}</span
        >
        <span class="muted-chip">{{
          loading
            ? '等待本次读取'
            : runtime?.p3_enabled
              ? '仅允许本次明确批准的连接'
              : runtime?.remote_access_enabled === false
                ? '远程访问关闭'
                : '远程服务尚未接入'
        }}</span>
      </div>
    </div>
    <button
      class="outline-button"
      :disabled="loading"
      @click="$emit('refresh')"
    >
      <AppIcon name="refresh" />{{ loading ? '读取中…' : '重新检测' }}
    </button>
  </section>
</template>
