// Keep this entry independent of React and app dependencies so the static
// loading screen remains useful while the main bundle loads or fails.
import { BRAND, READY_EVENT, storageKey } from './brand.js';

const loading = document.getElementById('boot-loading');
const status = document.getElementById('boot-status');
const description = document.getElementById('boot-description');
const reload = document.getElementById('boot-reload');

try {
  if (localStorage.getItem(storageKey('theme')) === 'light') {
    loading.dataset.theme = 'light';
  }
} catch {
  // Storage can be unavailable; the loading screen still works.
}

reload.addEventListener('click', () => window.location.reload());

const slowStart = window.setTimeout(() => {
  if (!loading.isConnected) return;
  status.textContent = 'Still starting…';
  description.textContent = 'This is taking a little longer. You can keep waiting while the app opens.';
}, 12000);

function ready() {
  window.clearTimeout(slowStart);
  window.removeEventListener(READY_EVENT, ready);
}
window.addEventListener(READY_EVENT, ready);

// Browser preview (`make preview`): mock the Tauri runtime. Removed from production builds.
const preview = import.meta.env.DEV && new URLSearchParams(window.location.search).has('mock')
  ? import('./dev/mock-ipc.js')
  : Promise.resolve();

preview.then(() => import('./main.jsx')).catch(error => {
  ready();
  console.error(`[boot] Could not load ${BRAND.name}:`, error);
  if (!loading.isConnected) return;
  loading.dataset.state = 'error';
  loading.setAttribute('aria-busy', 'false');
  status.textContent = `Could not open ${BRAND.name}`;
  description.textContent = 'Something went wrong while starting the app. Please try again.';
  reload.hidden = false;
});
