import { beforeEach, expect, test } from 'vitest';
import { readNavigation, writeNavigation } from './navigation.js';

beforeEach(() => window.history.replaceState(null, '', '/'));

test('Live URLs retain session/report navigation state without serializing analytics filters into Live', () => {
  const initial = readNavigation();
  const state = { ...initial, view: 'live', selected: 42, selectionExplicit: true,
    analytics: { ...initial.analytics, activeTab: 'tools', agentFilter: 'codex' } };
  writeNavigation(state);
  expect(window.location.search).toBe('?view=live');
  expect(readNavigation()).toEqual(state);
  writeNavigation({ ...state, view: 'analytics' });
  expect(window.location.search).toContain('tab=tools');
  expect(readNavigation().analytics.agentFilter).toBe('codex');
  writeNavigation({ ...state, view: 'sessions' });
  expect(window.location.search).toBe('?session=42');
  expect(readNavigation().selected).toBe('42');
});

test('fresh Live links are recognized and unknown view names still use Sessions', () => {
  window.history.replaceState(null, '', '/?view=live&tab=unknown&agent=unexpected');
  const live = readNavigation();
  expect(live.view).toBe('live'); expect(live.analytics.activeTab).toBe('overview');
  expect(live.analytics.agentFilter).toBe('');
  window.history.replaceState(null, '', '/?view=untrusted');
  expect(readNavigation().view).toBe('sessions');
});
