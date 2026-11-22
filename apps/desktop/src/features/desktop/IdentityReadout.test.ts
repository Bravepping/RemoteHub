import { mount } from '@vue/test-utils';
import { expect, it } from 'vitest';
import IdentityReadout from './IdentityReadout.vue';
it('keeps absent identity honest, removes stale fields while loading and displays fixed errors', async () => {
  const w = mount(IdentityReadout, { props: { loading: false } });
  expect(w.text()).toContain('尚未读取');
  expect(w.find('code').exists()).toBe(false);
  await w.setProps({
    identity: {
      status: 'ready',
      device_id: 'RH-0123-4567-89AB',
      fingerprint: '0123456789ab'.repeat(5) + '0123',
    },
  });
  expect(w.findAll('code')).toHaveLength(2);
  expect(w.text()).toContain('不代表已联网');
  await w.setProps({ loading: true });
  expect(w.findAll('code')).toHaveLength(0);
  await w.setProps({
    loading: false,
    identity: { status: 'unavailable', error_code: 'IDENTITY_STORE_INVALID' },
  });
  expect(w.get('[role="alert"]').text()).toContain('未自动更换身份');
  expect(w.find('code').exists()).toBe(false);
  w.unmount();
});
