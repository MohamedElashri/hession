// Browser preview: stands in for the Tauri runtime so the UI runs in a normal
// browser with realistic data (`make preview`, or `npm run dev`, opened in a browser).
// Responses come from fixtures.json, which the real backend generates
// (`make preview-fixtures`), or from your own library via
// `make preview-fixtures FROM=~/.hession/sessions.sqlite` (fixtures.local.json,
// git-ignored), which is preferred when present. Loaded only in development builds.
import sample from './fixtures.json';
import appPackage from '../../package.json' with { type: 'json' };

// Fetched rather than imported: a real library can be tens of megabytes, more
// than Vite will transform into a module.
const local = await fetch(new URL('./fixtures.local.json', import.meta.url))
  .then((response) => (response.ok ? response.json() : null))
  .catch(() => null);
const fixtures = local ?? sample;

const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const state = {
  sessions: fixtures.sessions.map((s) => ({ ...s })),
  hosts: (fixtures.facets.hosts || []).map((h) => ({ name: h.value, enabled: true, lastSync: fixtures.generatedAt, agents: ['claude', 'codex'] })),
  terminal: 'gnome-terminal',
};
let callbacks = 0;

function search(args) {
  const q = (args.q || '').toLowerCase();
  const models = (args.model || '').split(',').filter(Boolean);
  const tools = (args.tool || '').split(',').filter(Boolean);
  let rows = state.sessions.filter((s) => {
    const detail = fixtures.details[s.id];
    if (args.host === '@local' ? s.host : args.host && s.host !== args.host) return false;
    if (args.agent && s.agent !== args.agent) return false;
    if (args.project && s.project !== args.project) return false;
    if (args.branch && !String(s.branch || '').includes(args.branch)) return false;
    if (models.length && !models.includes(s.model)) return false;
    if (tools.length && !detail?.entries.some((e) => (e.tools || []).some((t) => tools.includes(t.tool_name)))) return false;
    if (args.starred === 'true' && !s.starred) return false;
    if ((args.tags || []).some((t) => !(s.tags || []).includes(t))) return false;
    if (args.since && new Date(s.last_event_at) < new Date(args.since)) return false;
    if (q && !`${s.title} ${s.cwd} ${detail?.entries.map((e) => e.parts?.map((p) => p.text).join(' ')).join(' ')}`.toLowerCase().includes(q)) return false;
    return true;
  });
  const time = (s) => new Date(s.last_event_at || s.started_at).getTime();
  const order = { oldest: (a, b) => time(a) - time(b), messages: (a, b) => b.entry_count - a.entry_count, longest: (a, b) => b.duration_days - a.duration_days };
  rows = rows.sort(order[args.sort] || ((a, b) => time(b) - time(a)));
  const offset = Number(args.offset || 0);
  return { sessions: rows.slice(offset, offset + Number(args.limit || 20)), total: rows.length };
}

function annotate({ id, starred, tags }) {
  const session = state.sessions.find((s) => String(s.id) === String(id));
  if (starred !== undefined) session.starred = starred;
  if (tags) session.tags = tags;
  fixtures.details[session.id].session = { ...fixtures.details[session.id].session, starred: session.starred, tags: session.tags };
  return { starred: session.starred, tags: session.tags };
}

function resumeInfo({ id }) {
  const s = fixtures.details[id].session;
  const cmd = s.agent === 'codex' ? `codex resume ${s.external_id}` : `claude --resume ${s.external_id}`;
  return {
    ok: true, family: s.agent, host: s.host || null, terminal: 'GNOME Terminal', terminalId: state.terminal,
    cwd: s.host ? '/home/dev' : s.cwd, cwdExists: true, cliFound: true, sourceExists: true, warnings: [],
    displayCommand: s.host ? `ssh -t ${s.host} 'cd ${s.cwd} && ${cmd}'` : `cd ${s.cwd} && ${cmd}`,
  };
}

const handlers = {
  get_agents: () => fixtures.agents,
  get_projects: () => fixtures.projects,
  get_stats: () => fixtures.stats,
  get_facets: () => ({ ...fixtures.facets, tags: [...new Set(state.sessions.flatMap((s) => s.tags || []))].sort() }),
  get_sync_checkpoints: () => fixtures.checkpoints,
  get_sync_status: () => ({ running: false, phase: 'complete', filesDone: 0, filesTotal: 0, errors: [] }),
  start_sync: () => ({ running: false, phase: 'complete', filesDone: 12, filesTotal: 12, errors: [] }),
  stop_sync: () => true,
  search_sessions: search,
  get_session: ({ id }) => fixtures.details[id] ?? Promise.reject('Session not found'),
  update_session_annotations: annotate,
  get_analytics: () => fixtures.analytics,
  refresh_analytics: () => ({ ok: true }),
  get_pricing_status: () => fixtures.pricingStatus,
  get_pricing_overrides: () => fixtures.pricingOverrides,
  sync_pricing: () => Promise.reject('Price download is disabled in the preview.'),
  list_terminals: () => ({ terminals: [{ id: 'gnome-terminal', label: 'GNOME Terminal', available: true }, { id: 'kitty', label: 'Kitty', available: false }], defaultId: 'gnome-terminal', saved: state.terminal }),
  set_terminal_preference: ({ terminal }) => { state.terminal = terminal; return { ok: true }; },
  get_resume_info: resumeInfo,
  resume_session: () => ({ ok: false, message: 'Resuming is disabled in the browser preview.' }),
  open_in_editor: () => Promise.reject('Opening VS Code is disabled in the browser preview.'),
  app_version: () => ({ name: 'Hession', version: appPackage.version }),
  check_for_update: async () => {
    await delay(600);
    return { current: appPackage.version, latest: '0.2.0', tag: 'v0.2.0', newer: true, publishedAt: new Date().toISOString(), url: '#',
      notes: '### Features\n- A sample release note for the preview.\n\n### Fixes\n- Another sample entry.' };
  },
  update_command: ({ tag }) => `curl -fsSL https://raw.githubusercontent.com/example/app/${tag}/install.sh | sh -s -- --version ${tag}`,
  run_update: () => Promise.reject('Updating is disabled in the browser preview.'),
  open_release_page: () => Promise.reject('Opening the browser is disabled in the preview.'),
  get_remote_hosts: () => ({ hosts: state.hosts, suggestions: [{ name: 'staging', source: 'vscode' }, { name: 'ci-runner', source: 'ssh-config' }], tools: { ssh: true, rsync: true } }),
  set_remote_hosts: ({ hosts }) => { state.hosts = hosts.map((h) => ({ ...h, lastSync: null, agents: [], ...state.hosts.find((x) => x.name === h.name), enabled: h.enabled })); return { ok: true }; },
  test_remote_host: async ({ name }) => { await delay(700); return { ok: true, home: `/home/${name}`, rsync: true, agents: ['claude'] }; },
  preview_saved_data: () => ({ sessions: state.sessions.length, entries: fixtures.stats.entries, tools: 14, totalSessions: state.sessions.length, databaseBytes: 2_400_000, databasePath: '~/.hession/sessions.sqlite', token: 'preview' }),
  'plugin:event|listen': () => callbacks++,
  'plugin:event|unlisten': () => null,
};

window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };

window.__TAURI_INTERNALS__ = {
  metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
  transformCallback: () => ++callbacks,
  async invoke(command, args = {}) {
    await delay(60);
    const handler = handlers[command];
    if (!handler) throw `${command} is not available in the browser preview.`;
    return handler(args);
  },
};
console.info(`[preview] Mock IPC with ${local ? 'your library' : 'sample data'} from`, fixtures.generatedAt);
