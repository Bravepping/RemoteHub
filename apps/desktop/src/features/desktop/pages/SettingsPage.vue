<script setup lang="ts">
import type { RuntimeInfo } from '../../../bridge';
import AppIcon from '../AppIcon.vue';
import IdentityReadout from '../IdentityReadout.vue';
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
      {{
        loading
          ? '正在读取本次信息，尚无可展示的结果。'
          : '以下信息来自本机核心。无法读取时保留未知状态。'
      }}
    </p>
    <dl class="settings-list">
      <div>
        <dt>应用版本</dt>
        <dd>{{ loading ? '读取中…' : (runtime?.app_version ?? '未读取') }}</dd>
      </div>
      <div>
        <dt>操作系统</dt>
        <dd>{{ loading ? '读取中…' : (runtime?.os ?? '未读取') }}</dd>
      </div>
      <div>
        <dt>系统架构</dt>
        <dd>{{ loading ? '读取中…' : (runtime?.arch ?? '未读取') }}</dd>
      </div>
      <div>
        <dt>采集后端</dt>
        <dd>
          {{ loading ? '读取中…' : (runtime?.capture_backend ?? '未读取') }}
        </dd>
      </div>
    </dl>
  </section>
  <IdentityReadout :identity="runtime?.identity" :loading="loading" />
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
            loading
              ? '读取中…'
              : runtime?.p3_enabled
                ? '仅本次会话授权，未开启无人值守'
                : runtime
                  ? runtime.remote_access_enabled
                    ? '状态展示尚未接入'
                    : '关闭'
                  : '未读取'
          }}
        </dd>
      </div>
      <div>
        <dt>后台驻留</dt>
        <dd>
          {{
            !runtime
              ? '托盘状态未读取'
              : runtime.p004_enabled || runtime.p006_enabled
                ? '实验构建不提供托盘驻留'
                : '由原生托盘菜单管理，仅本次有效'
          }}
        </dd>
      </div>
      <div>
        <dt>开机启动</dt>
        <dd>不自动启用</dd>
      </div>
    </dl>
    <div class="inline-note">
      <AppIcon name="info" />
      <p>
        当前不提供权限开关。切换页面或检测本机信息不会启用采集、连接或远程控制。
      </p>
    </div>
    <p class="card-description">
      默认点击 ×
      真正退出。普通桌面构建可在托盘菜单主动勾选“本次允许托盘驻留”，此后关窗仅隐藏；通过“显示
      RemoteHub”返回，“真正退出
      RemoteHub”结束程序。驻留不授予采集或远控权限，也不会保存到下次启动。
    </p>
  </section>
</template>
