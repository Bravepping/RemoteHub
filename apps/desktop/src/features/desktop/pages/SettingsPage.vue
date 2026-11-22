<script setup lang="ts">
import type { RuntimeInfo } from '../../../bridge';
import AppIcon from '../AppIcon.vue';
defineProps<{ runtime?: RuntimeInfo; loading: boolean }>();
defineEmits<{ refresh: [] }>();
</script>
<template>
  <section class="product-card">
    <div class="card-header">
      <h2><AppIcon name="devices" />本机信息</h2>
      <button
        class="outline-button"
        :disabled="loading"
        @click="$emit('refresh')"
      >
        <AppIcon name="refresh" />{{ loading ? '读取中…' : '重新检测' }}
      </button>
    </div>
    <p class="card-description">
      以下信息来自本机核心。无法读取时保留未知状态。
    </p>
    <dl class="settings-list">
      <div>
        <dt>应用版本</dt>
        <dd>{{ runtime?.app_version ?? '未读取' }}</dd>
      </div>
      <div>
        <dt>操作系统</dt>
        <dd>{{ runtime?.os ?? '未读取' }}</dd>
      </div>
      <div>
        <dt>系统架构</dt>
        <dd>{{ runtime?.arch ?? '未读取' }}</dd>
      </div>
      <div>
        <dt>采集后端</dt>
        <dd>{{ runtime?.capture_backend ?? '未读取' }}</dd>
      </div>
    </dl>
  </section>
  <section class="product-card settings-security">
    <div class="card-header">
      <h2><AppIcon name="shield" />权限与启动</h2>
      <span class="subtle-label">只读状态</span>
    </div>
    <dl class="settings-list">
      <div>
        <dt>远程访问</dt>
        <dd>
          {{
            runtime
              ? runtime.remote_access_enabled
                ? '状态展示尚未接入'
                : '关闭'
              : '未读取'
          }}
        </dd>
      </div>
      <div>
        <dt>后台驻留</dt>
        <dd>尚未提供设置</dd>
      </div>
      <div>
        <dt>开机启动</dt>
        <dd>尚未提供设置</dd>
      </div>
    </dl>
    <div class="inline-note">
      <AppIcon name="info" />
      <p>
        当前不提供权限开关。切换页面或检测本机信息不会启用采集、连接或远程控制。
      </p>
    </div>
  </section>
</template>
