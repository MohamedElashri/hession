export const LIVE_PAGE_SIZE = 100;
export const MAX_LIVE_ENTRIES = 300;
export const MAX_LIVE_BYTES = 4 * 1024 * 1024;
const encoder = new TextEncoder();

export const liveStatusLabel = status => ({ recent: 'Recently updated', quiet: 'Quiet', stale: 'May be stale', unavailable: 'Unavailable' })[status] || 'Status unavailable';

// Same normalized conversation-kind policy as the core search/recent service.
const CONVERSATION_KINDS = new Set(['message', 'text', 'user', 'assistant', 'user_message', 'agent_message', 'task', 'user_feedback',
  'user_feedback_diff', 'completion_result', 'followup', 'plan_mode_respond', 'USER_INPUT']);

export function liveEntrySummary(entry) {
  if (['user', 'assistant'].includes(entry.role) && CONVERSATION_KINDS.has(entry.kind)) {
    const parts = (entry.parts || []).filter(part => part.kind === 'text' && typeof part.text === 'string');
    let summary = '';
    for (const part of parts) { summary = `${summary}${summary ? '\n' : ''}${part.text.slice(0, 400)}`.slice(0, 400); if (summary.length >= 400) break; }
    if (summary) return summary;
  }
  const names = (entry.tools || []).map(tool => typeof tool.tool_name === 'string' ? tool.tool_name.slice(0, 80) : '').filter(Boolean).slice(0, 3);
  return names.length ? `Recorded tool activity: ${names.join(', ')}` : 'Recorded event. Expand to inspect its content.';
}

/** Full replacement on reset, stable-ID updates otherwise; ordinals are ordering, never row positions. */
export function mergeLiveEntries(previous, incoming, reset = false) {
  if (!Array.isArray(incoming) || incoming.length > LIVE_PAGE_SIZE) throw new Error('Invalid live event response.');
  if (!reset && !incoming.length) return { entries: previous, added: 0, updated: 0, trimmed: 0 };
  const before = new Map(previous.map(entry => [String(entry.id), entry]));
  const merged = reset ? new Map() : new Map(before);
  const lastOrdinal = previous.length ? Math.max(...previous.map(entry => entry.ordinal)) : -Infinity;
  let added = 0, updated = 0;
  for (const entry of incoming) {
    if (!entry || !Number.isSafeInteger(entry.id) || entry.id <= 0 || !Number.isSafeInteger(entry.ordinal)) throw new Error('Invalid live event response.');
    const id = String(entry.id), old = reset ? null : before.get(id);
    if (!old && (reset || entry.ordinal > lastOrdinal)) added++;
    else if (!old) updated++;
    else if (JSON.stringify(old) !== JSON.stringify(entry)) updated++;
    merged.set(id, entry);
  }
  const ordered = [...merged.values()].sort((a, b) => a.ordinal - b.ordinal || a.id - b.id);
  const retained = [];
  let bytes = 0;
  for (let i = ordered.length - 1; i >= 0 && retained.length < MAX_LIVE_ENTRIES; i--) {
    const size = encoder.encode(JSON.stringify(ordered[i])).byteLength;
    if (bytes + size > MAX_LIVE_BYTES) break;
    retained.push(ordered[i]); bytes += size;
  }
  return { entries: retained.reverse(), added, updated, trimmed: ordered.length - retained.length };
}

export function createLiveCommands({ openLive, openFiles, observer }) {
  const unavailable = () => !observer?.available ? 'Automatic indexing is available in the desktop app.'
    : observer.busy ? 'An indexing preference is being updated.' : null;
  return [
    { id: 'view.live', title: 'Show live local activity', run: openLive },
    { id: 'view.files', title: 'Show Files and Git', disabled: () => openFiles ? null : 'Open a project with file evidence first.', run: () => openFiles?.() },
    { id: 'indexing.pause', title: 'Pause automatic local indexing', disabled: () => unavailable() || (!observer.enabled ? 'Automatic indexing is already paused.' : null), run: () => observer.toggle() },
    { id: 'indexing.resume', title: 'Resume automatic local indexing', disabled: () => unavailable() || (observer.enabled ? 'Automatic indexing is already enabled.' : null), run: () => observer.toggle() },
  ];
}
