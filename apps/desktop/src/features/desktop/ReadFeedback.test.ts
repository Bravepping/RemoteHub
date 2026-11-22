import { mount } from '@vue/test-utils';
import { expect, it } from 'vitest';
import ReadFeedback from './ReadFeedback.vue';
it('announces only errors as alerts, and exposes cancel/retry at the matching states', async () => {
  const w = mount(ReadFeedback, {
    props: { status: 'loading', message: 'loading-fixture' },
  });
  expect(w.get('[role="status"]').text()).toContain('loading-fixture');
  await w.get('button').trigger('click');
  expect(w.emitted('cancel')).toHaveLength(1);
  await w.setProps({ status: 'error', message: 'fixed-error-fixture' });
  expect(w.get('[role="alert"]').text()).toContain('fixed-error-fixture');
  await w.get('button').trigger('click');
  expect(w.emitted('retry')).toHaveLength(1);
  await w.setProps({ status: 'cancelled', message: 'cancel-fixture' });
  expect(w.find('[role="alert"]').exists()).toBe(false);
  expect(w.get('button').text()).toBe('重试读取');
  await w.setProps({ status: 'ready', message: 'ready-fixture' });
  expect(w.find('button').exists()).toBe(false);
  w.unmount();
});
