<script setup lang="ts">
import EmptyState from '../EmptyState.vue';
import PanelCard from '../PanelCard.vue';
import SessionToolbar from '../SessionToolbar.vue';
import type { PageId } from '../navigation';
defineEmits<{ navigate: [page: PageId] }>();
</script>
<template>
  <PanelCard
    title="会话工作区"
    icon="devices"
    label="暂无活动会话"
    class="session-workspace"
  >
    <div class="session-stage" aria-label="会话画面空态">
      <EmptyState
        icon="devices"
        title="尚未连接远端画面"
        description="建立经过双方同意的真实会话后，画面将在这里呈现。"
        heading="h3"
      />
      <SessionToolbar @prepare-connection="$emit('navigate', 'connect')" />
    </div>
  </PanelCard>
  <PanelCard
    title="会话记录"
    icon="sessions"
    label="尚未接入"
    class="session-history"
  >
    <EmptyState
      icon="clock"
      title="暂无会话数据"
      description="当前尚未提供会话记录。建立真实连接并接入记录能力后，你可以在这里查看会话信息。"
      heading="h3"
      ><p class="empty-detail">
        没有连接时，不显示模拟时长、延迟或画面统计。
      </p></EmptyState
    >
  </PanelCard>
</template>
