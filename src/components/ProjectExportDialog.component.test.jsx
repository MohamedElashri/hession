import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, test, vi } from 'vitest';
import { save } from '@tauri-apps/plugin-dialog';
import { api } from '../api.js';
import ProjectExportDialog, { ProjectExportButton } from './ProjectExportDialog.jsx';
vi.mock('../api.js', () => ({ api: vi.fn() }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ save: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));
const project = { workspace_id: 1, project_key: 'project-v1:abc', display_name: 'Example project', recorded_path: '/private/project' };
const calls = command => api.mock.calls.filter(([name]) => command === name);
const preview = { projectKey: project.project_key, sessionCount: 2, selectedSessionCount: 2, exportedEntryCount: 5, estimatedUncompressedBytes: 4096, intendedOutput: '/tmp/project.zip', redactions: { secretPatterns: 3, pathPatterns: 4 }, warnings: ['Pattern redaction cannot find every secret.'] };
const saved = { destination: '/tmp/project.zip', sessionCount: 2, archiveBytes: 2000, warnings: [] };
function handler(command, args) {
  if (command === 'prepare_project_archive') return { jobId: args.jobId, preview };
  if (command === 'write_project_archive') return saved;
  if (command === 'cancel_project_archive') return true;
  if (command === 'release_project_archive') return undefined;
  throw new Error(`Unexpected command ${command}`);
}
beforeEach(() => { api.mockReset(); api.mockImplementation(async (command, args) => handler(command, args)); save.mockReset(); save.mockResolvedValue('/tmp/project.zip'); });
async function prepare(user) { await user.click(screen.getByRole('button', { name: 'Choose destination' })); await user.click(screen.getByRole('button', { name: 'Prepare preview' })); await screen.findByRole('heading', { name: 'Reviewed selection' }); }

test('privacy defaults require an exact reviewed preview before any archive is written', async () => {
  const user = userEvent.setup(); render(<ProjectExportDialog project={project} onClose={vi.fn()}/>);
  expect(screen.getByRole('checkbox', { name: 'Redact common secret patterns' }).checked).toBe(true);
  for (const name of ['Include tool inputs and outputs', 'Include absolute machine paths', 'Include recorded Git metadata', 'Include cached statistics']) expect(screen.getByRole('checkbox', { name }).checked).toBe(false);
  expect(screen.getByRole('button', { name: 'Save reviewed ZIP' }).disabled).toBe(true);
  await prepare(user); expect(calls('write_project_archive')).toHaveLength(0);
  expect(calls('prepare_project_archive')[0][1]).toMatchObject({ expectedProjectKey: project.project_key, target: '/tmp/project.zip', request: { workspaceId: 1, options: { redact: true, includeToolBodies: false, includeAbsolutePaths: false } } });
  expect(screen.getByText(/2 session snapshots/)).toBeTruthy(); expect(screen.getByText(/3 secret-pattern matches/)).toBeTruthy();
  await user.click(screen.getByRole('button', { name: 'Save reviewed ZIP' })); await screen.findByRole('heading', { name: 'Project archive saved' });
  expect(calls('write_project_archive')[0][1]).toEqual({ jobId: calls('prepare_project_archive')[0][1].jobId });
});
test('changing options or query invalidates and releases the old reviewed plan', async () => {
  const user = userEvent.setup(); render(<ProjectExportDialog project={project} onClose={vi.fn()}/>); await prepare(user);
  await user.click(screen.getByRole('checkbox', { name: 'Include tool inputs and outputs' }));
  expect(screen.getByRole('button', { name: 'Save reviewed ZIP' }).disabled).toBe(true); expect(screen.queryByText('Reviewed selection')).toBeNull();
  await waitFor(() => expect(calls('release_project_archive').length).toBeGreaterThan(0));
  await user.type(screen.getByRole('textbox', { name: 'Developer query' }), 'tool:Write -secret');
  await user.click(screen.getByRole('button', { name: 'Prepare preview' })); await screen.findByText('Reviewed selection');
  expect(calls('prepare_project_archive').at(-1)[1].request).toMatchObject({ query: 'tool:Write -secret', options: { includeToolBodies: true } }); expect(calls('write_project_archive')).toHaveLength(0);
});
test('double save activation starts one job and prevents editing options during publication', async () => {
  const user = userEvent.setup(); let finish;
  api.mockImplementation((command, args) => command === 'write_project_archive' ? new Promise(resolve => { finish = resolve; }) : Promise.resolve(handler(command, args)));
  render(<ProjectExportDialog project={project} onClose={vi.fn()}/>); await prepare(user);
  const button = screen.getByRole('button', { name: 'Save reviewed ZIP' }); act(() => { fireEvent.click(button); fireEvent.click(button); });
  expect(calls('write_project_archive')).toHaveLength(1); expect(screen.getByRole('checkbox', { name: 'Include absolute machine paths' }).matches(':disabled')).toBe(true);
  await act(async () => finish(saved)); await screen.findByText('Project archive saved');
});
test('cancelling a pending preview never makes its late response actionable', async () => {
  const user = userEvent.setup(); let finish;
  api.mockImplementation((command, args) => command === 'prepare_project_archive' ? new Promise(resolve => { finish = () => resolve({ jobId: args.jobId, preview }); }) : Promise.resolve(handler(command, args)));
  render(<ProjectExportDialog project={project} onClose={vi.fn()}/>); await user.click(screen.getByRole('button', { name: 'Choose destination' })); await user.click(screen.getByRole('button', { name: 'Prepare preview' }));
  await user.click(screen.getByRole('button', { name: 'Cancel export' })); expect(calls('cancel_project_archive')).toHaveLength(1);
  await act(async () => finish()); await screen.findByText('Project preview cancelled. No archive was saved.');
  expect(screen.getByRole('button', { name: 'Save reviewed ZIP' }).disabled).toBe(true); expect(calls('write_project_archive')).toHaveLength(0);
});
test('another workspace cancels old preparation and rejects late preview results', async () => {
  const user = userEvent.setup(); let finish;
  api.mockImplementation((command, args) => command === 'prepare_project_archive' ? new Promise(resolve => { finish = () => resolve({ jobId: args.jobId, preview }); }) : Promise.resolve(handler(command, args)));
  const view = render(<ProjectExportDialog project={project} onClose={vi.fn()}/>); await user.click(screen.getByRole('button', { name: 'Choose destination' })); await user.click(screen.getByRole('button', { name: 'Prepare preview' }));
  view.rerender(<ProjectExportDialog project={{ ...project, workspace_id: 2, project_key: 'project-v1:other', display_name: 'Other project' }} onClose={vi.fn()}/>);
  await act(async () => finish()); expect(screen.getByText('Other project')).toBeTruthy(); expect(screen.queryByText('Reviewed selection')).toBeNull();
  expect(screen.getByRole('button', { name: 'Save reviewed ZIP' }).disabled).toBe(true); expect(calls('release_project_archive').length).toBeGreaterThan(0);
});
test('stale library errors require preparation again and destination cancellation creates no job', async () => {
  const user = userEvent.setup(); api.mockImplementation(async (command, args) => { if (command === 'write_project_archive') throw new Error('The library changed after preview; prepare the export again'); return handler(command, args); });
  render(<ProjectExportDialog project={project} onClose={vi.fn()}/>); save.mockResolvedValueOnce(null); await user.click(screen.getByRole('button', { name: 'Choose destination' }));
  expect(calls('prepare_project_archive')).toHaveLength(0); await prepare(user); await user.click(screen.getByRole('button', { name: 'Save reviewed ZIP' }));
  await screen.findByRole('alert'); expect(screen.getByRole('button', { name: 'Save reviewed ZIP' }).disabled).toBe(true); expect(screen.getByRole('button', { name: 'Prepare preview' }).disabled).toBe(false);
});

test('the extracted project button opens the dialog and restores keyboard focus on close', async () => {
  const user = userEvent.setup(); render(<ProjectExportButton project={project}/>);
  const button = screen.getByRole('button', { name: 'Export project ZIP' }); await user.click(button);
  await screen.findByRole('dialog', { name: 'Export project history' });
  await user.click(screen.getByRole('button', { name: 'Close project export' }));
  expect(screen.queryByRole('dialog')).toBeNull(); expect(document.activeElement).toBe(button);
});
test('a completed save wins a concurrent cancellation without claiming the archive was removed', async () => {
  const user = userEvent.setup(); let finish;
  api.mockImplementation((command, args) => command === 'write_project_archive' ? new Promise(resolve => { finish = resolve; }) : Promise.resolve(command === 'cancel_project_archive' ? false : handler(command, args)));
  render(<ProjectExportDialog project={project} onClose={vi.fn()}/>); await prepare(user);
  await user.click(screen.getByRole('button', { name: 'Save reviewed ZIP' })); await user.click(screen.getByRole('button', { name: 'Cancel export' }));
  await screen.findByText('The archive is already saved; finishing…');
  await act(async () => finish(saved)); await screen.findByRole('heading', { name: 'Project archive saved' });
  expect(screen.queryByRole('alert')).toBeNull(); expect(screen.getByRole('button', { name: 'Done' })).toBeTruthy();
});

test.each(['close', 'cancel'])('waiting for old cleanup cannot start a new preview after %s', async action => {
  const user=userEvent.setup();const view=render(<ProjectExportDialog project={project} onClose={vi.fn()}/>);await prepare(user);
  let finishRelease;
  api.mockImplementation((command,args)=>command==='release_project_archive'?new Promise(resolve=>{finishRelease=resolve;}):Promise.resolve(handler(command,args)));
  await user.click(screen.getByRole('checkbox',{name:'Include tool inputs and outputs'}));
  await waitFor(()=>expect(finishRelease).toBeTypeOf('function'));
  await user.click(screen.getByRole('button',{name:'Prepare preview'}));
  expect(calls('prepare_project_archive')).toHaveLength(1);
  if(action==='close') view.unmount();
  else await user.click(screen.getByRole('button',{name:'Cancel export'}));
  await act(async()=>finishRelease());
  expect(calls('prepare_project_archive')).toHaveLength(1);
  if(action==='cancel') {
    await screen.findByText('Project preview cancelled. No archive was saved.');
    expect(screen.getByRole('button',{name:'Save reviewed ZIP'}).disabled).toBe(true);
  }
});
test('repeated cancellation shares one pending native request', async()=>{
  const user=userEvent.setup();let finishPreview,finishCancel;
  api.mockImplementation((command,args)=>command==='prepare_project_archive'?new Promise(resolve=>{finishPreview=()=>resolve({jobId:args.jobId,preview});})
    :command==='cancel_project_archive'?new Promise(resolve=>{finishCancel=resolve;}):Promise.resolve(handler(command,args)));
  render(<ProjectExportDialog project={project} onClose={vi.fn()}/>);
  await user.click(screen.getByRole('button',{name:'Choose destination'}));await user.click(screen.getByRole('button',{name:'Prepare preview'}));
  const button=screen.getByRole('button',{name:'Cancel export'});
  await act(async()=>{for(let n=0;n<20;n++)fireEvent.click(button);});
  expect(calls('cancel_project_archive')).toHaveLength(1);
  await act(async()=>finishCancel(true));await act(async()=>finishPreview());
  await screen.findByText('Project preview cancelled. No archive was saved.');
  expect(calls('write_project_archive')).toHaveLength(0);
});
test('late native admission is released even if closing cleanup ran before admission',async()=>{
  const user=userEvent.setup();let finish;
  api.mockImplementation((command,args)=>command==='prepare_project_archive'?new Promise(resolve=>{finish=()=>resolve({jobId:args.jobId,preview});})
    :command==='release_project_archive'?Promise.reject(Error('Not admitted yet')):Promise.resolve(handler(command,args)));
  const view=render(<ProjectExportDialog project={project} onClose={vi.fn()}/>);
  await user.click(screen.getByRole('button',{name:'Choose destination'}));await user.click(screen.getByRole('button',{name:'Prepare preview'}));
  const id=calls('prepare_project_archive')[0][1].jobId;view.unmount();
  await waitFor(()=>expect(calls('release_project_archive')).toHaveLength(1));
  await act(async()=>finish());
  expect(calls('release_project_archive').map(([,args])=>args.jobId)).toEqual([id,id]);
  expect(calls('write_project_archive')).toHaveLength(0);
});
test('a late cancellation response cannot replace a newer preview status',async()=>{
  const user=userEvent.setup();const previews=[];let finishCancel;
  api.mockImplementation((command,args)=>command==='prepare_project_archive'?new Promise(resolve=>previews.push(()=>resolve({jobId:args.jobId,preview})))
    :command==='cancel_project_archive'?new Promise(resolve=>{finishCancel=resolve;}):Promise.resolve(handler(command,args)));
  render(<ProjectExportDialog project={project} onClose={vi.fn()}/>);
  await user.click(screen.getByRole('button',{name:'Choose destination'}));await user.click(screen.getByRole('button',{name:'Prepare preview'}));
  await user.click(screen.getByRole('button',{name:'Cancel export'}));await act(async()=>previews[0]());
  await screen.findByText('Project preview cancelled. No archive was saved.');
  await user.click(screen.getByRole('button',{name:'Prepare preview'}));await act(async()=>finishCancel(false));
  expect(screen.queryByText('The archive is already saved; finishing…')).toBeNull();
  await act(async()=>previews[1]());await screen.findByText('Reviewed selection');
  expect(screen.getByRole('button',{name:'Save reviewed ZIP'}).disabled).toBe(false);
});
