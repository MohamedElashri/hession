import { defineConfig } from 'vitest/config';
import react from '@vitejs/plugin-react';

// The existing Node tests keep their runner. These tests exercise rendered
// components in an isolated DOM, with the native IPC boundary mocked.
export default defineConfig({
  plugins: [react()],
  test: {
    environment: 'jsdom',
    include: ['src/**/*.component.test.jsx'],
    setupFiles: ['src/test/setup.js'],
    restoreMocks: true,
    clearMocks: true,
    coverage: {
      provider: 'v8',
      include: [
        'src/components/DeviceSyncSettings.jsx',
        'src/components/SettingsDialog.jsx',
        'src/components/ContinuationDialog.jsx',
        'src/components/FeatureDocumentation.jsx',
        'src/device-sync-settings.js',
      ],
      reporter: ['text', 'json-summary'],
      reportsDirectory: 'coverage/components',
    },
  },
});
