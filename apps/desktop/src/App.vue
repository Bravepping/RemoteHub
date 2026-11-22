<script setup lang="ts">
import { onMounted, ref } from 'vue';
import LoopbackVideoProbe from './features/p004/LoopbackVideoProbe.vue';
import LanVideoProbe from './features/p004/LanVideoProbe.vue';
import RelayVideoProbe from './features/p006/RelayVideoProbe.vue';
import {
  errorMessage,
  getRuntimeInfo,
  runCaptureProbe,
  type ProbeReport,
  type RuntimeInfo,
} from './bridge';

const runtime = ref<RuntimeInfo>();
const report = ref<ProbeReport>();
const status = ref('正在检测本机运行环境…');
const error = ref('');
const consent = ref(false);
const busy = ref(false);

async function refresh() {
  error.value = '';
  try {
    runtime.value = await getRuntimeInfo();
    status.value = '桌面核心已就绪 · 远程访问关闭';
  } catch (cause) {
    status.value = '本机能力未连接';
    error.value = errorMessage(cause);
  }
}

async function capture() {
  if (!consent.value || busy.value || !runtime.value) return;
  busy.value = true;
  error.value = '';
  report.value = undefined;
  try {
    report.value = await runCaptureProbe();
  } catch (cause) {
    error.value = errorMessage(cause);
  } finally {
    busy.value = false;
    consent.value = false;
  }
}

onMounted(refresh);
</script>

<template>
  <div class="shell">
    <aside>
      <div class="brand"><span class="brand-icon">▣</span> RemoteHub</div>
      <div class="nav-item">◈ <span>技术验证</span></div>
      <div class="side-note">自托管 · Windows<br />Phase 0 / 首次工程</div>
      <div class="access-state">
        <span class="dot"></span
        >{{ runtime?.p004_lan_enabled ? '仅本次指定对端' : '远程访问关闭' }}
      </div>
    </aside>
    <main>
      <header>
        <div>
          <p class="eyebrow">REMOTEHUB / ENGINEERING PREVIEW</p>
          <h1>先让真实画面跑起来</h1>
        </div>
        <span class="badge">Phase 0</span>
      </header>
      <p class="intro">
        {{
          runtime?.p004_lan_enabled
            ? runtime?.p006_enabled
              ? runtime?.p006c_enabled
                ? '两台电脑明确确认后，优先直连并在需要时自动中继，验证视频与模拟事件。'
                : '两台电脑明确确认后，经加密中继验证跨网络视频与模拟事件。'
              : '两台电脑明确确认后，验证局域网实时视频。'
            : '当前验证本机窗口与主屏采集，尚未开放设备连接。'
        }}
      </p>
      <RelayVideoProbe
        v-if="runtime?.p006_enabled"
        :direct-enabled="runtime?.p006c_enabled ?? false"
      />
      <LanVideoProbe
        v-else-if="runtime?.p004_lan_enabled"
        :control-enabled="runtime?.p005_enabled ?? false"
      />
      <LoopbackVideoProbe v-else-if="runtime?.p004_enabled" />
      <template v-if="!runtime?.p004_enabled && !runtime?.p006_enabled">
        <section class="runtime-card">
          <div class="monitor">▣</div>
          <div>
            <h2>本机运行环境</h2>
            <p role="status">{{ status }}</p>
            <p v-if="runtime" class="details">
              {{ runtime.os }} / {{ runtime.arch }} · v{{ runtime.app_version }}
              ·
              {{ runtime.capture_backend }}
            </p>
          </div>
          <button class="secondary" :disabled="busy" @click="refresh">
            重新检测
          </button>
        </section>
        <div v-if="error" role="alert" class="error">{{ error }}</div>
        <div class="content-grid">
          <section>
            <span class="step">01 / 本机实验</span>
            <h2>DXGI 主屏采集</h2>
            <p>
              采集最多 30 帧，5
              秒后停止。此窗口仅返回尺寸、时间戳和计数，不保存或发送桌面画面。
            </p>
            <label class="consent"
              ><input
                v-model="consent"
                type="checkbox"
                :disabled="busy || !runtime"
              />我同意进行本次本机主屏采集实验</label
            >
            <button :disabled="!consent || busy || !runtime" @click="capture">
              {{ busy ? '正在采集，最多 5 秒…' : '开始采集实验' }}
            </button>
            <div v-if="report" class="results" aria-label="采集结果">
              <strong>已获得真实 DXGI 帧</strong>
              <dl>
                <dt>主屏尺寸</dt>
                <dd>{{ report.width }} × {{ report.height }}</dd>
                <dt>已采集帧</dt>
                <dd>{{ report.frames }}</dd>
                <dt>观察帧率</dt>
                <dd>{{ report.observed_fps.toFixed(2) }} FPS</dd>
                <dt>超时次数</dt>
                <dd>{{ report.timeouts }}</dd>
                <dt>耗时</dt>
                <dd>{{ report.elapsed_ms }} ms</dd>
                <dt>适配器</dt>
                <dd>{{ report.adapter }}</dd>
              </dl>
              <p>观察帧率按收到的桌面更新计数，静态桌面可能低于目标帧率。</p>
            </div>
          </section>
          <section>
            <span class="step">02 / 关卡说明</span>
            <h2>媒体链路待验收</h2>
            <ol class="pipeline">
              <li>DXGI 捕获主屏</li>
              <li>Media Foundation · H.264</li>
              <li>webrtc-rs → WebView2</li>
              <li>DataChannel 模拟控制消息</li>
              <li>两机直连与 TURN 中继</li>
            </ol>
            <p class="note">
              本窗口不表示视频链路通过。每项状态与实测证据记录在交付包 TASKS.md
              和测试报告中。
            </p>
          </section>
        </div>
      </template>
      <footer>本机显式授权 · 无后台共享 · 无模拟设备或网络指标</footer>
    </main>
  </div>
</template>
