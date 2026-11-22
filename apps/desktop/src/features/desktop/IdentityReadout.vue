<script setup lang="ts">
import type { DeviceIdentity, IdentityErrorCode } from '../../ipc/contracts';
defineProps<{ identity?: DeviceIdentity; loading: boolean }>();
const messages: Record<IdentityErrorCode, string> = {
  IDENTITY_UNSUPPORTED: '当前系统不支持 Windows 设备身份保护。',
  IDENTITY_RANDOM_FAILED: '无法安全创建设备身份，请关闭并重新打开客户端。',
  IDENTITY_PROTECT_FAILED: 'Windows 无法保护设备私钥，本次未创建可用身份。',
  IDENTITY_UNPROTECT_FAILED: 'Windows 无法恢复设备身份，已保留原文件。',
  IDENTITY_STORAGE_FAILED:
    '无法读取或保存设备身份，请检查本机用户目录后重开客户端。',
  IDENTITY_STORAGE_UNSAFE: '设备身份存储位置或访问权限不符合要求，已停止读取。',
  IDENTITY_BUSY: '其他实例正在使用设备身份，请关闭其他实例后重开客户端。',
  IDENTITY_STORE_INVALID:
    '设备身份文件无法识别，已保留原文件，未自动更换身份。',
  IDENTITY_STORE_INCOMPLETE:
    '发现未完成的身份保存记录，已保留现场，未自动更换身份。',
};
</script>
<template>
  <section class="product-card" aria-label="本机设备身份" :aria-busy="loading">
    <div class="card-header">
      <h2>本机设备身份</h2>
      <span class="subtle-label">仅本地</span>
    </div>
    <p v-if="loading" role="status">正在读取本机设备身份…</p>
    <template v-else-if="identity?.status === 'ready'">
      <dl class="settings-list">
        <div>
          <dt>设备标识</dt>
          <dd>
            <code class="identity-value">{{ identity.device_id }}</code>
          </dd>
        </div>
        <div>
          <dt>公钥指纹</dt>
          <dd>
            <code class="identity-value">{{ identity.fingerprint }}</code>
          </dd>
        </div>
      </dl>
      <p class="card-description">
        私钥由 Windows
        当前用户保护，仅保存在本机。可选中文本复制标识；短标识不代表已联网、配对或获得远控授权。
      </p>
    </template>
    <p v-else-if="identity?.status === 'unavailable'" role="alert">
      {{ messages[identity.error_code] }}
    </p>
    <p v-else role="status">本机设备身份尚未读取。</p>
  </section>
</template>
<style scoped>
.identity-value {
  overflow-wrap: anywhere;
  user-select: text;
}
</style>
