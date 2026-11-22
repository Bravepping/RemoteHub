import { createPinia, disposePinia } from 'pinia';
import type { App, InjectionKey } from 'vue';
import type { DesktopApi } from '../ipc/contracts';
import { tauriDesktopApi } from '../ipc/tauriDesktopApi';

export const desktopApiKey: InjectionKey<DesktopApi> = Symbol('desktopApi');
export function createDesktopState(api: DesktopApi = tauriDesktopApi) {
  const pinia = createPinia();
  return {
    pinia,
    install(app: App) {
      app.provide(desktopApiKey, api);
      app.use(pinia);
      app.onUnmount(() => disposePinia(pinia));
    },
  };
}
