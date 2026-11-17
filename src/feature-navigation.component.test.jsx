import { useEffect, useMemo, useState } from 'react';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, test, vi } from 'vitest';
import { api } from './api.js';
import { createCommandRegistry, createNavigationCommands, focusAfterRender, keyboardEventContext, moveRegionFocus, SESSION_ITEM_SELECTOR } from './commands.js';
import CommandPalette from './components/CommandPalette.jsx';

vi.mock('./api.js', () => ({ api: vi.fn() }));
beforeEach(() => api.mockResolvedValue({ sessions: [{ id: 2, title: 'Recent agent session' }] }));

// These feature-owned screen callbacks exercise the production registry/palette
// boundary. They deliberately supply no Live, Files, Git or export backend.
function FeatureNavigation() {
  const [view, setView] = useState('sessions');
  const [selected, setSelected] = useState(1);
  const [project, setProject] = useState('');
  const [palette, setPalette] = useState(false);
  const [review, setReview] = useState(false);
  const context = useMemo(() => ({ view, project, mac: false }), [view, project]);
  const registry = createCommandRegistry([
    { id: 'palette.open', title: 'Open commands', shortcuts: ['Mod+k'], allowInInput: true, run: () => setPalette(true) },
    { id: 'search.global', title: 'Search all sessions', shortcuts: ['/'], run: () => setView('sessions') },
    { id: 'shortcuts.help', title: 'Show keyboard help', shortcuts: ['?'], run: () => setReview(true) },
    ...createNavigationCommands([
      { id: 'view.sessions', title: 'Show sessions', open: () => setView('sessions'), focus: '#feature-heading' },
      { id: 'view.live', title: 'Show live activity', open: () => setView('live'), focus: '#feature-heading' },
      { id: 'view.projects', title: 'Show projects', open: () => setView('projects'), focus: '#feature-heading' },
      { id: 'project.files', title: 'Show project files', disabled: state => state.project ? null : 'Choose a project first.',
        open: () => setView('files'), focus: '#feature-heading' },
    ]),
    { id: 'list.next', title: 'Next session', shortcuts: ['j'], hidden: true, when: state => !!state.sessionList,
      run: state => moveRegionFocus(state.sessionList, SESSION_ITEM_SELECTOR, 1) },
    { id: 'list.open', title: 'Open session', shortcuts: ['Enter'], hidden: true, when: state => state.onSessionCard,
      run: state => state.target.click() },
  ]);
  useEffect(() => {
    const handler = event => registry.handleKeyDown(event, keyboardEventContext(event, { ...context, modalOpen: palette }));
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [registry, context, palette]);
  const afterPalette = action => { setPalette(false); requestAnimationFrame(action); };
  const openSession = id => { setSelected(id); setView('sessions'); focusAfterRender('#feature-heading'); };
  const labels = { sessions: 'Sessions', live: 'Live activity', projects: 'Projects', files: 'Project files' };
  return <>
    <button type="button" onClick={() => setPalette(true)}>Commands</button>
    <button type="button" onClick={() => setReview(true)}>Open feature review</button>
    <main className="main-panel" tabIndex={-1}>
      <h1 id="feature-heading" tabIndex={-1}>{labels[view]}</h1>
      <p>Selected session: {selected}</p><p>Project: {project || 'none'}</p>
      <input aria-label="Feature filter"/>
      {view === 'live' && <section data-session-list aria-label="Recent sessions">
        <input aria-label="Recent session filter"/>
        <div contentEditable suppressContentEditableWarning role="textbox" aria-label="Feature editor"/>
        {[1, 2].map(id => <button type="button" data-session-navigation key={id} onClick={() => openSession(id)}>Recent session {id}</button>)}
      </section>}
    </main>
    {review && <div role="dialog" aria-modal="true" aria-label="Feature review">
      <input aria-label="Review field"/><button type="button" onClick={() => setReview(false)}>Close review</button>
    </div>}
    {palette && <CommandPalette registry={registry} context={context} projects={[{ path: '/work/demo', display_name: 'Demo workspace' }]}
      onClose={() => setPalette(false)} onExecute={id => afterPalette(() => registry.execute(id, context))}
      onSession={id => afterPalette(() => openSession(id))}
      onProject={value => afterPalette(() => { setProject(value.path); setView('projects'); focusAfterRender('#feature-heading'); })}/>}
  </>;
}

async function chooseCommand(user, title) {
  await user.keyboard('{Control>}k{/Control}');
  await user.type(await screen.findByRole('combobox'), title);
  await user.keyboard('{Enter}');
}

test('feature navigation hooks open Live and Projects/Files and restore focus without resetting the selected session', async () => {
  const user = userEvent.setup();
  render(<FeatureNavigation/>);
  await chooseCommand(user, 'Show live activity');
  await waitFor(() => expect(document.activeElement).toBe(screen.getByRole('heading', { name: 'Live activity' })));
  expect(screen.getByText('Selected session: 1')).toBeTruthy();
  const first = screen.getByRole('button', { name: 'Recent session 1' });
  first.focus();
  fireEvent.keyDown(first, { key: 'j', isComposing: true });
  expect(document.activeElement).toBe(first);
  await user.keyboard('j{Enter}');
  await waitFor(() => expect(document.activeElement).toBe(screen.getByRole('heading', { name: 'Sessions' })));
  expect(screen.getByText('Selected session: 2')).toBeTruthy();
  await chooseCommand(user, 'Show project files');
  expect(screen.getByRole('option', { name: /Choose a project first/ }).getAttribute('aria-disabled')).toBe('true');
  expect(screen.getByRole('dialog', { name: 'Command palette' })).toBeTruthy();
  await user.keyboard('{Escape}');
  await chooseCommand(user, 'Demo workspace');
  await waitFor(() => expect(document.activeElement).toBe(screen.getByRole('heading', { name: 'Projects' })));
  await chooseCommand(user, 'Show project files');
  await waitFor(() => expect(document.activeElement).toBe(screen.getByRole('heading', { name: 'Project files' })));
  expect(screen.getByText('Project: /work/demo')).toBeTruthy();
  expect(screen.getByText('Selected session: 2')).toBeTruthy();
  await user.keyboard('{Control>}k{/Control}');
  await user.type(await screen.findByRole('combobox'), 'Recent agent');
  await screen.findByRole('option', { name: 'Session Recent agent session' });
  await user.keyboard('{Enter}');
  await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
  expect(await screen.findByRole('heading', { name: 'Sessions' })).toBeTruthy();
  expect(new Set(api.mock.calls.map(([command]) => command))).toEqual(new Set(['search_sessions']));
});

test('new feature inputs and review dialogs retain typing and suppress app shortcuts', async () => {
  const user = userEvent.setup();
  render(<FeatureNavigation/>);
  await chooseCommand(user, 'Show live activity');
  await waitFor(() => expect(document.activeElement).toBe(screen.getByRole('heading', { name: 'Live activity' })));
  const filter = screen.getByRole('textbox', { name: 'Feature filter' });
  await user.type(filter, 'j/k?');
  expect(filter.value).toBe('j/k?');
  const recentFilter = screen.getByRole('textbox', { name: 'Recent session filter' });
  await user.type(recentFilter, 'j/k?');
  expect(recentFilter.value).toBe('j/k?');
  const editor = screen.getByRole('textbox', { name: 'Feature editor' });
  await user.type(editor, 'j/k?');
  expect(editor.textContent).toBe('j/k?');
  expect(screen.queryByRole('dialog')).toBeNull();
  await user.click(screen.getByRole('button', { name: 'Open feature review' }));
  const review = screen.getByRole('textbox', { name: 'Review field' });
  await user.type(review, 'j/k?');
  await user.keyboard('{Control>}k{/Control}');
  expect(screen.queryByRole('combobox')).toBeNull();
  expect(review.value).toBe('j/k?');
  await user.click(screen.getByRole('button', { name: 'Close review' }));
  await user.click(screen.getByRole('button', { name: 'Commands' }));
  await user.keyboard('{Escape}');
  expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Commands' }));
});
