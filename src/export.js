import { BRAND } from './brand.js';

export function exportFilename(scope, session, count, format, now = new Date()) {
  const safe = value => String(value || '').replace(/[^a-zA-Z0-9_-]+/g, '-').replace(/^-+|-+$/g, '') || 'session';
  const pad = number => String(number).padStart(2, '0');
  const localTime = `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}_${pad(now.getHours())}-${pad(now.getMinutes())}-${pad(now.getSeconds())}`;
  const minutes = -now.getTimezoneOffset();
  const zone = `UTC${minutes >= 0 ? '+' : '-'}${pad(Math.floor(Math.abs(minutes) / 60))}-${pad(Math.abs(minutes) % 60)}`;
  const name = scope === 'session' ? `${safe(session?.agent || session?.product)}-${safe(session?.external_id || session?.id)}` : scope === 'filtered' ? `filtered-${count}` : 'all';
  return `${BRAND.slug}-${name}_${localTime}_${zone}.${format === 'json' ? 'json' : 'md'}`;
}
