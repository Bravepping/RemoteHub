<script setup lang="ts">
import { ref } from 'vue';
import AppIcon from './AppIcon.vue';
const expanded = ref(true);
defineEmits<{ prepareConnection: [] }>();
</script>
<template>
  <div class="session-toolbar" role="toolbar" aria-label="会话工具栏">
    <div class="toolbar-topline">
      <span class="toolbar-state"><span class="status-dot" />尚未连接</span>
      <button
        class="toolbar-toggle"
        :aria-expanded="expanded"
        aria-controls="session-toolbar-actions"
        @click="expanded = !expanded"
      >
        <AppIcon :name="expanded ? 'collapse' : 'expand'" />{{
          expanded ? '收起工具栏' : '展开工具栏'
        }}
      </button>
    </div>
    <div v-show="expanded" id="session-toolbar-actions" class="toolbar-details">
      <dl class="toolbar-metrics">
        <div>
          <dt>路径</dt>
          <dd>暂无会话</dd>
        </div>
        <div>
          <dt>RTT</dt>
          <dd>未取得</dd>
        </div>
        <div>
          <dt>FPS</dt>
          <dd>未取得</dd>
        </div>
      </dl>
      <div class="toolbar-buttons">
        <button disabled aria-describedby="session-actions-unavailable">
          <AppIcon name="fullscreen" />全屏
        </button>
        <button disabled aria-describedby="session-actions-unavailable">
          <AppIcon name="settings" />画质
        </button>
        <button disabled aria-describedby="session-actions-unavailable">
          码率上限
        </button>
        <button
          class="disconnect-button"
          disabled
          aria-describedby="session-actions-unavailable"
        >
          <AppIcon name="power" />断开
        </button>
        <button class="prepare-button" @click="$emit('prepareConnection')">
          查看连接准备<AppIcon name="arrow" />
        </button>
      </div>
      <p id="session-actions-unavailable">
        当前没有活动会话；画面设置与断开操作暂不可用。
      </p>
    </div>
  </div>
</template>
