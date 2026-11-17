import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, test, vi } from 'vitest';
import { api } from './api.js';
import ProjectsView from './ProjectsView.jsx';
vi.mock('./api.js', () => ({ api: vi.fn() }));
const local = { workspace_id: 1, project_key: 'project-v1:local', display_name: 'project', recorded_path: '/work/project', host: null, local_session_count: 1, synced_session_count: 0, session_count: 1 };
const remote = { ...local, workspace_id: 2, recorded_path: 'box:/work/project', host: 'box', local_session_count: 0 };
const file = { reported_path: 'src/a.rs', reported_read_count: 1, reported_write_intent_count: 2, reported_reference_count: 0, session_count: 1 };
const event = { id: 10, reported_path: 'src/a.rs', category: 'reported_write_intent', source: { session_id: 7, entry_id: 9, entry_ordinal: 999, tool_invocation_id: 8 }, candidate_workspace_relative_path: 'src/a.rs', agent_family: 'codex', tool_name: 'Write' };
const context = { context_id: 3, source: event.source, branch: 'recorded-branch', commit_sha: 'old-commit', observed_at: 'yesterday' };
const inspection = { canonical_root: '/work/project', worktree_git_dir: '/work/project/.git', branch: 'current-branch', head: 'new-commit', observed_at_unix_ms: 1000, worktree_scan_complete: false, inspected_tracked_paths: 100, skipped_paths: 2, worktree_scan_truncated: true, changes: [{ category: 'verified_commit_change', status: 'R100', commit_sha: 'new-commit', path: { display: 'new\nname.rs', bytes_hex: 'one' }, previous_path: { display: 'old.rs' } }] };
const calls = command => api.mock.calls.filter(([name]) => name === command);
function handler(command, args) {
  if (command === 'project_catalog') return { projects: [local, remote], has_more: false, next_offset: null };
  if (command === 'project_files') return { files: [file], has_more: false, next_offset: null };
  if (command === 'project_activity' || command === 'file_history') return { evidence: [event], entry_positions: { 9: 1 }, has_more: false, next_offset: null };
  if (command === 'recorded_git') return { contexts: [context], entry_positions: { 9: 1 }, has_more: false, next_offset: null };
  if (command === 'inspect_git') return inspection;
  throw new Error(`Unexpected IPC ${command} ${JSON.stringify(args)}`);
}
beforeEach(() => { api.mockReset(); api.mockImplementation(async (command, args) => handler(command, args)); });

test('recorded evidence never inspects Git automatically and links the exact source position', async () => {
  const user = userEvent.setup(); const onSelectSession = vi.fn();
  render(<ProjectsView onSelectSession={onSelectSession}/>);
  await screen.findByText('Reported write intent');
  expect(calls('inspect_git')).toHaveLength(0);
  expect(screen.getByText('Reported write intent').title).toContain('intent');
  await user.click(within(screen.getByRole('region', { name: 'Source activity' })).getByRole('button', { name: 'Open session 7, event 999' }));
  expect(onSelectSession).toHaveBeenCalledWith(7, { entryId: 9, position: 1 });
  await user.click(within(screen.getByRole('region', { name: 'Git context' })).getByRole('button', { name: 'Open session 7, event 999' }));
  expect(onSelectSession).toHaveBeenLastCalledWith(7, { entryId: 9, position: 1 });
});
test('literal filename filters, file history and paging use the selected workspace ID', async () => {
  const user = userEvent.setup();
  api.mockImplementation(async (command, args) => command === 'project_files' ? { files: [file], has_more: !args.page.offset, next_offset: args.page.offset ? null : 20 } : handler(command, args));
  render(<ProjectsView onSelectSession={vi.fn()}/>);
  await screen.findByRole('button', { name: 'src/a.rs' });
  fireEvent.change(screen.getByRole('searchbox', { name: 'Filter reported filenames' }), { target: { value: 'a%_".rs' } });
  await waitFor(() => expect(api).toHaveBeenCalledWith('project_files', { workspaceId: 1, page: { limit: 20, offset: 0 }, filenameFilter: 'a%_".rs' }));
  await user.click(await screen.findByRole('button', { name: 'src/a.rs' }));
  await waitFor(() => expect(api).toHaveBeenCalledWith('file_history', { workspaceId: 1, reportedPath: 'src/a.rs', page: { limit: 20, offset: 0 } }));
  await user.click(screen.getByRole('button', { name: 'Next file pages' }));
  await waitFor(() => expect(api).toHaveBeenCalledWith('project_files', { workspaceId: 1, page: { limit: 20, offset: 20 }, filenameFilter: 'a%_".rs' }));
});
test('explicit Git inspection keeps recorded/current metadata distinct and shows bounds', async () => {
  const user = userEvent.setup(); render(<ProjectsView onSelectSession={vi.fn()}/>);
  await screen.findByText('recorded-branch');
  await user.click(screen.getByRole('button', { name: 'Inspect local Git' }));
  await screen.findByText('current-branch');
  expect(screen.getByText('recorded-branch')).toBeTruthy();
  expect(screen.getByText(/Current Git metadata differs/)).toBeTruthy();
  expect(screen.getByText(/100 examined, 2 skipped, path limit reached/)).toBeTruthy();
  expect(screen.getByText('Verified commit change').title).toContain('no agent authorship');
  expect(calls('inspect_git')).toEqual([['inspect_git', { workspaceId: 1 }]]);
});
test('remote and synced-only workspaces disable local inspection', async () => {
  const user = userEvent.setup(); render(<ProjectsView onSelectSession={vi.fn()}/>);
  await user.click(await screen.findByRole('button', { name: /box:\/work\/project/ }));
  expect((await screen.findByRole('button', { name: 'Inspect local Git' })).disabled).toBe(true);
  expect(screen.getByText('SSH host box', { exact: false })).toBeTruthy(); expect(calls('inspect_git')).toHaveLength(0);
});
test('stale file responses from another workspace never replace the current evidence', async () => {
  const user = userEvent.setup(); let finish;
  api.mockImplementation((command, args) => command === 'project_files' && args.workspaceId === 1 ? new Promise(resolve => { finish = resolve; }) : Promise.resolve(command === 'project_files' ? { files: [{ ...file, reported_path: 'remote.rs' }] } : handler(command, args)));
  render(<ProjectsView onSelectSession={vi.fn()}/>);
  await waitFor(() => expect(finish).toBeTypeOf('function'));
  await user.click(screen.getByRole('button', { name: /box:\/work\/project/ }));
  await screen.findByRole('button', { name: 'remote.rs' });
  await act(async () => finish({ files: [{ ...file, reported_path: 'stale-private.rs' }] }));
  expect(screen.queryByText('stale-private.rs')).toBeNull();
});
test('late Git inspection cannot paint another workspace and failures are actionable', async () => {
  const user = userEvent.setup(); let finish;
  api.mockImplementation((command, args) => command === 'inspect_git' ? new Promise(resolve => { finish = resolve; }) : Promise.resolve(handler(command, args)));
  render(<ProjectsView onSelectSession={vi.fn()}/>); await screen.findByText('Recorded Git context');
  await user.click(screen.getByRole('button', { name: 'Inspect local Git' }));
  await user.click(screen.getByRole('button', { name: /box:\/work\/project/ }));
  await act(async () => finish(inspection)); expect(screen.queryByText('current-branch')).toBeNull();
});
test('errors offer retry, and refresh reloads history without inspecting Git', async () => {
  const user = userEvent.setup(); let fail = true;
  api.mockImplementation(async (command, args) => { if (command === 'project_files' && fail) throw new Error('Library unavailable'); return handler(command, args); });
  render(<ProjectsView onSelectSession={vi.fn()}/>);
  await screen.findByText('Library unavailable'); fail = false;
  await user.click(within(screen.getByRole('region', { name: 'Reported files' })).getByRole('button', { name: 'Retry' }));
  await screen.findByRole('button', { name: 'src/a.rs' }); const before = calls('project_activity').length;
  await user.click(screen.getByRole('button', { name: 'Refresh recorded history' }));
  await waitFor(() => expect(calls('project_activity').length).toBeGreaterThan(before)); expect(calls('inspect_git')).toHaveLength(0);
});
test('untrusted path text is escaped and unresolved evidence never opens a file', async () => {
  const path = '<img src=x onerror=alert(1)>/../secret';
  api.mockImplementation(async (command, args) => command === 'project_activity' ? { evidence: [{ ...event, reported_path: path, candidate_workspace_relative_path: null }], entry_positions: { 9: 1 } } : handler(command, args));
  const { container } = render(<ProjectsView onSelectSession={vi.fn()}/>);
  await screen.findByText(path); expect(container.querySelector('img')).toBeNull();
  expect(screen.getByText('Unresolved recorded path · no filesystem access')).toBeTruthy(); expect(api.mock.calls.some(([name]) => name.includes('open_file'))).toBe(false);
});

test('workspace filtering resets paging and explicit inspection admits only one in-flight request', async () => {
  let finish;
  api.mockImplementation((command, args) => command === 'inspect_git' ? new Promise(resolve => { finish = resolve; }) : Promise.resolve(handler(command, args)));
  render(<ProjectsView onSelectSession={vi.fn()}/>);
  fireEvent.change(screen.getByRole('searchbox', { name: 'Filter recorded workspaces' }), { target: { value: '/work/project' } });
  await waitFor(() => expect(api).toHaveBeenCalledWith('project_catalog', { page: { limit: 20, offset: 0 }, projectFilter: '/work/project' }));
  const button = await screen.findByRole('button', { name: 'Inspect local Git' });
  act(() => { fireEvent.click(button); fireEvent.click(button); });
  expect(calls('inspect_git')).toHaveLength(1);
  await act(async () => finish(inspection));
  await screen.findByText('current-branch');
});
