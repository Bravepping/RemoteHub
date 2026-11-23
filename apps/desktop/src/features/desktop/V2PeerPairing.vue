<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core';
import { computed, onBeforeUnmount, ref, watch } from 'vue';
import PanelCard from './PanelCard.vue';

const cardPattern = /^RH2-[0-9a-f]{64}$/;
const fingerprintPattern = /^[0-9a-f]{64}$/;
const prefillKey = 'remotehub.v2.peer-form-prefill.v1';
type PublicPeerForm = { card: string; fingerprint: string };
const saved = ref(readPrefill());
const cardField = ref(saved.value?.card ?? '');
const fingerprintField = ref(saved.value?.fingerprint ?? '');
const consent = ref(false);
const busy = ref<'read' | 'verify'>();
const copying = ref(false);
const local = ref<{ card: string; fingerprint: string }>();
const verified = ref<string>();
const error = ref('');
const copyFeedback = ref('');
const prefillFeedback = ref('');
let alive = true;
let revision = 0;

const card = computed(() =>
  cardField.value.length <= 100 ? cardField.value.trim() : '',
);
const fingerprint = computed(() =>
  fingerprintField.value.length <= 100
    ? fingerprintField.value.trim().toLowerCase()
    : '',
);
const valid = computed(
  () =>
    cardPattern.test(card.value) && fingerprintPattern.test(fingerprint.value),
);
const alreadySaved = computed(
  () =>
    saved.value?.card === card.value &&
    saved.value?.fingerprint === fingerprint.value,
);

watch(
  [cardField, fingerprintField],
  () => {
    revision++;
    consent.value = false;
    verified.value = undefined;
    error.value = '';
    prefillFeedback.value = '';
  },
  { flush: 'sync' },
);
onBeforeUnmount(() => {
  alive = false;
});

const messages: Readonly<Record<string, string>> = {
  V2_PAIRING_DISABLED: '当前客户端未启用配对验证。',
  V2_PAIRING_IDENTITY_UNAVAILABLE: '本机设备身份不可用。',
  V2_PAIRING_CONSENT_REQUIRED: '请先核对完整指纹并确认。',
  V2_PAIRING_INVALID_CARD: '配对卡格式无效，请向对方重新获取。',
  V2_PAIRING_FINGERPRINT_MISMATCH:
    '配对卡与完整指纹不匹配。请通过可信渠道重新核对。',
  V2_PAIRING_SELF: '不能把本机配对卡当作对端。',
  V2_PAIRING_BUSY: '另一项配对操作正在进行，请稍后重试。',
  V2_PAIRING_DENIED: '没有批准本次本机配对验证。',
};

function message(cause: unknown): string {
  return typeof cause === 'string' && Object.hasOwn(messages, cause)
    ? messages[cause]
    : '配对操作未完成，请检查客户端后重试。';
}

function strictStrings(value: unknown, keys: string[]): Record<string, string> {
  if (
    !value ||
    typeof value !== 'object' ||
    ![Object.prototype, null].includes(Object.getPrototypeOf(value)) ||
    Object.keys(value).length !== keys.length
  )
    throw new Error();
  const record = value as Record<string, unknown>;
  if (
    keys.some(
      (key) => !Object.hasOwn(record, key) || typeof record[key] !== 'string',
    )
  )
    throw new Error();
  return record as Record<string, string>;
}

function readPrefill(): PublicPeerForm | undefined {
  try {
    const raw = window.localStorage.getItem(prefillKey);
    if (!raw || raw.length > 256) return;
    const { card, fingerprint } = strictStrings(JSON.parse(raw), [
      'card',
      'fingerprint',
    ]);
    if (cardPattern.test(card) && fingerprintPattern.test(fingerprint))
      return { card, fingerprint };
  } catch {
    // Unavailable or edited storage must never prevent manual verification.
  }
}

function savePrefill() {
  if (
    !alive ||
    busy.value ||
    !valid.value ||
    verified.value !== fingerprint.value
  )
    return;
  const entry = { card: card.value, fingerprint: fingerprint.value };
  try {
    window.localStorage.setItem(prefillKey, JSON.stringify(entry));
    saved.value = entry;
    prefillFeedback.value = '已保存这两项公开信息，下次自动填入。';
  } catch {
    prefillFeedback.value = '无法保存预填信息，仍可手动验证。';
  }
}

function clearPrefill() {
  if (!alive || busy.value) return;
  try {
    window.localStorage.removeItem(prefillKey);
    saved.value = undefined;
    prefillFeedback.value = '已清除预填信息，下次不会自动填入。';
  } catch {
    prefillFeedback.value = '暂时无法清除预填信息。';
  }
}

async function readLocal() {
  if (!alive || busy.value) return;
  busy.value = 'read';
  local.value = undefined;
  error.value = '';
  copyFeedback.value = '';
  try {
    const result = await invoke<unknown>('get_v2_pairing_card');
    if (!alive) return;
    const { card, fingerprint } = strictStrings(result, [
      'card',
      'fingerprint',
    ]);
    if (
      card.length !== 68 ||
      !cardPattern.test(card) ||
      fingerprint.length !== 64 ||
      !fingerprintPattern.test(fingerprint)
    )
      throw new Error();
    local.value = { card, fingerprint };
  } catch (cause) {
    if (alive) error.value = message(cause);
  } finally {
    if (alive) busy.value = undefined;
  }
}

async function copyPublic(field: 'card' | 'fingerprint') {
  const value = local.value?.[field];
  if (!alive || !value || copying.value) return;
  copying.value = true;
  copyFeedback.value = '';
  try {
    await navigator.clipboard.writeText(value);
    if (alive)
      copyFeedback.value =
        field === 'card' ? '已复制本机公开配对卡。' : '已复制本机完整指纹。';
  } catch {
    if (alive) copyFeedback.value = '无法自动复制，可选中文本手动复制。';
  } finally {
    if (alive) copying.value = false;
  }
}

async function verify() {
  if (!alive || busy.value || !consent.value || !valid.value) return;
  const requested = {
    card: card.value,
    fingerprint: fingerprint.value,
    consent: true,
  };
  const expectedRevision = revision;
  busy.value = 'verify';
  consent.value = false;
  verified.value = undefined;
  error.value = '';
  try {
    const result = await invoke<unknown>('verify_v2_peer', requested);
    if (!alive || expectedRevision !== revision) return;
    const { fingerprint } = strictStrings(result, ['fingerprint']);
    if (fingerprint !== requested.fingerprint) throw new Error();
    verified.value = fingerprint;
  } catch (cause) {
    if (alive && expectedRevision === revision) error.value = message(cause);
  } finally {
    if (alive) busy.value = undefined;
  }
}
</script>

<template>
  <PanelCard title="V2 手动验证对端" icon="shield" label="仅本地">
    <p class="card-description">
      把本机公开配对卡交给对方，并通过另一条可信渠道核对完整 SHA-256 公钥指纹。
      验证结果只保留到退出应用，不会联网、自动连接或授予远控权限。
    </p>
    <button class="outline-button" :disabled="!!busy" @click="readLocal">
      读取本机公开配对卡
    </button>
    <dl v-if="local" class="local-card">
      <dt>本机公开配对卡</dt>
      <dd>
        <code>{{ local.card }}</code>
        <button
          class="outline-button"
          :disabled="copying"
          @click="copyPublic('card')"
        >
          复制配对卡
        </button>
      </dd>
      <dt>本机完整 SHA-256 指纹</dt>
      <dd>
        <code>{{ local.fingerprint }}</code>
        <button
          class="outline-button"
          :disabled="copying"
          @click="copyPublic('fingerprint')"
        >
          复制指纹
        </button>
      </dd>
    </dl>
    <p v-if="copyFeedback" role="status">{{ copyFeedback }}</p>
    <form class="peer-form" @submit.prevent="verify">
      <label for="v2-peer-card">对端公开配对卡</label>
      <input
        id="v2-peer-card"
        v-model="cardField"
        :disabled="busy === 'verify'"
        autocomplete="off"
        spellcheck="false"
      />
      <label for="v2-peer-fingerprint">另行核对的对端完整 SHA-256 指纹</label>
      <input
        id="v2-peer-fingerprint"
        v-model="fingerprintField"
        :disabled="busy === 'verify'"
        autocomplete="off"
        spellcheck="false"
      />
      <p class="card-description">
        {{
          alreadySaved
            ? '已填入本机保存的公开信息。这不代表已核验；若对方设备或指纹变了，请清除预填并重新核对。'
            : '不要从配对卡推算指纹。请向对方另行核对完整 64 位十六进制指纹。'
        }}
      </p>
      <p v-if="cardField && !cardPattern.test(card)">
        配对卡格式为 RH2- 加 64 位小写十六进制字符。
      </p>
      <p v-if="fingerprintField && !fingerprintPattern.test(fingerprint)">
        请填写完整 64 位十六进制指纹。
      </p>
      <label class="consent">
        <input v-model="consent" type="checkbox" :disabled="!valid || !!busy" />
        我已通过独立可信渠道和对方核对完整指纹，同意本次仅做本机验证。
      </label>
      <button
        type="submit"
        class="outline-button"
        :disabled="!!busy || !valid || !consent"
      >
        验证此对端
      </button>
    </form>
    <p v-if="busy === 'verify'" role="status">请在原生确认窗口再次核对指纹。</p>
    <p v-if="verified" role="status">
      已在本机核对该对端指纹：<code>{{ verified }}</code
      >。尚未连接或授权远控。
    </p>
    <button
      v-if="verified && !alreadySaved"
      class="outline-button"
      type="button"
      :disabled="!!busy"
      @click="savePrefill"
    >
      下次预填此设备
    </button>
    <button
      v-if="saved"
      class="outline-button"
      type="button"
      :disabled="!!busy"
      @click="clearPrefill"
    >
      清除预填信息
    </button>
    <p v-if="prefillFeedback" class="card-description">{{ prefillFeedback }}</p>
    <p v-if="error" role="alert">{{ error }}</p>
  </PanelCard>
</template>

<style scoped>
.local-card,
.peer-form {
  display: grid;
  gap: 8px;
  margin: 14px 0;
}
.local-card dd {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 8px;
  margin: 0 0 6px;
}
.peer-form input:not([type='checkbox']) {
  width: 100%;
  padding: 8px;
  border: 1px solid #ccd5d1;
  border-radius: 6px;
}
.peer-form button {
  justify-self: start;
}
.consent {
  display: flex;
  gap: 8px;
  align-items: start;
}
code {
  overflow-wrap: anywhere;
  user-select: text;
}
</style>
