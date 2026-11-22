<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from 'vue';
import AppIcon from './AppIcon.vue';
const emit = defineEmits<{ close: [] }>();
const dialog = ref<HTMLDialogElement>();
onMounted(() => dialog.value?.showModal());
onBeforeUnmount(() => dialog.value?.close());
</script>
<template>
  <dialog
    ref="dialog"
    class="connection-dialog"
    aria-labelledby="connection-dialog-title"
    aria-describedby="connection-dialog-description"
    @close="emit('close')"
  >
    <header class="dialog-header">
      <h2 id="connection-dialog-title"><AppIcon name="link" />连接准备</h2>
      <button
        class="icon-button"
        aria-label="关闭连接准备"
        autofocus
        @click="dialog?.close()"
      >
        <AppIcon name="close" />
      </button>
    </header>
    <div class="dialog-body">
      <div class="connection-endpoints" aria-label="连接对象">
        <div>
          <AppIcon name="devices" /><strong>本机</strong
          ><span>身份尚未接入</span>
        </div>
        <AppIcon name="link" />
        <div>
          <AppIcon name="devices" /><strong>对端设备</strong
          ><span>尚未选择</span>
        </div>
      </div>
      <p id="connection-dialog-description" class="dialog-notice">
        当前未创建会话。设备身份、对端批准与会话服务接入后，才能发起连接。
      </p>
      <ol class="connection-readiness" aria-label="连接准备项">
        <li>
          <span>1</span>
          <div>
            <h3>设备与本次授权</h3>
            <p>需要真实设备身份及对端明确批准。</p>
          </div>
          <em>尚未接入</em>
        </li>
        <li>
          <span>2</span>
          <div>
            <h3>信令与 ICE 协商</h3>
            <p>直连或中继路径由实际协商结果确定。</p>
          </div>
          <em>未开始</em>
        </li>
        <li>
          <span>3</span>
          <div>
            <h3>视频与控制通道</h3>
            <p>连接后才可显示真实画面与通道统计。</p>
          </div>
          <em>未开始</em>
        </li>
      </ol>
      <p id="connection-start-unavailable" class="empty-detail">
        连接服务尚未接入；查看此弹窗不会请求授权、开始采集或联系对端。
      </p>
    </div>
    <footer class="dialog-footer">
      <button disabled aria-describedby="connection-start-unavailable">
        发起连接
      </button>
      <button class="outline-button" @click="dialog?.close()">关闭</button>
    </footer>
  </dialog>
</template>
