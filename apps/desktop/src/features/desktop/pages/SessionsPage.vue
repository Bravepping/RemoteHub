<script setup lang="ts">
import AppIcon from '../AppIcon.vue';
import EmptyState from '../EmptyState.vue';
</script>
<template>
  <section class="product-card">
    <div class="card-header">
      <h2><AppIcon name="sessions" />会话记录</h2>
      <span class="subtle-label">尚未接入</span>
    </div>
    <EmptyState
      icon="clock"
      title="暂无会话数据"
      description="当前尚未提供会话记录。建立真实连接并接入记录能力后，你可以在这里查看会话信息。"
      heading="h3"
      ><p class="empty-detail">
        没有连接时，不显示模拟时长、延迟或画面统计。
      </p></EmptyState
    >
  </section>
</template>
