<script setup lang="ts">
import type { PageId } from '../navigation';
import AppIcon from '../AppIcon.vue';
import EmptyState from '../EmptyState.vue';
import { ref } from 'vue';
import ConnectionDialog from '../ConnectionDialog.vue';
defineEmits<{ navigate: [page: PageId] }>();
const preparationOpen = ref(false);
</script>
<template>
  <div class="connection-page-grid">
    <section class="product-card">
      <EmptyState
        icon="link"
        title="连接能力暂不可用"
        description="设备选择、配对与连接服务尚未接入。当前不会尝试直连或中继，也不会发送远程请求。"
        ><button disabled aria-describedby="connect-unavailable">
          连接设备
        </button>
        <p id="connect-unavailable" class="empty-detail">
          需要先接入真实设备和会话服务。
        </p>
        <button class="outline-button" @click="preparationOpen = true">
          查看连接准备
        </button>
        <a
          href="#/devices"
          class="text-link"
          @click.prevent="$emit('navigate', 'devices')"
          >查看设备页面<AppIcon name="arrow" /></a
      ></EmptyState>
    </section>
    <section class="product-card permission-guide">
      <span class="section-label">连接前的约定</span>
      <h2>连接始于双方同意</h2>
      <ol>
        <li>
          <span>01</span>
          <div>
            <h3>选择已授权的电脑</h3>
            <p>设备身份可验证，连接对象明确。</p>
          </div>
        </li>
        <li>
          <span>02</span>
          <div>
            <h3>由对端确认本次连接</h3>
            <p>打开页面不代表授予访问权限。</p>
          </div>
        </li>
        <li>
          <span>03</span>
          <div>
            <h3>随时结束会话</h3>
            <p>建立会话后，双方都应能主动停止。</p>
          </div>
        </li>
      </ol>
    </section>
  </div>
  <ConnectionDialog v-if="preparationOpen" @close="preparationOpen = false" />
</template>
