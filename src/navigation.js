const analyticsDefaults = {
  activeTab: 'overview', dateFilter: '', customSince: '', agentFilter: '',
  workspaceFilter: '', modelFilter: '', trendMetric: 'cost', timezone: '',
};
const analyticsParams = {
  activeTab: 'tab', dateFilter: 'since', customSince: 'customSince', agentFilter: 'agent',
  workspaceFilter: 'workspace', modelFilter: 'model', trendMetric: 'metric', timezone: 'timezone',
};

export function readNavigation() {
  const params = new URL(window.location.href).searchParams;
  const saved = window.history.state?.appNavigation;
  const view = params.get('view') === 'analytics' ? params.get('view') : 'sessions';
  const session = params.get('session');
  const selected = session && /^\d+$/.test(session) && Number(session) > 0 ? session : null;
  const analytics = { ...analyticsDefaults, ...saved?.analytics };
  if (view !== 'sessions') {
    for (const [key, param] of Object.entries(analyticsParams)) analytics[key] = params.get(param) ?? analyticsDefaults[key];
  }
  if (!['overview', 'tokens-cost', 'tools', 'productivity', 'pricing', 'overrides'].includes(analytics.activeTab)) analytics.activeTab = 'overview';
  if (!['cost', 'tokens', 'sessions'].includes(analytics.trendMetric)) analytics.trendMetric = 'cost';
  return { view, selected: view !== 'sessions' ? saved?.selected ?? null : selected,
    selectionExplicit: saved?.selectionExplicit ?? Boolean(selected), analytics };
}

export function writeNavigation(navigation, { replace = false } = {}) {
  const url = new URL(window.location.href);
  for (const param of ['view', 'session', ...Object.values(analyticsParams)]) url.searchParams.delete(param);
  if (navigation.view !== 'sessions') {
    url.searchParams.set('view', navigation.view);
    const reportState = navigation.analytics;
    for (const [key, param] of Object.entries(analyticsParams)) {
      const value = reportState[key] ?? analyticsDefaults[key];
      if (value !== analyticsDefaults[key]) url.searchParams.set(param, value);
    }
  } else if (navigation.selected != null) {
    url.searchParams.set('session', String(navigation.selected));
  }
  const state = { ...window.history.state, appNavigation: navigation };
  const sameUrl = url.href === window.location.href;
  window.history[replace || sameUrl ? 'replaceState' : 'pushState'](state, '', url);
}
