import { check } from '@tauri-apps/plugin-updater';
import { api } from './api.js';
import { UpdateController } from './update-controller.js';

export const updates = new UpdateController({
  info: () => api('app_version'),
  check: () => check({ timeout: 20_000 }),
  restart: () => api('restart_after_update'),
});
