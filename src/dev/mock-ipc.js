// Browser preview: stands in for the Tauri runtime so the UI runs in a normal
// browser with realistic data (`make preview`, or `npm run dev`, opened in a browser).
// Responses come from fixtures.json, which the real backend generates
// (`make preview-fixtures`), or from your own library via
// `make preview-fixtures FROM=~/.hession/sessions.sqlite` (fixtures.local.json,
// git-ignored), which is preferred when present. Loaded only in development builds.
import sample from './fixtures.json';
import appPackage from '../../package.json' with { type: 'json' };
import { liveEntrySummary } from '../live-state.js';

// Fetched rather than imported: a real library can be tens of megabytes, more
// than Vite will transform into a module.
const local = await fetch(new URL('./fixtures.local.json', import.meta.url))
  .then((response) => (response.ok ? response.json() : null))
  .catch(() => null);
const fixtures = local ?? sample;

function recentLocal({ agent = '', project = '', since = '', limit = 50, offset = 0 }) {
  const rows = fixtures.sessions.filter(session => !session.host && !String(session.source_uri || '').startsWith('hession-sync://')
    && (!agent || session.agent === agent) && (!project || (session.project || session.cwd) === project)
    && (!since || new Date(session.last_event_at || session.started_at) >= new Date(since)))
    .sort((a, b) => new Date(b.last_event_at || b.started_at) - new Date(a.last_event_at || a.started_at) || b.id - a.id);
  const selected = rows.slice(offset, offset + limit).map(session => {
    const entries = fixtures.details[session.id]?.entries || [], latest = entries.at(-1);
    return { id: session.id, title: session.title, agent: session.agent, product: session.product,
      project: session.project || session.cwd, workspaceId: null, lastEventAt: session.last_event_at || session.started_at,
      status: 'stale', latestEvent: latest ? { id: latest.id, ordinal: latest.ordinal, position: entries.length - 1,
        kind: latest.kind, role: latest.role, summary: liveEntrySummary(latest).slice(0, 240), occurredAt: latest.occurred_at } : null };
  });
  const hasMore = offset + selected.length < rows.length;
  return { sessions: selected, hasMore, nextOffset: hasMore ? offset + limit : null, revision: { identity: 'browser-preview', revision: 0 } };
}

function sessionDelta({ sessionId, cursor, limit = 100 }) {
  const detail = fixtures.details[sessionId];
  const rows = (detail?.entries || []).map((entry, position) => ({ ...entry, position }));
  const current = { identity: 'browser-preview', epoch: 0, seq: rows.length, sessionId };
  const reset = !cursor || JSON.stringify(cursor) !== JSON.stringify(current);
  return { revision: { identity: 'browser-preview', revision: 0 }, cursor: current, reset, deleted: !detail,
    entries: reset ? rows.slice(-Math.min(100, limit)) : [], hasMore: false };
}

// Project preview uses only recorded fixture metadata and structured path arguments.
// It never inspects the browser host's filesystem or invents verified Git changes.
function previewProjects() {
  return fixtures.projects.map((project, index) => {
    const sessions = fixtures.sessions.filter(session => session.project === project.path);
    return { workspace_id: index + 1, project_key: `preview:${project.path}`, recorded_path: project.path,
      display_name: project.display_name, host: sessions.find(session => session.host)?.host || null,
      ambiguous_host: new Set(sessions.map(session => session.host || '')).size > 1,
      session_count: sessions.length, local_session_count: sessions.filter(session => !session.host).length,
      remote_session_count: sessions.filter(session => session.host).length, synced_session_count: 0 };
  });
}
function previewReferences(workspaceId) {
  const project = previewProjects().find(project => project.workspace_id === workspaceId);
  if (!project) throw 'Recorded workspace unavailable';
  const evidence = [], entry_positions = {};
  for (const session of fixtures.sessions.filter(session => session.project === project.recorded_path)) {
    for (const [position, entry] of (fixtures.details[session.id]?.entries || []).entries()) {
      entry_positions[entry.id] = position;
      for (const tool of entry.tools || []) {
        let args; try { args = JSON.parse(tool.arguments_json || '{}'); } catch { continue; }
        for (const key of ['file_path', 'filepath', 'path', 'target_file', 'filename']) {
          if (typeof args[key] !== 'string') continue;
          const category = /write|edit|patch|delete/i.test(tool.tool_name) ? 'reported_write_intent' : /read|view|open_file/i.test(tool.tool_name) ? 'reported_read' : 'reported_reference';
          evidence.push({ id: `${tool.id}:${key}`, reported_path: args[key], category,
            source: { session_id: session.id, entry_id: entry.id, entry_ordinal: entry.ordinal, tool_invocation_id: tool.id },
            candidate_workspace_relative_path: null, occurred_at: entry.occurred_at, agent_family: session.agent, tool_name: tool.tool_name });
        }
      }
    }
  }
  return { project, evidence, entry_positions, revision: { identity: 'browser-preview', revision: 0 } };
}
function previewPage(rows, page = { limit: 20, offset: 0 }) {
  return { rows: rows.slice(page.offset, page.offset + page.limit), has_more: rows.length > page.offset + page.limit,
    next_offset: rows.length > page.offset + page.limit ? page.offset + page.limit : null };
}
function previewActivity({ workspaceId, reportedPath, page }) {
  const result = previewReferences(workspaceId);
  const selected = result.evidence.filter(item => reportedPath == null || item.reported_path === reportedPath);
  const paged = previewPage(selected, page);
  return { ...result, evidence: paged.rows, has_more: paged.has_more, next_offset: paged.next_offset };
}

const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const state = {
  sessions: fixtures.sessions.map((s) => ({ ...s })),
  hosts: (fixtures.facets.hosts || []).map((h) => ({ name: h.value, enabled: true, lastSync: fixtures.generatedAt, agents: ['claude', 'codex'] })),
  terminal: 'default',
  customPath: null,
  customArgs: null,
  optionalFeatures: { continuationEnabled: false },
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

function resumeInfo({ id, terminal }) {
  const chosen = terminal || state.terminal;
  const resolved = chosen === 'default' ? 'gnome-terminal' : chosen;
  const s = fixtures.details[id].session;
  const cmd = s.agent === 'codex' ? `codex resume ${s.external_id}` : `claude --resume ${s.external_id}`;
  return {
    ok: true, family: s.agent, host: s.host || null, terminal: resolved === 'custom' ? 'Custom terminal' : 'GNOME Terminal', terminalId: resolved,
    cwd: s.host ? '/home/dev' : s.cwd, cwdExists: true, cliFound: true, sourceExists: true, warnings: [],
    displayCommand: s.host ? `ssh -t ${s.host} 'cd ${s.cwd} && ${cmd}'` : `cd ${s.cwd} && ${cmd}`,
  };
}

const handlers = {
  recent_local_sessions: recentLocal,
  session_delta: sessionDelta,
  get_optional_features: () => ({ ...state.optionalFeatures }),
  set_continuation_enabled: ({ enabled }) => { state.optionalFeatures.continuationEnabled = !!enabled; return { ...state.optionalFeatures }; },
  get_device_sync_settings: () => ({ enabled: false, configured: false, relayUrl: '', deviceName: '', scope: [], workspaces: fixtures.projects.map(project => ({ path: project.path, count: project.sessions || 0 })), status: { running: false, phase: 'disabled', uploaded: 0, downloaded: 0 } }),
  preview_device_sync: ({ workspaces }) => {
    const selected = state.sessions.filter(session => workspaces.includes(session.project || session.cwd));
    return { sessions: selected.length, entries: selected.reduce((sum, session) => sum + (session.entry_count || 0), 0), estimatedBytes: selected.length * 4096 };
  },
  test_device_sync_relay: () => Promise.reject('Relay connections are disabled in the browser preview. Use the desktop app to configure private sync.'),
  setup_device_sync: () => Promise.reject('Library creation is disabled in the browser preview.'),
  join_device_sync: () => Promise.reject('Device pairing is disabled in the browser preview.'),
  get_continuation_info: ({ id, target, cwd, note, includeTools }) => {
    if (!state.optionalFeatures.continuationEnabled) throw 'Enable cross-harness continuation in Settings first.';
    const detail = fixtures.details[id];
    if (!detail) throw 'Session not found';
    const session = detail.session;
    const text = detail.entries.filter(entry => ['user', 'assistant'].includes(entry.role))
      .slice(-12).map(entry => `${entry.role}: ${(entry.parts || []).filter(part => part.kind === 'text').map(part => part.text).join('\n')}`).join('\n\n');
    return {
      source: { id, family: session.agent, title: session.title, cwd: session.cwd, host: session.host, importedAt: fixtures.generatedAt },
      workspace: { path: cwd || session.cwd, exists: true, repositoryRoot: cwd || session.cwd, branch: session.branch || 'main', commit: 'preview', dirty: false, status: '' },
      target, checkpoint: 'browser-preview', content: `# Hession handoff preview\n\n${note || ''}\n\n${text}`,
      omissions: ['Images, private reasoning, permissions, and running processes are omitted.', ...(includeTools ? ['This browser preview does not include tool payloads.'] : ['Tool inputs and outputs are excluded.'])],
      warnings: ['Browser preview: launching real harnesses is disabled.'], cliFound: true, cliVersion: 'preview', canLaunch: false,
      nativeConversionAvailable: false, providerNotice: 'The selected harness may send the reviewed context to its model provider when launched in the desktop app.',
    };
  },
  launch_continuation: () => Promise.reject('Launching a harness is disabled in the browser preview.'),
  list_continuation_packages: () => ({ packages: [] }),
  cleanup_continuation_packages: () => ({ removed: 0 }),
  get_agents: () => fixtures.agents,
  get_projects: () => fixtures.projects,
  project_catalog: ({ page, projectFilter }) => {
    const all = previewProjects().filter(project => !projectFilter || `${project.recorded_path} ${project.display_name}`.toLowerCase().includes(projectFilter.toLowerCase()));
    const paged = previewPage(all, page); return { projects: paged.rows, has_more: paged.has_more, next_offset: paged.next_offset };
  },
  project_activity: previewActivity,
  file_history: previewActivity,
  project_files: ({ workspaceId, filenameFilter, page }) => {
    const data = previewReferences(workspaceId), files = new Map(), seen = new Set();
    for (const item of data.evidence) {
      const key = `${item.source.session_id}:${item.source.entry_id}:${item.source.tool_invocation_id}:${item.reported_path}:${item.category}`;
      if (seen.has(key) || filenameFilter && !item.reported_path.toLowerCase().includes(filenameFilter.toLowerCase())) continue;
      seen.add(key);
      const file = files.get(item.reported_path) || { reported_path: item.reported_path, reported_read_count: 0, reported_write_intent_count: 0, reported_reference_count: 0, sessions: new Set() };
      file[`${item.category}_count`]++; file.sessions.add(item.source.session_id); files.set(item.reported_path, file);
    }
    const paged = previewPage([...files.values()].map(file => ({ ...file, session_count: file.sessions.size })), page);
    return { project: data.project, files: paged.rows, has_more: paged.has_more, next_offset: paged.next_offset };
  },
  recorded_git: ({ workspaceId, page }) => {
    const data = previewReferences(workspaceId);
    const contexts = fixtures.sessions.filter(session => session.project === data.project.recorded_path && session.branch).map(session => ({ context_id: session.id, branch: session.branch, commit_sha: null, observed_at: null, source: { session_id: session.id, entry_id: null } }));
    const paged = previewPage(contexts, page); return { contexts: paged.rows, entry_positions: {}, has_more: paged.has_more, next_offset: paged.next_offset };
  },
  inspect_git: () => Promise.reject('Local Git inspection is available only in the desktop app.'),
  get_stats: () => fixtures.stats,
  get_facets: () => ({ ...fixtures.facets, tags: [...new Set(state.sessions.flatMap((s) => s.tags || []))].sort() }),
  get_sync_checkpoints: () => fixtures.checkpoints,
  get_sync_status: () => ({ running: false, phase: 'complete', filesDone: 0, filesTotal: 0, errors: [] }),
  start_sync: () => ({ running: false, phase: 'complete', filesDone: 12, filesTotal: 12, errors: [] }),
  stop_sync: () => true,
  search_sessions: search,
  developer_search: () => Promise.reject({ kind: 'preview', message: 'Developer search uses the desktop SQLite service. Switch to Legacy search in this browser preview.' }),
  get_session: ({ id, limit = '500', page = '1' }) => {
    const detail = fixtures.details[id];
    if (!detail) throw 'Session not found';
    const offset = (Number(page) - 1) * Number(limit);
    return { ...detail, entries: detail.entries.slice(offset, offset + Number(limit)), offset, limit: Number(limit) };
  },
  search_session_entries: ({ id, q, limit = '20', offset = '0' }) => {
    const detail = fixtures.details[id];
    if (!detail) throw 'Session not found';
    const terms = [...new Set(q.toLowerCase().match(/[\p{L}\p{N}_]+/gu) || [])];
    const entries = detail.entries.flatMap((entry, position) => {
      const text = entry.search_text || '';
      if (!(terms.length ? terms.some(term => text.toLowerCase().includes(term)) : text.includes(q.trim()))) return [];
      return [{ ...entry, position, excerpt: text.slice(0, 600) }];
    });
    return { entries: entries.slice(Number(offset), Number(offset) + Number(limit)), total: entries.length };
  },
  update_session_annotations: annotate,
  get_analytics: () => fixtures.analytics,
  refresh_analytics: () => ({ ok: true }),
  get_pricing_status: () => fixtures.pricingStatus,
  get_pricing_overrides: () => fixtures.pricingOverrides,
  sync_pricing: () => Promise.reject('Price download is disabled in the preview.'),
  list_terminals: () => ({ customPath: state.customPath, customArgs: state.customArgs, terminals: [{ id: 'default', label: 'Default terminal (automatic)', available: true }, ...(state.customPath ? [{ id: 'custom', label: 'Custom terminal', available: true }] : []), { id: 'gnome-terminal', label: 'GNOME Terminal', available: true }, { id: 'kitty', label: 'Kitty', available: false }], defaultId: 'gnome-terminal', saved: state.terminal }),
  set_terminal_preference: ({ terminal, customPath, customArgs }) => { state.terminal = terminal; if (customPath !== undefined) state.customPath = customPath; if (customArgs !== undefined) state.customArgs = customArgs; return { ok: true }; },
  'plugin:dialog|open': () => '/opt/my terminal/bin/terminal',
  get_resume_info: resumeInfo,
  resume_session: () => ({ ok: false, message: 'Resuming is disabled in the browser preview.' }),
  open_in_editor: () => Promise.reject('Opening VS Code is disabled in the browser preview.'),
  app_version: () => ({ name: 'Hession', version: appPackage.version, supported: true, needsAuthorization: false }),
  'plugin:updater|check': async () => {
    await delay(600);
    const [major, minor, patch] = appPackage.version.split('.').map(Number);
    return { rid: 1, currentVersion: appPackage.version, version: `${major}.${minor}.${patch + 1}`, date: new Date().toISOString(),
      body: '### Features\n- Signed updates directly in Hession.\n- Download progress and restart when you’re ready.\n\n### Fixes\n- Update state is kept when you close this dialog.' };
  },
  'plugin:updater|download': async ({ onEvent }) => {
    onEvent.onmessage({ event: 'Started', data: { contentLength: 25 * 1048576 } });
    for (let step = 0; step < 10; step++) {
      await delay(200);
      onEvent.onmessage({ event: 'Progress', data: { chunkLength: 2.5 * 1048576 } });
    }
    onEvent.onmessage({ event: 'Finished' });
    await delay(400);
    return 2;
  },
  'plugin:updater|install': () => delay(700),
  'plugin:resources|close': () => null,
  restart_after_update: () => Promise.reject('This is a browser preview. No update was installed; restart is available in the desktop app.'),
  open_release_page: () => Promise.reject('Opening the browser is disabled in the preview.'),
  get_remote_hosts: () => ({ hosts: state.hosts, suggestions: [{ name: 'staging', source: 'vscode' }, { name: 'ci-runner', source: 'ssh-config' }], tools: { ssh: true, rsync: true } }),
  set_remote_hosts: ({ hosts }) => { state.hosts = hosts.map((h) => ({ ...h, lastSync: null, agents: [], ...state.hosts.find((x) => x.name === h.name), enabled: h.enabled })); return { ok: true }; },
  test_remote_host: async ({ name }) => { await delay(700); return { ok: true, home: `/home/${name}`, rsync: true, agents: ['claude'] }; },
  preview_saved_data: () => ({ sessions: state.sessions.length, entries: fixtures.stats.entries, tools: 14, totalSessions: state.sessions.length, databaseBytes: 2_400_000, databasePath: '~/.hession/sessions.sqlite', token: 'preview', options: { agents: [], workspaces: [], tags: [] } }),
  'plugin:event|listen': () => callbacks++,
  'plugin:event|unlisten': () => null,
};

window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };

window.__TAURI_INTERNALS__ = {
  isPreview: true,
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
