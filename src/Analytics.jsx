import React, { useState, useEffect, useMemo, useCallback, useRef } from 'react';
import {
  TrendingUp, Coins, Terminal, Activity, Flame, AlertTriangle, CheckCircle2,
  Clock, ArrowUpRight, ArrowUp, ArrowDown, ArrowUpDown, RefreshCw, SlidersHorizontal, Filter,
  DollarSign, Search, Copy, Check, ExternalLink, ChevronDown,
  ChevronRight, Info, X, Zap, BarChart3, Cpu, LoaderCircle
} from 'lucide-react';

import { parseDate, filterSince } from './dates.js';
import ReportFilters from './components/ReportFilters.jsx';
import { useNumericSort, NumericSortHeader, numericSortValue } from './components/SortableTable.jsx';
import { changeReportFilters, reportError } from './analytics-data.js';
import { invoke } from '@tauri-apps/api/core';

const API = '/api';
// Tauri-first API layer: try `invoke` first (desktop app); fall back to the
// HTTP dev API when the Tauri runtime is absent (vite dev outside Tauri).
// Command names follow the frontend<->backend contract:
// GET /api/analytics->get_analytics, POST /api/analytics/refresh->refresh_analytics,
// GET /api/analytics/pricing->get_pricing_status,
// POST /api/analytics/pricing/sync->sync_pricing,
// GET /api/analytics/pricing/overrides->get_pricing_overrides,
// POST ->save_pricing_override, DELETE ->delete_pricing_override.
// Same query params / request bodies are passed as invoke args.
const hasTauriRuntime = () => typeof window !== 'undefined' && !!window.__TAURI_INTERNALS__?.invoke;
const tauriError = (e) => e instanceof Error ? e : new Error(typeof e === 'string' ? e : (e?.message || JSON.stringify(e ?? 'Unknown error')));
const apiCall = async (cmd, args, url, opts = {}) => {
  try { return await invoke(cmd, args); }
  catch (e) {
    if (url == null || hasTauriRuntime()) throw tauriError(e);
    return fetchJson(url, opts);
  }
};
const fetchJson = async (url, opts = {}) => {
  const res = await fetch(url, opts);
  const json = await res.json().catch(() => ({}));
  if (!res.ok) throw new Error(json.error || res.statusText);
  return json;
};

// Format helper utilities
const formatNum = (num) => {
  if (num == null) return '0';
  if (num >= 1000000000) return (num / 1000000000).toFixed(2) + 'B';
  if (num >= 1000000) return (num / 1000000).toFixed(2) + 'M';
  if (num >= 1000) return (num / 1000).toFixed(1) + 'K';
  return num.toLocaleString();
};

const formatCost = (val) => {
  if (val == null || val === 0) return '$0.00';
  if (val < 0.01) return `< $0.01`;
  return `$${val.toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;
};

const formatDuration = (ms) => {
  if (!ms || ms <= 0) return '0ms';
  if (ms < 1000) return `${Math.round(ms)}ms`;
  if (ms < 60000) return `${(ms / 1000).toFixed(1)}s`;
  const mins = Math.floor(ms / 60000);
  const secs = Math.round((ms % 60000) / 1000);
  return `${mins}m ${secs}s`;
};

const shortPath = (p) => String(p ?? '').replace(/\\/g, '/').split('/').filter(Boolean).at(-1) || 'Local workspace';

const DAY_NAMES = ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday'];
const DAY_SHORT = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];

export default function AnalyticsView({
  theme,
  agents = [],
  projects = [],
  metadataState = {},
  navigationState = {},
  onNavigationChange,
  onSelectSession,
  onClose,
  refreshKey = 0,
  syncRunning = false
}) {
  const activeTab = navigationState.activeTab || 'overview';
  const setActiveTab = value => onNavigationChange?.({ activeTab: value });
  const [data, setData] = useState(null);
  const [loading, setLoading] = useState(true);
  const [filterLoading, setFilterLoading] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [pricingStatus, setPricingStatus] = useState(null);
  const [pricingSyncing, setPricingSyncing] = useState(false);
  const [overrides, setOverrides] = useState([]);
  const [overridesLoading, setOverridesLoading] = useState(false);
  const [overrideSaving, setOverrideSaving] = useState(false);
  const [error, setError] = useState('');
  const [copiedCmd, setCopiedCmd] = useState(null);
  const analyticsRequestId = useRef(0);

  // Filters (date uses the same dropdown UX as the sessions page:
  // presets + custom from-date with Apply. Internally a custom date is
  // stored as dateFilter='custom' + customSince='YYYY-MM-DD' to keep URLs stable.)
  const dateFilter = navigationState.dateFilter || '';
  const customSince = navigationState.customSince || '';
  const agentFilter = navigationState.agentFilter || '';
  const workspaceFilter = navigationState.workspaceFilter || '';
  const modelFilter = navigationState.modelFilter || '';
  const setModelFilter = value => onNavigationChange?.(changeReportFilters({ modelFilter: value }));

  // Selected chart metric for trend view
  const trendMetric = navigationState.trendMetric || 'cost';
  const setTrendMetric = value => onNavigationChange?.({ trendMetric: value });
  const [hoveredTrendPoint, setHoveredTrendPoint] = useState(null);
  const [hoveredHeatCell, setHoveredHeatCell] = useState(null);

  const fetchAnalytics = useCallback(async (isRefresh = false) => {
    const reqId = ++analyticsRequestId.current;
    if (isRefresh) setRefreshing(true);
    else if (!data) setLoading(true);
    else setFilterLoading(true);
    setError('');

    try {
      const params = new URLSearchParams({ timezone: navigationState.timezone || Intl.DateTimeFormat().resolvedOptions().timeZone });
      const since = dateFilter === 'custom' ? customSince : dateFilter;
      if (since) params.set('since', filterSince(since));
      if (agentFilter) params.set('agent', agentFilter);
      if (workspaceFilter) params.set('workspace', workspaceFilter);
      if (modelFilter) params.set('model', modelFilter);

      const analyticsParams = {
        timezone: navigationState.timezone || Intl.DateTimeFormat().resolvedOptions().timeZone,
        since: since ? filterSince(since) : '',
        until: '',
        agent: agentFilter || '',
        workspace: workspaceFilter || '',
        model: modelFilter || '',
      };
      const json = await apiCall('get_analytics', analyticsParams, `${API}/analytics?${params.toString()}`);
      if (reqId === analyticsRequestId.current) {
        setData(json);
        if (json.refreshError) setError(json.refreshError);
      }
    } catch (e) {
      if (reqId === analyticsRequestId.current) {
        setError(reportError(e));
      }
    } finally {
      if (reqId === analyticsRequestId.current) {
        setLoading(false);
        setFilterLoading(false);
        setRefreshing(false);
      }
    }
  }, [dateFilter, customSince, agentFilter, workspaceFilter, modelFilter, navigationState.timezone, data, syncRunning]);

  useEffect(() => {
    fetchAnalytics();
    return () => { analyticsRequestId.current++; };
  }, [activeTab, dateFilter, customSince, agentFilter, workspaceFilter, modelFilter, navigationState.timezone, refreshKey, syncRunning]);

  useEffect(() => {
    if (!syncRunning && !data?.refreshing) return;
    // Refresh committed reports without clearing the current display.
    const timer = setTimeout(() => fetchAnalytics(), 5000);
    return () => clearTimeout(timer);
  }, [syncRunning, data, fetchAnalytics]);

  const handleManualRefresh = async () => {
    setRefreshing(true);
    try {
      await apiCall('refresh_analytics', {}, `${API}/analytics/refresh`, { method: 'POST' });
      await fetchAnalytics(true);
    } catch (e) {
      setError(reportError(e));
      setRefreshing(false);
    }
  };

  const fetchPricingStatus = useCallback(async () => {
    try {
      setPricingStatus(await apiCall('get_pricing_status', {}, `${API}/analytics/pricing`));
    } catch (e) { setError(reportError(e)); }
  }, []);

  const fetchOverrides = useCallback(async () => {
    setOverridesLoading(true);
    try {
      const json = await apiCall('get_pricing_overrides', {}, `${API}/analytics/pricing/overrides`);
      setOverrides(Array.isArray(json.overrides) ? json.overrides : []);
    } catch (e) { setError(reportError(e)); }
    finally {
      setOverridesLoading(false);
    }
  }, []);

  useEffect(() => {
    fetchPricingStatus();
    fetchOverrides();
  }, [fetchPricingStatus, fetchOverrides, syncRunning]);

  // Save one price override, then recalculate costs.
  const handleSaveOverride = async (payload) => {
    setOverrideSaving(true);
    setError('');
    try {
      const json = await apiCall('save_pricing_override', payload, `${API}/analytics/pricing/overrides`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(payload),
      });
      if (Array.isArray(json.overrides)) setOverrides(json.overrides);
      else await fetchOverrides();
      await fetchPricingStatus();
      await fetchAnalytics(true);
      return true;
    } catch (e) {
      setError(reportError(e));
      return false;
    } finally {
      setOverrideSaving(false);
    }
  };

  // Reset overrides: { provider, modelId } -> one; { provider } -> provider;
  // { modelId } -> model everywhere; {} -> all.
  const handleResetOverrides = async (scope, confirmMessage) => {
    if (confirmMessage && typeof window !== 'undefined' && !window.confirm(confirmMessage)) return false;
    setOverrideSaving(true);
    setError('');
    try {
      const json = await apiCall('delete_pricing_override', (scope || {}), `${API}/analytics/pricing/overrides`, {
        method: 'DELETE',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(scope || {}),
      });
      if (Array.isArray(json.overrides)) setOverrides(json.overrides);
      else await fetchOverrides();
      await fetchPricingStatus();
      await fetchAnalytics(true);
      return true;
    } catch (e) {
      setError(reportError(e));
      return false;
    } finally {
      setOverrideSaving(false);
    }
  };

  // Sync model prices from models.dev, then recalculate costs.
  const handlePricingSync = async () => {
    setPricingSyncing(true);
    setError('');
    try {
      const json = await apiCall('sync_pricing', {}, `${API}/analytics/pricing/sync`, { method: 'POST' });
      if (json.pricing) setPricingStatus(json.pricing);
      else await fetchPricingStatus();
      await fetchAnalytics(true);
    } catch (e) {
      setError(reportError(e));
    } finally {
      setPricingSyncing(false);
    }
  };

  const pricingTitle = pricingStatus?.updatedAt
    ? `Model prices from models.dev, cached ${(parseDate(pricingStatus.updatedAt)?.toLocaleString() || 'date unavailable')} (${pricingStatus.modelCount?.toLocaleString()} models). Click to re-sync.`
    : 'Model prices are using built-in fallback rates. Click to sync live prices from models.dev.';

  const copyCommand = (cmd) => {
    navigator.clipboard?.writeText(cmd).then(() => {
      setCopiedCmd(cmd);
      setTimeout(() => setCopiedCmd(null), 1800);
    });
  };

  const kpis = data?.kpis || {};
  const tokensAndCosts = data?.tokens_and_costs || {};
  const toolMetrics = data?.tool_metrics || {};
  const productivity = data?.productivity_pulse || {};

  return (
    <div className="analytics-container">
      {/* Top Header & Global Filters */}
      <header className="analytics-header">
        <div className="analytics-title-group">
          <div className="analytics-eyebrow">
            <span className="eyebrow-dot" /> TOKEN, COST & PERFORMANCE PULSE
          </div>
          <h2>Analytics Dashboard</h2>
        </div>

        <div className="analytics-filter-bar">
          <ReportFilters navigation={navigationState} onChange={onNavigationChange} agents={agents} projects={projects} metadataState={metadataState} />
          {modelFilter && <button className="filter-button filter-has-value" onClick={() => setModelFilter('')} title="Clear model filter">Model: {modelFilter}<X size={12} /></button>}

          {(filterLoading || (loading && data)) && (
            <div className="filter-updating-badge" role="status" aria-live="polite">
              <LoaderCircle className="spin" size={13} />
              <span>Updating…</span>
            </div>
          )}

          <button
            className={`analytics-refresh-btn ${refreshing ? 'spinning' : ''}`}
            onClick={handleManualRefresh}
            title="Refresh analytics data"
            disabled={loading || filterLoading || refreshing}
          >
            <RefreshCw size={14} className={refreshing ? 'spin' : ''} />
            <span>{refreshing ? 'Refreshing…' : 'Refresh'}</span>
          </button>

          <button
            className="analytics-refresh-btn pricing-sync-btn"
            onClick={handlePricingSync}
            title={pricingTitle}
            disabled={loading || pricingSyncing || refreshing}
          >
            <DollarSign size={14} className={pricingSyncing ? 'spin' : ''} />
            <span>{pricingSyncing ? 'Syncing…' : 'Sync prices'}</span>
            {pricingStatus && pricingStatus.source !== 'fallback' && (
              <span className={`pricing-stale-dot ${pricingStatus.stale ? 'stale' : 'fresh'}`} />
            )}
          </button>
          {pricingStatus && (
            <span className="pricing-age-label" title={pricingTitle}>
              {pricingStatus.source === 'fallback'
                ? 'fallback rates'
                : `models.dev · ${pricingStatus.stale ? 'stale' : 'cached'}`}
            </span>
          )}
        </div>
      </header>

      {/* Subtle indicator bar for background filter re-fetches */}
      {(filterLoading || refreshing) && (
        <div className="analytics-filter-progress" role="progressbar" aria-label="Loading analytics data">
          <span className="analytics-filter-progress-bar" />
        </div>
      )}

      {/* Tabs Navigation */}
      <nav className="analytics-tabs-nav" aria-label="Analytics sections">
        <button
          className={`tab-btn ${activeTab === 'overview' ? 'active' : ''}`}
          onClick={() => setActiveTab('overview')}
        >
          <TrendingUp size={15} />
          <span>Overview</span>
        </button>
        <button
          className={`tab-btn ${activeTab === 'tokens-cost' ? 'active' : ''}`}
          onClick={() => setActiveTab('tokens-cost')}
        >
          <Coins size={15} />
          <span>Token & Cost Tracking</span>
          {kpis.estimated_cost != null && (
            <span className="tab-badge">{formatCost(kpis.estimated_cost)}</span>
          )}
        </button>
        <button
          className={`tab-btn ${activeTab === 'tools' ? 'active' : ''}`}
          onClick={() => setActiveTab('tools')}
        >
          <Terminal size={15} />
          <span>Tool Failure & Execution</span>
          {kpis.tool_error_count > 0 && (
            <span className="tab-badge badge-warning">{kpis.tool_error_count} fails</span>
          )}
        </button>
        <button
          className={`tab-btn ${activeTab === 'productivity' ? 'active' : ''}`}
          onClick={() => setActiveTab('productivity')}
        >
          <Flame size={15} />
          <span>Agent Productivity Pulse</span>
        </button>
        <button
          className={`tab-btn ${activeTab === 'pricing' ? 'active' : ''}`}
          onClick={() => setActiveTab('pricing')}
        >
          <Info size={15} />
          <span>Model Pricing Matrix</span>
        </button>
        <button
          className={`tab-btn ${activeTab === 'overrides' ? 'active' : ''}`}
          onClick={() => setActiveTab('overrides')}
        >
          <SlidersHorizontal size={15} />
          <span>Price Overrides</span>
          {overrides.length > 0 && (
            <span className="tab-badge">{overrides.length} custom</span>
          )}
        </button>
      </nav>

      <>
      {/* Error alert if any */}
      {error && (
        <div className="analytics-alert error">
          <AlertTriangle size={16} />
          <span>{error}</span>
          <button onClick={() => setError('')}><X size={14} /></button>
        </div>
      )}

      {/* Loading state skeleton */}
      {loading && !data && (
        <div className="analytics-loading-placeholder">
          <div className="loading-spinner"><RefreshCw className="spin" size={24} /></div>
          <strong>Aggregating session metrics…</strong>
          <span>Calculating tokens and estimating costs</span>
        </div>
      )}

      {/* Main Content Area */}
      {data && (
        <div className="analytics-content-scroll">
          {activeTab === 'overview' && (
            <OverviewSection
              kpis={kpis}
              tokensAndCosts={tokensAndCosts}
              toolMetrics={toolMetrics}
              productivity={productivity}
              trendMetric={trendMetric}
              setTrendMetric={setTrendMetric}
              hoveredTrendPoint={hoveredTrendPoint}
              setHoveredTrendPoint={setHoveredTrendPoint}
              onNavigate={setActiveTab}
              sessionLeaderboards={data.session_leaderboards || {}}
              onSelectSession={onSelectSession}
            />
          )}

          {activeTab === 'tokens-cost' && (
            <TokenCostSection
              kpis={kpis}
              tokensAndCosts={tokensAndCosts}
              trendMetric={trendMetric}
              setTrendMetric={setTrendMetric}
              hoveredTrendPoint={hoveredTrendPoint}
              setHoveredTrendPoint={setHoveredTrendPoint}
            />
          )}

          {activeTab === 'tools' && (
            <ToolExecutionSection
              kpis={kpis}
              toolMetrics={toolMetrics}
              copiedCmd={copiedCmd}
              onCopyCommand={copyCommand}
              onSelectSession={onSelectSession}
            />
          )}

          {activeTab === 'productivity' && (
            <ProductivitySection
              kpis={kpis}
              productivity={productivity}
              hoveredHeatCell={hoveredHeatCell}
              setHoveredHeatCell={setHoveredHeatCell}
            />
          )}

          {activeTab === 'pricing' && (
            <PricingMatrixSection
              pricingList={tokensAndCosts.pricing_reference || []}
              methodology={tokensAndCosts.methodology}
            />
          )}

          {activeTab === 'overrides' && (
            <PriceOverridesSection
              overrides={overrides}
              overridesLoading={overridesLoading || overrideSaving}
              sessionModels={tokensAndCosts.by_model || []}
              methodology={tokensAndCosts.methodology}
              onSave={handleSaveOverride}
              onReset={handleResetOverrides}
              onNavigate={setActiveTab}
            />
          )}
        </div>
      )}
      </>
    </div>
  );
}

function SessionLeaderboards({ boards = {}, onSelectSession }) {
  const categories = [
    ['estimated_cost', 'Most Expensive', row => formatCost(row.estimated_cost)],
    ['total_tokens', 'Most Tokens', row => `${formatNum(row.total_tokens)} tokens`],
    ['duration_seconds', 'Longest Sessions', row => formatDuration(row.duration_seconds * 1000)],
    ['total_turns', 'Most Turns', row => `${formatNum(row.total_turns)} turns`],
  ];
  return <div className="two-column-grid">{categories.map(([key, title, format]) => (
    <div className="analytics-card" key={key}><h3>{title}</h3><span className="card-subtext">Top 5 sessions in the selected range</span>
      <div className="analytics-leaderboard">{(boards[key] || []).map((row, i) => (
        <button type="button" className="analytics-session-leader" key={row.session_id} onClick={() => onSelectSession?.(row.session_id)}>
          <span className="analytics-leader-rank">{i + 1}</span>
          <span className="analytics-leader-info"><strong>{row.session_title || `Session ${row.session_id}`}</strong>
            <span>{row.family} · {row.workspace} · {row.model_name || 'Unknown model'}</span></span>
          <strong>{format(row)}</strong><ExternalLink size={12} />
        </button>
      ))}</div>
      {!boards[key]?.length && <div className="empty-submessage">No sessions in this period.</div>}
    </div>
  ))}</div>;
}

function SpendOutlook({ outlook }) {
  if (!outlook) return null;
  const metrics = [['estimated_cost', 'Spend', formatCost], ['total_tokens', 'Tokens', formatNum], ['sessions', 'Sessions', formatNum], ['turns', 'Turns', formatNum]];
  return <div className="analytics-card">
    <h3>Week over Week & Spend Outlook</h3>
    <span className="card-subtext">{outlook.recent_start} – {outlook.recent_end} vs {outlook.prior_start} – {outlook.prior_end}.</span>
    <div className="analytics-outlook-grid">{metrics.map(([key, label, format]) => {
      const delta = outlook.weekly_deltas[key];
      return <div key={key}><span>{label}</span><strong>{format(delta.current)}</strong>
        <span>{delta.pct_change == null ? 'New activity (prior week was zero)' : `${delta.pct_change > 0 ? '+' : ''}${delta.pct_change.toFixed(1)}% vs prior 7d`} · prior {format(delta.previous)}</span></div>;
    })}
      <div><span>7-day daily spend average</span><strong>{formatCost(outlook.daily_average_cost)}</strong><span>Includes days with no sessions</span></div>
      <div><span>Forecast month-end spend</span><strong>{formatCost(outlook.forecast_month_end_cost)}</strong><span>{formatCost(outlook.month_to_date_cost)} so far + remaining days at the 7-day average</span></div>
    </div>
  </div>;
}

function EfficiencyTable({ title, byModel = [], byAgent = [], byWorkspace = [], cache = false }) {
  const [group, setGroup] = useState('model');
  const sorting = useNumericSort(group === 'model' ? byModel : group === 'agent' ? byAgent : byWorkspace);
  const rows = sorting.rows;
  const columns = cache ? [['cache_hit_tokens', 'Cached input'], ['cache_hit_pct', 'Cache hit %'], ['cache_savings', 'Estimated savings'], ['potential_cache_savings', 'Potential additional savings']]
    : [['total_turns', 'Turns'], ['total_duration_seconds', 'Session hours'], ['active_days', 'Active days'], ['cost_per_turn', 'Cost / turn'], ['cost_per_hour', 'Cost / hour'], ['cost_per_day', 'Cost / active day']];
  const label = row => group === 'model' ? row.model_name || 'Unknown model' : group === 'agent' ? row.product || row.family : row.name;
  const rate = value => value == null ? '—' : value === 0 ? '$0.00' : `$${value.toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 4 })}`;
  return <div className="analytics-card">
    <div className="card-toolbar"><div><h3>{title}</h3><span className="card-subtext">{cache
      ? 'Hit % = cached input ÷ input tokens. Savings use each model’s input and cache prices; potential savings assumes all remaining input could be cached.'
      : 'Cost ÷ total turns, summed session hours, or active calendar days. Session hours include idle time; — means no denominator.'}</span></div>
      <div className="metric-toggle-group">{(cache ? ['model', 'agent'] : ['model', 'agent', 'workspace']).map(key => (
        <button key={key} className={`metric-toggle ${group === key ? 'active' : ''}`} onClick={() => setGroup(key)}>By {key}</button>
      ))}</div>
    </div>
    <div className="table-responsive"><table className="analytics-table compact">
      <thead><tr><th>{group}</th>{columns.map(([key, name]) => <NumericSortHeader key={key} column={key} sorting={sorting}>{name}</NumericSortHeader>)}</tr></thead>
      <tbody>{rows.map((row, i) => <tr key={`${label(row)}-${i}`}><td><strong>{label(row)}</strong></td>
        {cache ? <><td className="num-col">{formatNum(row.cache_hit_tokens)}</td><td className="num-col">{(row.cache_hit_pct || 0).toFixed(1)}%</td><td className="num-col">{formatCost(row.cache_savings)}</td><td className="num-col">{formatCost(row.potential_cache_savings)}</td></>
          : <><td className="num-col">{formatNum(row.total_turns)}</td><td className="num-col">{(row.total_duration_seconds / 3600).toFixed(1)}</td><td className="num-col">{row.active_days}</td><td className="num-col">{rate(row.cost_per_turn)}</td><td className="num-col">{rate(row.cost_per_hour)}</td><td className="num-col">{rate(row.cost_per_day)}</td></>}
      </tr>)}</tbody>
    </table>{!rows.length && <div className="empty-submessage">No sessions in this period.</div>}</div>
  </div>;
}

// -------------------------------------------------------------
// 1. OVERVIEW SECTION
// -------------------------------------------------------------
function OverviewSection({
  kpis,
  tokensAndCosts,
  toolMetrics,
  productivity,
  trendMetric,
  setTrendMetric,
  hoveredTrendPoint,
  setHoveredTrendPoint,
  onNavigate,
  sessionLeaderboards,
  onSelectSession
}) {
  return (
    <div className="tab-pane-animate">
      {/* KPI Cards Grid */}
      <div className="kpi-grid">
        <div className="kpi-card highlight-card">
          <div className="kpi-header">
            <span className="kpi-label">ESTIMATED TOTAL COST</span>
            <span className="kpi-icon"><DollarSign size={16} /></span>
          </div>
          <div className="kpi-value">{formatCost(kpis.estimated_cost)}</div>
          <div className="kpi-meta">
            <span>Avg {formatCost(kpis.avg_cost_per_session)} / session</span>
            <span className="meta-badge">{kpis.total_sessions?.toLocaleString()} sessions</span>
          </div>
        </div>

        <div className="kpi-card">
          <div className="kpi-header">
            <span className="kpi-label">TOTAL TOKENS</span>
            <span className="kpi-icon"><Coins size={16} /></span>
          </div>
          <div className="kpi-value">{formatNum(kpis.total_tokens)}</div>
          <div className="kpi-meta">
            <span>In: {formatNum(kpis.input_tokens)} · Out: {formatNum(kpis.output_tokens)}</span>
          </div>
        </div>

        <div className="kpi-card">
          <div className="kpi-header">
            <span className="kpi-label">TOOL FAILURE RATE</span>
            <span className="kpi-icon"><Terminal size={16} /></span>
          </div>
          <div className="kpi-value error-color">
            {kpis.tool_error_rate}%
          </div>
          <div className="kpi-meta">
            <span>{kpis.tool_error_count?.toLocaleString()} failed of {formatNum(kpis.total_tool_calls)} calls</span>
          </div>
        </div>

        <div className="kpi-card">
          <div className="kpi-header">
            <span className="kpi-label">PRODUCTIVITY PULSE</span>
            <span className="kpi-icon"><Flame size={16} /></span>
          </div>
          <div className="kpi-value">{formatNum(kpis.total_turns)}</div>
          <div className="kpi-meta">
            <span>{kpis.avg_turns_per_session} turns/session avg · {kpis.total_duration_hours}h work</span>
          </div>
        </div>
      </div>

      <SpendOutlook outlook={tokensAndCosts.spend_outlook} />
      <SessionLeaderboards boards={sessionLeaderboards} onSelectSession={onSelectSession} />

      {/* Main Trend Chart */}
      <div className="analytics-card main-chart-card">
        <div className="card-toolbar">
          <div>
            <h3>Activity & Usage Trends</h3>
            <span className="card-subtext">Daily trajectory across sessions, tokens, and estimated cost</span>
          </div>
          <div className="metric-toggle-group">
            <button
              className={`metric-toggle ${trendMetric === 'cost' ? 'active' : ''}`}
              onClick={() => setTrendMetric('cost')}
            >
              Cost ($)
            </button>
            <button
              className={`metric-toggle ${trendMetric === 'tokens' ? 'active' : ''}`}
              onClick={() => setTrendMetric('tokens')}
            >
              Tokens
            </button>
            <button
              className={`metric-toggle ${trendMetric === 'sessions' ? 'active' : ''}`}
              onClick={() => setTrendMetric('sessions')}
            >
              Sessions
            </button>
          </div>
        </div>

        <TrendChart
          data={tokensAndCosts.daily_trend || []}
          metric={trendMetric}
          hoveredPoint={hoveredTrendPoint}
          setHoveredPoint={setHoveredTrendPoint}
        />
      </div>

      {/* 2-Column Split: Top Models & Top Failing Commands */}
      <div className="two-column-grid">
        {/* Model Breakdown Summary */}
        <div className="analytics-card">
          <div className="card-head-row">
            <div>
              <h3>Model Cost & Token Leaders</h3>
              <span className="card-subtext">Comparison across LLM families</span>
            </div>
            <button className="card-action-btn" onClick={() => onNavigate('tokens-cost')}>
              Full details <ArrowUpRight size={13} />
            </button>
          </div>

          <div className="model-summary-list">
            {(tokensAndCosts.by_model || []).slice(0, 5).map((m) => (
              <div className="model-row" key={m.model_name}>
                <div className="model-info">
                  <div className="model-name-title">
                    <span className="model-dot" />
                    <strong>{m.model_name}</strong>
                  </div>
                  <span className="model-details">
                    {formatNum(m.total_tokens)} tokens · {m.sessions} sessions
                  </span>
                </div>
                <div className="model-cost-col">
                  <strong>{formatCost(m.estimated_cost)}</strong>
                  <span className="cost-bar-outer">
                    <span
                      className="cost-bar-inner"
                      style={{ width: `${Math.min(100, Math.max(4, m.cost_pct || 0))}%` }}
                    />
                  </span>
                </div>
              </div>
            ))}
          </div>
        </div>

        {/* Top Failing Commands Quick Preview */}
        <div className="analytics-card">
          <div className="card-head-row">
            <div>
              <h3>Top Failing Commands</h3>
              <span className="card-subtext">Most frequent execution errors</span>
            </div>
            <button className="card-action-btn" onClick={() => onNavigate('tools')}>
              All failures <ArrowUpRight size={13} />
            </button>
          </div>

          <div className="failing-commands-list">
            {(toolMetrics.top_failing_bash || []).slice(0, 4).map((cmd, idx) => (
              <div className="failing-cmd-item" key={idx}>
                <div className="failing-cmd-head">
                  <span className="fail-badge">{cmd.fail_count} failures</span>
                  <span className="fail-tool-name">{cmd.tool_name}</span>
                </div>
                <code className="cmd-code-line">{cmd.command}</code>
              </div>
            ))}
            {!(toolMetrics.top_failing_bash?.length) && (
              <div className="empty-submessage">
                <CheckCircle2 size={16} /> No tool failures recorded in this period.
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

// -------------------------------------------------------------
// 2. TOKEN & COST TRACKING SECTION
// -------------------------------------------------------------
function TokenCostSection({
  kpis,
  tokensAndCosts,
  trendMetric,
  setTrendMetric,
  hoveredTrendPoint,
  setHoveredTrendPoint
}) {
  const byModel = tokensAndCosts.by_model || [];
  const byAgent = tokensAndCosts.by_agent || [];
  const byWorkspace = tokensAndCosts.by_workspace || [];
  const modelSorting = useNumericSort(byModel);

  return (
    <div className="tab-pane-animate">
      <MethodologyNote methodology={tokensAndCosts.methodology} />
      {/* Key Metric Strip */}
      <div className="kpi-grid">
        <div className="kpi-card highlight-card">
          <div className="kpi-header">
            <span className="kpi-label">TOTAL ESTIMATED SPEND</span>
            <span className="kpi-icon"><DollarSign size={16} /></span>
          </div>
          <div className="kpi-value">{formatCost(kpis.estimated_cost)}</div>
          <div className="kpi-meta">
            <span>Across {byModel.length} active models</span>
          </div>
        </div>

        <div className="kpi-card">
          <div className="kpi-header">
            <span className="kpi-label">INPUT TOKENS</span>
            <span className="kpi-icon"><Coins size={16} /></span>
          </div>
          <div className="kpi-value">{formatNum(kpis.input_tokens)}</div>
          <div className="kpi-meta">
            <span>Cached: {formatNum(kpis.cached_tokens)} · {(kpis.cache_hit_pct || 0).toFixed(1)}% hit</span>
          </div>
        </div>

        <div className="kpi-card">
          <div className="kpi-header">
            <span className="kpi-label">OUTPUT TOKENS</span>
            <span className="kpi-icon"><Coins size={16} /></span>
          </div>
          <div className="kpi-value">{formatNum(kpis.output_tokens)}</div>
          <div className="kpi-meta">
            <span>Assistant generation</span>
          </div>
        </div>

        <div className="kpi-card">
          <div className="kpi-header">
            <span className="kpi-label">AVG COST PER SESSION</span>
            <span className="kpi-icon"><Activity size={16} /></span>
          </div>
          <div className="kpi-value">{formatCost(kpis.avg_cost_per_session)}</div>
          <div className="kpi-meta">
            <span>{formatNum(kpis.total_sessions)} total sessions</span>
          </div>
        </div>
      </div>

      <EfficiencyTable title="Cache Efficiency" byModel={byModel} byAgent={byAgent} cache />
      <EfficiencyTable title="Cost per Turn, Hour & Day" byModel={byModel} byAgent={byAgent} byWorkspace={byWorkspace} />

      {/* Model Comparison Table & Chart */}
      <div className="analytics-card">
        <div className="card-header-with-badge">
          <div>
            <h3>Token & Cost Breakdown by Model</h3>
            <span className="card-subtext">
              Direct comparison of token volumes, generation costs, and pricing tiers
            </span>
          </div>
          <span className="badge-pill">{byModel.length} Models Tracked</span>
        </div>

        <div className="table-responsive">
          <table className="analytics-table">
            <thead>
              <tr>
                <th>Model</th>
                <th>Pricing Tier</th>
                <NumericSortHeader column="sessions" sorting={modelSorting}>Sessions</NumericSortHeader>
                <NumericSortHeader column="input_tokens" sorting={modelSorting}>Input Tokens</NumericSortHeader>
                <NumericSortHeader column="output_tokens" sorting={modelSorting}>Output Tokens</NumericSortHeader>
                <NumericSortHeader column="total_tokens" sorting={modelSorting}>Total Tokens</NumericSortHeader>
                <NumericSortHeader column="estimated_cost" sorting={modelSorting}>Estimated Cost</NumericSortHeader>
                <NumericSortHeader column="cost_pct" sorting={modelSorting}>Share of Spend</NumericSortHeader>
              </tr>
            </thead>
            <tbody>
              {modelSorting.rows.map((m) => (
                <tr key={m.model_name}>
                  <td>
                    <div className="model-name-cell">
                      <span className="model-dot" />
                      <strong>{m.model_name}</strong>
                    </div>
                  </td>
                  <td>
                    <span className="tier-tag">{m.pricing?.displayName || 'Standard'}</span>
                  </td>
                  <td className="num-col">{m.sessions.toLocaleString()}</td>
                  <td className="num-col">{formatNum(m.input_tokens)}</td>
                  <td className="num-col">{formatNum(m.output_tokens)}</td>
                  <td className="num-col"><strong>{formatNum(m.total_tokens)}</strong></td>
                  <td className="num-col"><strong className="cost-highlight">{formatCost(m.estimated_cost)}</strong></td>
                  <td className="num-col">
                    <div className="share-cell">
                      <span>{(m.cost_pct || 0).toFixed(1)}%</span>
                      <div className="progress-bar-small">
                        <div className="bar-fill" style={{ width: `${Math.min(100, m.cost_pct || 0)}%` }} />
                      </div>
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>

      {/* Two Columns: Spend by Agent & Spend by Workspace */}
      <div className="two-column-grid">
        {/* By Agent */}
        <div className="analytics-card">
          <h3>Token & Cost by Agent Family</h3>
          <span className="card-subtext">Codex vs Claude Code vs OpenCode vs Pi</span>

          <div className="agent-breakdown-list">
            {byAgent.map((a) => (
              <div className="agent-breakdown-item" key={a.family}>
                <div className="agent-item-top">
                  <div className="agent-badge-name">
                    <span className={`agent-chip-dot ${a.family}`} />
                    <strong>{a.product || a.family}</strong>
                    <span className="sessions-count-chip">{a.sessions} sessions</span>
                  </div>
                  <strong className="agent-cost">{formatCost(a.estimated_cost)}</strong>
                </div>

                <div className="agent-token-line">
                  <span>Tokens: {formatNum(a.total_tokens)} ({formatNum(a.input_tokens)} in / {formatNum(a.output_tokens)} out)</span>
                  <span>{(a.cost_pct || 0).toFixed(1)}% of total</span>
                </div>

                <div className="progress-bar-small">
                  <div className="bar-fill" style={{ width: `${Math.min(100, a.cost_pct || 0)}%` }} />
                </div>
              </div>
            ))}
          </div>
        </div>

        {/* By Workspace */}
        <div className="analytics-card">
          <h3>Top Workspaces by Token Spend</h3>
          <span className="card-subtext">Repositories consuming the most agent compute</span>

          <div className="workspace-breakdown-list">
            {byWorkspace.slice(0, 8).map((w, idx) => (
              <div className="workspace-item" key={idx}>
                <div className="workspace-head">
                  <strong className="workspace-title">{w.name}</strong>
                  <strong className="workspace-cost">{formatCost(w.estimated_cost)}</strong>
                </div>
                <div className="workspace-meta">
                  <span className="workspace-path-truncate" title={w.path}>{w.path}</span>
                  <span>{formatNum(w.total_tokens)} tokens</span>
                </div>
                <div className="progress-bar-small">
                  <div className="bar-fill mint" style={{ width: `${Math.min(100, w.cost_pct || 0)}%` }} />
                </div>
              </div>
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}

// -------------------------------------------------------------
// 3. TOOL FAILURE & EXECUTION METRICS SECTION
// -------------------------------------------------------------
function ToolExecutionSection({
  kpis,
  toolMetrics,
  copiedCmd,
  onCopyCommand,
  onSelectSession
}) {
  const failingBash = toolMetrics.top_failing_bash || [];
  const errorSorting = useNumericSort(toolMetrics.tool_error_rates || []);
  const errorRates = errorSorting.rows;
  const latencySorting = useNumericSort(toolMetrics.latency_distribution || []);
  const slowestTools = toolMetrics.slowest_tools || [];
  const toolStatuses = toolMetrics.tool_statuses || [];

  return (
    <div className="tab-pane-animate">
      {/* Tool Metrics KPI Cards */}
      <div className="kpi-grid">
        <div className="kpi-card">
          <div className="kpi-header">
            <span className="kpi-label">TOTAL INVOCATIONS</span>
            <span className="kpi-icon"><Terminal size={16} /></span>
          </div>
          <div className="kpi-value">{formatNum(kpis.total_tool_calls)}</div>
          <div className="kpi-meta">
            <span>Across all sessions and agents</span>
          </div>
        </div>

        <div className="kpi-card highlight-card">
          <div className="kpi-header">
            <span className="kpi-label">FAILURE / ERROR RATE</span>
            <span className="kpi-icon"><AlertTriangle size={16} /></span>
          </div>
          <div className="kpi-value error-color">{kpis.tool_error_rate}%</div>
          <div className="kpi-meta">
            <span>{kpis.tool_error_count?.toLocaleString()} failed invocations</span>
          </div>
        </div>

        <div className="kpi-card">
          <div className="kpi-header">
            <span className="kpi-label">EXECUTION SUCCESS RATE</span>
            <span className="kpi-icon"><CheckCircle2 size={16} /></span>
          </div>
          <div className="kpi-value success-color">
            {(100 - (kpis.tool_error_rate || 0)).toFixed(1)}%
          </div>
          <div className="kpi-meta">
            <span>Commands finished without errors</span>
          </div>
        </div>

        <div className="kpi-card">
          <div className="kpi-header">
            <span className="kpi-label">STATUS BREAKDOWN</span>
            <span className="kpi-icon"><Activity size={16} /></span>
          </div>
          <div className="kpi-status-chips">
            {toolStatuses.slice(0, 3).map((st) => (
              <span key={st.status} className={`status-pill status-${st.status}`}>
                {st.status}: {formatNum(st.count)}
              </span>
            ))}
          </div>
        </div>
      </div>

      <div className="analytics-card">
        <h3>Tool Latency Distribution</h3>
        <span className="card-subtext">Nearest-rank p50 / p95 across all recorded durations, including calls over 10 minutes. Missing timings are excluded.</span>
        <div className="table-responsive analytics-distribution-scroll">
          <table className="analytics-table compact">
            <thead><tr><th>Tool</th>{[['samples', 'Timed calls'], ['p50_duration_ms', 'p50'], ['p95_duration_ms', 'p95'], ['avg_duration_ms', 'Average'], ['max_duration_ms', 'Maximum']].map(([key, label]) => (
              <NumericSortHeader key={key} column={key} sorting={latencySorting}>{label}</NumericSortHeader>
            ))}</tr></thead>
            <tbody>{latencySorting.rows.map(t => (
              <tr key={t.tool_name}><td><span className="tool-code-badge">{t.tool_name}</span></td>
                <td className="num-col">{formatNum(t.samples)}</td>
                <td className="num-col">{formatDuration(t.p50_duration_ms)}</td>
                <td className="num-col">{formatDuration(t.p95_duration_ms)}</td>
                <td className="num-col">{formatDuration(t.avg_duration_ms)}</td>
                <td className="num-col">{formatDuration(t.max_duration_ms)}</td></tr>
            ))}</tbody>
          </table>
          {!toolMetrics.latency_distribution?.length && <div className="empty-submessage">No recorded tool durations in this period.</div>}
        </div>
      </div>

      {/* Top Failing Bash Commands Dashboard */}
      <div className="analytics-card">
        <div className="card-header-with-badge">
          <div>
            <h3>Top Failing Bash Commands</h3>
            <span className="card-subtext">
              Frequent command crashes, syntax errors, or non-zero exit codes
            </span>
          </div>
          <span className="badge-pill warning">{failingBash.length} Failing Commands</span>
        </div>

        <div className="failing-commands-grid">
          {failingBash.map((item, idx) => (
            <div className="command-fail-card" key={idx}>
              <div className="command-fail-header">
                <div className="fail-tag-row">
                  <span className="fail-count-tag">{item.fail_count} failures</span>
                  <span className="tool-type-tag">{item.tool_name}</span>
                </div>
                <button
                  className="copy-cmd-btn"
                  onClick={() => onCopyCommand(item.command)}
                  title="Copy command to clipboard"
                >
                  {copiedCmd === item.command ? <Check size={13} /> : <Copy size={13} />}
                  <span>{copiedCmd === item.command ? 'Copied' : 'Copy'}</span>
                </button>
              </div>

              <pre className="command-text-block"><code>$ {item.command}</code></pre>

              {item.sample_error && (
                <div className="command-error-snippet">
                  <span className="error-indicator">Error output:</span>
                  <p>{item.sample_error}</p>
                </div>
              )}

              {item.last_failed_at && (
                <div className="command-card-footer">
                  <Clock size={11} />
                  <span>Last failed: {parseDate(item.last_failed_at)?.toLocaleDateString()} {parseDate(item.last_failed_at)?.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}</span>
                </div>
              )}
            </div>
          ))}
          {!failingBash.length && (
            <div className="empty-submessage large">
              <CheckCircle2 size={24} />
              <strong>No failing bash commands found!</strong>
              <span>All shell operations completed with code 0 in the selected period.</span>
            </div>
          )}
        </div>
      </div>

      {/* Two Column Grid: Error Rates by Tool & Slowest Invocations */}
      <div className="two-column-grid">
        {/* Tool Error Rates Table */}
        <div className="analytics-card">
          <h3>Tool Error Rates by Category</h3>
          <span className="card-subtext">Comparison of failure probabilities across tool types</span>

          <div className="table-responsive">
            <table className="analytics-table compact">
              <thead>
                <tr>
                  <th>Tool</th>
                  <NumericSortHeader column="total_calls" sorting={errorSorting}>Total Calls</NumericSortHeader>
                  <NumericSortHeader column="error_calls" sorting={errorSorting}>Failures</NumericSortHeader>
                  <NumericSortHeader column="error_rate" sorting={errorSorting}>Error Rate</NumericSortHeader>
                </tr>
              </thead>
              <tbody>
                {errorRates.map((t) => (
                  <tr key={t.tool_name}>
                    <td>
                      <span className="tool-code-badge">{t.tool_name}</span>
                    </td>
                    <td className="num-col">{t.total_calls.toLocaleString()}</td>
                    <td className="num-col error-color">{t.error_calls.toLocaleString()}</td>
                    <td className="num-col">
                      <div className="error-rate-visual">
                        <span className={t.error_rate > 10 ? 'high-error' : ''}>
                          {t.error_rate}%
                        </span>
                        <span className="error-bar-outer">
                          <span
                            className="error-bar-inner"
                            style={{ width: `${Math.min(100, t.error_rate)}%` }}
                          />
                        </span>
                      </div>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>

        {/* Slowest Tool Calls */}
        <div className="analytics-card">
          <h3>Slowest Tool Calls</h3>
          <span className="card-subtext">Longest duration operations impacting productivity</span>

          <div className="slowest-tools-list">
            {slowestTools.map((t) => (
              <div
                className="slowest-tool-row"
                key={t.id}
                onClick={() => onSelectSession?.(t.session_id)}
                title="Click to jump to this session"
              >
                <div className="slowest-tool-head">
                  <div className="slow-name-col">
                    <span className="tool-code-badge">{t.tool_name}</span>
                    <strong className="slow-duration">{formatDuration(t.duration_ms)}</strong>
                  </div>
                  <span className={`status-pill status-${t.status || 'invoked'}`}>
                    {t.status || 'invoked'}
                  </span>
                </div>

                <div className="slow-cmd-preview">
                  <code>{t.command}</code>
                </div>

                {t.session_title && (
                  <div className="slow-session-ref">
                    <span>Session: {t.session_title.slice(0, 60)}…</span>
                    <ExternalLink size={11} />
                  </div>
                )}
              </div>
            ))}
            {!slowestTools.length && (
              <div className="empty-submessage">
                <Clock size={16} /> No tool duration data available.
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

// -------------------------------------------------------------
// 4. AGENT PRODUCTIVITY PULSE SECTION
// -------------------------------------------------------------
function ProductivitySection({
  kpis,
  productivity,
  hoveredHeatCell,
  setHoveredHeatCell
}) {
  const heatmap = productivity.heatmap || [];
  const durationDist = productivity.duration_distribution || [];
  const turnsByProject = productivity.turns_by_project || [];

  return (
    <div className="tab-pane-animate">
      {/* Productivity Pulse Callout */}
      <div className="productivity-banner">
        <div className="pulse-icon-orb"><Flame size={20} /></div>
        <div className="pulse-banner-content">
          <strong>Productivity Rhythm: Peak Activity on {productivity.peak_day || 'Weekday'}s around {productivity.peak_hour || '14:00'}</strong>
          <span>
            {kpis.total_turns?.toLocaleString()} turns across {kpis.active_days} active days. Average session duration is {kpis.avg_duration_minutes || 0} minutes.
          </span>
        </div>
      </div>

      {/* 7x24 Activity Heatmap Card */}
      <div className="analytics-card">
        <div className="card-header-with-badge">
          <div>
            <h3>Weekly Session Activity Heatmap</h3>
            <span className="card-subtext">
              Concentration of sessions and turns by day of week and hour of day
            </span>
          </div>
          <div className="heat-legend">
            <span>Less</span>
            <span className="heat-box level-0" />
            <span className="heat-box level-1" />
            <span className="heat-box level-2" />
            <span className="heat-box level-3" />
            <span className="heat-box level-4" />
            <span>More</span>
          </div>
        </div>

        <ActivityHeatmap
          heatmapData={heatmap}
          hoveredCell={hoveredHeatCell}
          setHoveredCell={setHoveredHeatCell}
        />
      </div>

      <div className="two-column-grid">
        <div className="analytics-card">
          <h3>Turn Depth Distribution</h3>
          <span className="card-subtext">Total recorded turns per session; 20+ means more than 20.</span>
          <div className="duration-bars-list">{(productivity.turn_depth_distribution || []).map(bucket => (
            <div className="duration-bar-item" key={bucket.label}>
              <div className="duration-label-row"><span className="duration-bucket-label">{bucket.label} turns</span>
                <div className="duration-stat-values"><strong>{formatNum(bucket.count)} sessions</strong><span className="bucket-pct">({bucket.pct.toFixed(1)}%)</span></div></div>
              <div className="progress-bar-large"><div className="bar-fill duration-color" style={{ width: `${bucket.pct}%` }} /></div>
            </div>
          ))}</div>
        </div>
        <div className="analytics-card">
          <h3>User / Assistant Ratio over Time</h3>
          <span className="card-subtext">Daily turns grouped by session start date. Ratio = user turns ÷ assistant turns.</span>
          <div className="table-responsive analytics-distribution-scroll"><table className="analytics-table compact">
            <thead><tr><th>Date</th><th className="num-col">User</th><th className="num-col">Assistant</th><th className="num-col">Ratio</th><th>Mix</th></tr></thead>
            <tbody>{(productivity.turn_ratio_trend || []).map(day => {
              const total = day.user_turns + day.assistant_turns;
              return <tr key={day.date}><td>{day.date}</td><td className="num-col">{formatNum(day.user_turns)}</td>
                <td className="num-col">{formatNum(day.assistant_turns)}</td><td className="num-col">{day.user_assistant_ratio == null ? '—' : day.user_assistant_ratio.toFixed(2)}</td>
                <td><div className="stacked-turn-bar analytics-ratio-bar" title={`${day.user_turns} user / ${day.assistant_turns} assistant`}>
                  <div className="user-turn-segment" style={{ width: `${total ? day.user_turns / total * 100 : 0}%` }} />
                  <div className="agent-turn-segment" style={{ width: `${total ? day.assistant_turns / total * 100 : 0}%` }} />
                </div></td></tr>;
            })}</tbody></table>
            {!productivity.turn_ratio_trend?.length && <div className="empty-submessage">No turns in this period.</div>}
          </div>
        </div>
      </div>

      {/* Two Column Grid: Durations Distribution & Turns by Project */}
      <div className="two-column-grid">
        {/* Session Durations Distribution */}
        <div className="analytics-card">
          <h3>Session Durations Distribution</h3>
          <span className="card-subtext">How long do agent programming sessions typically last?</span>

          <div className="duration-bars-list">
            {durationDist.map((bucket) => (
              <div className="duration-bar-item" key={bucket.label}>
                <div className="duration-label-row">
                  <span className="duration-bucket-label">{bucket.label}</span>
                  <div className="duration-stat-values">
                    <strong>{bucket.count.toLocaleString()} sessions</strong>
                    <span className="bucket-pct">({bucket.pct}%)</span>
                  </div>
                </div>

                <div className="progress-bar-large">
                  <div
                    className="bar-fill duration-color"
                    style={{ width: `${Math.min(100, Math.max(3, bucket.pct))}%` }}
                  />
                </div>

                <div className="bucket-subtext">
                  Avg {bucket.avg_turns} turns per session
                </div>
              </div>
            ))}
          </div>
        </div>

        {/* Total Turns per Project */}
        <div className="analytics-card">
          <h3>Total Turns per Project</h3>
          <span className="card-subtext">Developer engagement & turn volume by workspace</span>

          <div className="turns-project-list">
            {turnsByProject.map((p, idx) => (
              <div className="turns-project-item" key={idx}>
                <div className="project-turn-header">
                  <strong className="project-title">{p.name}</strong>
                  <strong className="turn-count">{p.total_turns.toLocaleString()} turns</strong>
                </div>

                <div className="project-turn-meta">
                  <span>{p.sessions} sessions · {p.avg_turns_per_session} turns/session avg</span>
                  <span>{p.total_hours} hrs</span>
                </div>

                <div className="stacked-turn-bar">
                  <div
                    className="user-turn-segment"
                    style={{ width: `${p.total_turns > 0 ? (p.user_turns / p.total_turns) * 100 : 50}%` }}
                    title={`User turns: ${p.user_turns}`}
                  />
                  <div
                    className="agent-turn-segment"
                    style={{ width: `${p.total_turns > 0 ? (p.assistant_turns / p.total_turns) * 100 : 50}%` }}
                    title={`Assistant turns: ${p.assistant_turns}`}
                  />
                </div>
              </div>
            ))}
            {!turnsByProject.length && (
              <div className="empty-submessage">
                <Activity size={16} /> No workspace activity recorded.
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

// -------------------------------------------------------------
// Shared methodology footnote: pricing provenance + token estimation.
// -------------------------------------------------------------
function MethodologyNote({ methodology }) {
  const prices = methodology?.pricingSource === 'models.dev'
    ? (<>Model prices from <strong>models.dev</strong>{methodology?.pricingUpdatedAt ? ` (cached ${(parseDate(methodology.pricingUpdatedAt)?.toLocaleDateString() || 'date unavailable')}, ${Number(methodology.pricingModelCount || 0).toLocaleString()} models)` : ''}. </>)
    : (<>Model prices are using <strong>built-in fallback rates</strong> (models.dev unreachable or not synced yet — press <strong>Sync prices</strong>). </>);
  const customs = Number(methodology?.customOverrideCount || 0);
  return (
    <div className="methodology-note">
      <Info size={13} />
      <span>{prices}{customs > 0 && (<>{customs} custom price override{customs === 1 ? '' : 's'} take{customs === 1 ? 's' : ''} precedence over these rates. </>)}{methodology?.tokenNote || 'Token counts use actual reported usage when available; otherwise estimated from text length.'}</span>
    </div>
  );
}

// Shared source badge for effective model rates.
function PricingSourceBadge({ source, title }) {
  const cls = source === 'models.dev' ? 'source-live' : source === 'custom' ? 'source-custom' : 'source-fallback';
  const label = source === 'models.dev' ? 'models.dev' : source === 'custom' ? 'custom' : 'fallback';
  return (
    <span className={`source-pill ${cls}`} title={title || (source === 'custom' ? 'User-configured override rate' : source === 'models.dev' ? 'Live rate from models.dev' : 'Built-in fallback rate')}>
      {label}
    </span>
  );
}

// -------------------------------------------------------------
// 5. MODEL PRICING MATRIX REFERENCE
// -------------------------------------------------------------
function PricingMatrixSection({ pricingList = [], methodology }) {
  const pricingSorting = useNumericSort(pricingList);
  return (
    <div className="tab-pane-animate">
      <MethodologyNote methodology={methodology} />
      <div className="analytics-card">
        <div className="card-header-with-badge">
          <div>
            <h3>Model Token Pricing Reference</h3>
            <span className="card-subtext">
              Reference rates per 1,000,000 tokens (USD) used for estimating cost metrics
            </span>
          </div>
          <span className="badge-pill">USD Rates</span>
        </div>

        <div className="table-responsive">
          <table className="analytics-table">
            <thead>
              <tr>
                <th>Model Family</th>
                <NumericSortHeader column="inPer1M" sorting={pricingSorting}>Input Price / 1M</NumericSortHeader>
                <NumericSortHeader column="outPer1M" sorting={pricingSorting}>Output Price / 1M</NumericSortHeader>
                <NumericSortHeader column="cachePer1M" sorting={pricingSorting}>Cache Read Price / 1M</NumericSortHeader>
                <NumericSortHeader column="typical_cost" sorting={pricingSorting}>Typical Cost per 100K Tokens</NumericSortHeader>
                <th>Source</th>
              </tr>
            </thead>
            <tbody>
              {pricingSorting.rows.map((p) => {
                const inputPrice = numericSortValue(p, 'inPer1M');
                const outputPrice = numericSortValue(p, 'outPer1M');
                const cachePrice = numericSortValue(p, 'cachePer1M');
                const typicalCost = numericSortValue(p, 'typical_cost');
                return (
                  <tr key={p.id}>
                    <td>
                      <div className="model-name-cell">
                        <span className="model-dot" />
                        <strong>{p.name}</strong>
                      </div>
                    </td>
                    <td className="num-col"><strong>{inputPrice == null ? '—' : `$${inputPrice.toFixed(2)}`}</strong></td>
                    <td className="num-col"><strong>{outputPrice == null ? '—' : `$${outputPrice.toFixed(2)}`}</strong></td>
                    <td className="num-col">{cachePrice == null ? '—' : `$${cachePrice.toFixed(3)}`}</td>
                    <td className="num-col">
                      <span className="estimate-chip">
                        {typicalCost == null ? '—' : `~$${typicalCost.toFixed(3)} (80k in / 20k out)`}
                      </span>
                    </td>
                    <td>
                      <PricingSourceBadge
                        source={p.source}
                        title={p.source === 'custom'
                          ? `User override for ${p.matchedModel || p.name}`
                          : p.matchedModel ? `Matched ${p.matchedModel} (${p.provider})` : undefined}
                      />
                      {p.matchedModel && (
                        <div className="matched-model-sub" title={p.matchedModel}>{p.matchedModel}</div>
                      )}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      </div>
    </div>
  );
}

// -------------------------------------------------------------
// 6. CUSTOM PRICE OVERRIDES
// -------------------------------------------------------------
function PriceOverridesSection({
  overrides = [],
  overridesLoading = false,
  sessionModels = [],
  methodology,
  onSave,
  onReset,
  onNavigate
}) {
  const sessionModelSorting = useNumericSort(sessionModels);
  const [provider, setProvider] = useState('');
  const [modelId, setModelId] = useState('');
  const [displayName, setDisplayName] = useState('');
  const [inPrice, setInPrice] = useState('');
  const [outPrice, setOutPrice] = useState('');
  const [cachePrice, setCachePrice] = useState('');
  const [editingKey, setEditingKey] = useState(null);
  const [formError, setFormError] = useState('');
  const [resetProvider, setResetProvider] = useState('');

  const overrideKey = (o) => `${o.provider || ''}|||${o.modelId}`;
  const overrideLabel = (o) => (o.provider ? `${o.provider}/${o.modelId}` : o.modelId);
  const providersInUse = useMemo(
    () => [...new Set(overrides.map((o) => o.provider).filter(Boolean))].sort(),
    [overrides]
  );

  const resetForm = () => {
    setProvider(''); setModelId(''); setDisplayName('');
    setInPrice(''); setOutPrice(''); setCachePrice('');
    setEditingKey(null); setFormError('');
  };

  const fillForm = (o) => {
    setProvider(o.provider || ''); setModelId(o.modelId || '');
    setDisplayName(o.displayName && o.displayName !== o.modelId ? o.displayName : '');
    setInPrice(String(o.inPer1M ?? '')); setOutPrice(String(o.outPer1M ?? ''));
    setCachePrice(String(o.cachePer1M ?? ''));
    setEditingKey(overrideKey(o)); setFormError('');
  };

  // Prefill the form from a model seen in sessions (effective rate included).
  const prefillFromSession = (m) => {
    const matched = m.pricing?.matchedModel || m.model_name || '';
    const clean = matched.replace(/^(live|custom):/, '');
    const parts = clean.split('/');
    fillForm({
      provider: parts.length > 1 ? parts.slice(0, -1).join('/') : '',
      modelId: parts.length > 1 ? parts[parts.length - 1] : clean,
      displayName: m.pricing?.displayName && m.pricing.displayName !== m.model_name ? m.pricing.displayName : '',
      inPer1M: m.pricing?.inPer1M ?? '',
      outPer1M: m.pricing?.outPer1M ?? '',
      cachePer1M: m.pricing?.cachePer1M ?? '',
    });
  };

  const handleSubmit = async (e) => {
    e?.preventDefault();
    const model = modelId.trim();
    const inp = Number(inPrice);
    const outp = Number(outPrice);
    const cachep = cachePrice === '' ? 0 : Number(cachePrice);
    if (!model) { setFormError('Model id is required.'); return; }
    if (!Number.isFinite(inp) || inp < 0) { setFormError('Input price must be a number ≥ 0.'); return; }
    if (!Number.isFinite(outp) || outp < 0) { setFormError('Output price must be a number ≥ 0.'); return; }
    if (!Number.isFinite(cachep) || cachep < 0) { setFormError('Cache price must be a number ≥ 0.'); return; }
    setFormError('');
    const ok = await onSave({
      provider: provider.trim(),
      modelId: model,
      displayName: displayName.trim(),
      inPer1M: inp,
      outPer1M: outp,
      cachePer1M: cachep,
    });
    if (ok) resetForm();
  };

  const resetOne = (o) => onReset(
    { provider: o.provider || '', modelId: o.modelId },
    `Reset the custom price for ${overrideLabel(o)}? Session costs will be recalculated from models.dev / fallback rates.`
  );
  const resetWholeProvider = () => {
    if (!resetProvider) return;
    const n = overrides.filter((o) => o.provider === resetProvider).length;
    onReset(
      { provider: resetProvider },
      `Reset all ${n} custom price override${n === 1 ? '' : 's'} for provider "${resetProvider}"?`
    );
  };
  const resetAll = () => onReset(
    {},
    `Reset all ${overrides.length} custom price override${overrides.length === 1 ? '' : 's'}? Session costs will be recalculated from models.dev / fallback rates.`
  );

  return (
    <div className="tab-pane-animate">
      <MethodologyNote methodology={methodology} />

      {/* Add / edit form */}
      <div className="analytics-card">
        <div className="card-header-with-badge">
          <div>
            <h3>{editingKey ? 'Edit Price Override' : 'Add Price Override'}</h3>
            <span className="card-subtext">
              Custom rates take precedence over models.dev and survive re-syncs. They match exact model ids —
              use <code>provider/model</code> to scope to one provider, or a bare <code>model</code> for all providers.
              Unknown ids (e.g. future or fallback models) work too.
            </span>
          </div>
          <span className="badge-pill">{overrides.length} Custom</span>
        </div>

        <form className="override-form-grid" onSubmit={handleSubmit}>
          <label className="override-field">
            <span>Provider <em>(optional)</em></span>
            <input
              value={provider}
              onChange={(e) => setProvider(e.target.value)}
              placeholder="blank = all providers"
              aria-label="Override provider (optional)"
            />
          </label>
          <label className="override-field">
            <span>Model id <em>(required)</em></span>
            <input
              value={modelId}
              onChange={(e) => setModelId(e.target.value)}
              placeholder="e.g. openai/gpt-4o-mini or gpt-4o-mini"
              aria-label="Override model id"
              disabled={editingKey !== null}
            />
          </label>
          <label className="override-field">
            <span>Display name <em>(optional)</em></span>
            <input
              value={displayName}
              onChange={(e) => setDisplayName(e.target.value)}
              placeholder="defaults to model id"
              aria-label="Override display name"
            />
          </label>
          <label className="override-field override-field-num">
            <span>Input $ / 1M</span>
            <input type="number" min="0" step="any" value={inPrice} onChange={(e) => setInPrice(e.target.value)} placeholder="e.g. 2.50" aria-label="Override input price per million tokens" />
          </label>
          <label className="override-field override-field-num">
            <span>Output $ / 1M</span>
            <input type="number" min="0" step="any" value={outPrice} onChange={(e) => setOutPrice(e.target.value)} placeholder="e.g. 10.00" aria-label="Override output price per million tokens" />
          </label>
          <label className="override-field override-field-num">
            <span>Cache read $ / 1M</span>
            <input type="number" min="0" step="any" value={cachePrice} onChange={(e) => setCachePrice(e.target.value)} placeholder="0" aria-label="Override cache read price per million tokens" />
          </label>
        </form>
        {formError && (
          <div className="analytics-alert error override-form-error">
            <AlertTriangle size={14} />
            <span>{formError}</span>
          </div>
        )}
        <div className="override-form-actions">
          <button className="analytics-refresh-btn" onClick={handleSubmit} disabled={overridesLoading}>
            <Check size={13} />
            <span>{editingKey ? 'Save changes' : 'Add override'}</span>
          </button>
          {editingKey && (
            <button className="override-ghost-btn" onClick={resetForm} disabled={overridesLoading}>
              <X size={13} />
              <span>Cancel</span>
            </button>
          )}
        </div>
      </div>

      {/* Models seen in sessions */}
      <div className="analytics-card">
        <div className="card-header-with-badge">
          <div>
            <h3>Models In Your Sessions</h3>
            <span className="card-subtext">Effective rate per model with one-click override prefill</span>
          </div>
          <button className="card-action-btn" onClick={() => onNavigate('pricing')}>
            Pricing matrix <ArrowUpRight size={13} />
          </button>
        </div>
        <div className="table-responsive">
          <table className="analytics-table compact">
            <thead>
              <tr>
                <th>Model</th>
                <NumericSortHeader column="sessions" sorting={sessionModelSorting}>Sessions</NumericSortHeader>
                <NumericSortHeader column="pricing.inPer1M" sorting={sessionModelSorting}>Input / 1M</NumericSortHeader>
                <NumericSortHeader column="pricing.outPer1M" sorting={sessionModelSorting}>Output / 1M</NumericSortHeader>
                <th>Source</th>
                <th className="num-col">Action</th>
              </tr>
            </thead>
            <tbody>
              {sessionModelSorting.rows.map((m) => (
                <tr key={m.model_name}>
                  <td>
                    <div className="model-name-cell">
                      <span className="model-dot" />
                      <strong>{m.model_name}</strong>
                    </div>
                    {m.pricing?.matchedModel && (
                      <div className="matched-model-sub" title={m.pricing.matchedModel}>{m.pricing.matchedModel}</div>
                    )}
                  </td>
                  <td className="num-col">{m.sessions.toLocaleString()}</td>
                  <td className="num-col">${Number(m.pricing?.inPer1M ?? 0).toFixed(2)}</td>
                  <td className="num-col">${Number(m.pricing?.outPer1M ?? 0).toFixed(2)}</td>
                  <td>
                    <PricingSourceBadge
                      source={m.pricing?.source}
                      title={m.pricing?.source === 'custom'
                        ? `User override for ${m.pricing?.matchedModel || m.model_name}`
                        : m.pricing?.matchedModel ? `Matched ${m.pricing.matchedModel}` : undefined}
                    />
                  </td>
                  <td className="num-col">
                    <button className="override-row-btn" onClick={() => prefillFromSession(m)} title={m.pricing?.source === 'custom' ? 'Edit this override' : 'Prefill an override for this model'}>
                      {m.pricing?.source === 'custom' ? 'Edit' : 'Override'}
                    </button>
                  </td>
                </tr>
              ))}
              {!sessionModels.length && (
                <tr><td colSpan={6}><div className="empty-submessage">No session models in the selected period.</div></td></tr>
              )}
            </tbody>
          </table>
        </div>
      </div>

      {/* Saved overrides */}
      <div className="analytics-card">
        <div className="card-header-with-badge">
          <div>
            <h3>Saved Overrides</h3>
            <span className="card-subtext">Reset a single model, a whole provider, or everything at once</span>
          </div>
          <div className="override-bulk-actions">
            <select
              value={resetProvider}
              onChange={(e) => setResetProvider(e.target.value)}
              aria-label="Provider to reset"
              disabled={!providersInUse.length || overridesLoading}
            >
              <option value="">Provider…</option>
              {providersInUse.map((p) => (<option key={p} value={p}>{p}</option>))}
            </select>
            <button className="override-ghost-btn" onClick={resetWholeProvider} disabled={!resetProvider || overridesLoading} title="Reset all overrides for the selected provider">
              Reset provider
            </button>
            <button className="override-danger-btn" onClick={resetAll} disabled={!overrides.length || overridesLoading} title="Reset every custom price override">
              Reset all
            </button>
          </div>
        </div>
        <div className="table-responsive">
          <table className="analytics-table compact">
            <thead>
              <tr>
                <th>Provider</th>
                <th>Model</th>
                <th className="num-col">Input / 1M</th>
                <th className="num-col">Output / 1M</th>
                <th className="num-col">Cache / 1M</th>
                <th>Updated</th>
                <th className="num-col">Actions</th>
              </tr>
            </thead>
            <tbody>
              {overrides.map((o) => (
                <tr key={overrideKey(o)} className={editingKey === overrideKey(o) ? 'row-editing' : ''}>
                  <td>{o.provider ? (<span className="tool-code-badge">{o.provider}</span>) : (<span className="override-all-scope">all</span>)}</td>
                  <td>
                    <strong>{o.modelId}</strong>
                    {o.displayName && o.displayName !== o.modelId && (
                      <div className="matched-model-sub" title={o.displayName}>{o.displayName}</div>
                    )}
                  </td>
                  <td className="num-col"><strong>${Number(o.inPer1M).toFixed(2)}</strong></td>
                  <td className="num-col"><strong>${Number(o.outPer1M).toFixed(2)}</strong></td>
                  <td className="num-col">${Number(o.cachePer1M).toFixed(3)}</td>
                  <td><span className="override-updated" title={o.updatedAt}>{parseDate(o.updatedAt)?.toLocaleDateString() || '—'}</span></td>
                  <td className="num-col">
                    <div className="override-row-actions">
                      <button className="override-row-btn" onClick={() => fillForm(o)} disabled={overridesLoading}>Edit</button>
                      <button className="override-row-btn danger" onClick={() => resetOne(o)} disabled={overridesLoading} title={`Reset override for ${overrideLabel(o)}`}>Reset</button>
                    </div>
                  </td>
                </tr>
              ))}
              {!overrides.length && (
                <tr><td colSpan={7}>
                  <div className="empty-submessage large">
                    <SlidersHorizontal size={20} />
                    <strong>No custom price overrides yet</strong>
                    <span>Add one above, or prefill from a model in your sessions — it will survive every models.dev sync.</span>
                  </div>
                </td></tr>
              )}
            </tbody>
          </table>
        </div>
        {overridesLoading && (
          <div className="filter-updating-badge" role="status" aria-live="polite">
            <LoaderCircle className="spin" size={13} />
            <span>Updating overrides…</span>
          </div>
        )}
      </div>
    </div>
  );
}

// -------------------------------------------------------------
// CHART COMPONENT: Activity Heatmap (7 Days x 24 Hours)
// -------------------------------------------------------------
function ActivityHeatmap({ heatmapData = [], hoveredCell, setHoveredCell }) {
  // Build 7 x 24 matrix
  const matrix = useMemo(() => {
    const grid = Array.from({ length: 7 }, () => Array.from({ length: 24 }, () => ({ count: 0, turns: 0 })));
    let maxCount = 1;
    for (const item of heatmapData) {
      const d = item.day_of_week;
      const h = item.hour_of_day;
      if (d >= 0 && d < 7 && h >= 0 && h < 24) {
        grid[d][h] = { count: item.count || 0, turns: item.turns || 0 };
        if (item.count > maxCount) maxCount = item.count;
      }
    }
    return { grid, maxCount };
  }, [heatmapData]);

  const hours = [0, 2, 4, 6, 8, 10, 12, 14, 16, 18, 20, 22];

  const getHeatLevel = (count, max) => {
    if (!count || count <= 0) return 0;
    const ratio = count / max;
    if (ratio < 0.15) return 1;
    if (ratio < 0.40) return 2;
    if (ratio < 0.75) return 3;
    return 4;
  };

  return (
    <div className="heatmap-wrapper">
      <div className="heatmap-hours-header">
        <span className="day-col-spacer" />
        {hours.map((h) => (
          <span key={h} className="hour-label">
            {h === 0 ? '12a' : h === 12 ? '12p' : h > 12 ? `${h - 12}p` : `${h}a`}
          </span>
        ))}
      </div>

      <div className="heatmap-grid-body">
        {DAY_SHORT.map((dayName, dayIdx) => (
          <div className="heatmap-day-row" key={dayIdx}>
            <span className="day-row-label">{dayName}</span>
            <div className="heatmap-cells-row">
              {Array.from({ length: 24 }, (_, hour) => {
                const cell = matrix.grid[dayIdx][hour];
                const level = getHeatLevel(cell.count, matrix.maxCount);
                const isHovered = hoveredCell?.day === dayIdx && hoveredCell?.hour === hour;

                return (
                  <div
                    key={hour}
                    className={`heat-cell level-${level} ${isHovered ? 'hovered' : ''}`}
                    onMouseEnter={() => setHoveredCell({ day: dayIdx, hour, ...cell })}
                    onMouseLeave={() => setHoveredCell(null)}
                    title={`${DAY_NAMES[dayIdx]} ${hour}:00 · ${cell.count} sessions, ${cell.turns} turns`}
                  />
                );
              })}
            </div>
          </div>
        ))}
      </div>

      {hoveredCell && (
        <div className="heatmap-tooltip-pill">
          <strong>{DAY_NAMES[hoveredCell.day]} at {hoveredCell.hour}:00</strong>
          <span>{hoveredCell.count} sessions · {hoveredCell.turns?.toLocaleString()} turns</span>
        </div>
      )}
    </div>
  );
}

// -------------------------------------------------------------
// CHART COMPONENT: Timeseries Area/Line Chart (Pure SVG)
// -------------------------------------------------------------
function TrendChart({ data = [], metric = 'cost', hoveredPoint, setHoveredPoint }) {
  if (!data.length) {
    return (
      <div className="chart-empty-state">
        <Clock size={16} />
        <span>No timeseries data points in the selected range.</span>
      </div>
    );
  }

  // Dimensions
  const width = 800;
  const height = 220;
  const padLeft = 45;
  const padRight = 20;
  const padTop = 15;
  const padBottom = 25;

  const chartW = width - padLeft - padRight;
  const chartH = height - padTop - padBottom;

  // Extract values based on metric
  const getValue = (d) => {
    if (metric === 'cost') return d.estimated_cost || 0;
    if (metric === 'tokens') return d.total_tokens || 0;
    return d.sessions || 0;
  };

  const formatValue = (v) => {
    if (metric === 'cost') return formatCost(v);
    return formatNum(v);
  };

  const values = data.map(getValue);
  const averageKey = metric === 'cost' ? 'estimated_cost' : metric === 'tokens' ? 'total_tokens' : 'sessions';
  const averages = data.map(d => d.moving_average?.[averageKey] ?? getValue(d));
  const maxVal = Math.max(1, ...values, ...averages);
  const averagePath = averages.map((value, i) => `${i ? 'L' : 'M'} ${padLeft + i / Math.max(1, data.length - 1) * chartW} ${padTop + chartH - value / maxVal * chartH}`).join(' ');

  // Generate SVG points
  const points = data.map((d, i) => {
    const x = padLeft + (i / Math.max(1, data.length - 1)) * chartW;
    const y = padTop + chartH - (getValue(d) / maxVal) * chartH;
    return { x, y, data: d, val: getValue(d) };
  });

  const pathD = points.reduce((acc, p, i) => (i === 0 ? `M ${p.x} ${p.y}` : `${acc} L ${p.x} ${p.y}`), '');
  const areaD = points.length > 0 ? `${pathD} L ${points[points.length - 1].x} ${padTop + chartH} L ${points[0].x} ${padTop + chartH} Z` : '';

  return (
    <div className="svg-chart-container">
      <svg viewBox={`0 0 ${width} ${height}`} className="analytics-svg-chart">
        <defs>
          <linearGradient id="trendGradient" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor="#b7f58e" stopOpacity="0.32" />
            <stop offset="100%" stopColor="#b7f58e" stopOpacity="0.0" />
          </linearGradient>
        </defs>

        {/* Horizontal gridlines */}
        {[0, 0.25, 0.5, 0.75, 1].map((ratio) => {
          const y = padTop + chartH * (1 - ratio);
          const gridVal = maxVal * ratio;
          return (
            <g key={ratio} className="chart-gridline">
              <line x1={padLeft} y1={y} x2={width - padRight} y2={y} />
              <text x={padLeft - 8} y={y + 3} textAnchor="end" className="chart-axis-label">
                {metric === 'cost' ? `$${Math.round(gridVal)}` : formatNum(gridVal)}
              </text>
            </g>
          );
        })}

        {/* Gradient Area */}
        {areaD && <path d={areaD} fill="url(#trendGradient)" />}

        {/* Main Line */}
        {pathD && <path d={pathD} fill="none" stroke="#b7f58e" strokeWidth="2.2" strokeLinecap="round" />}

        <path d={averagePath} fill="none" stroke="#8ba9ef" strokeWidth="2" strokeDasharray="5 4" />

        {/* Date labels on X axis */}
        {points.map((p, i) => {
          if (i === 0 || i === Math.floor(points.length / 2) || i === points.length - 1) {
            return (
              <text key={i} x={p.x} y={height - 6} textAnchor="middle" className="chart-axis-label">
                {p.data.date?.slice(5)}
              </text>
            );
          }
          return null;
        })}

        {/* Interactive points & hover listeners */}
        {points.map((p, i) => (
          <circle
            key={i}
            cx={p.x}
            cy={p.y}
            r={hoveredPoint?.i === i ? 5 : 2.5}
            className={`chart-data-dot ${hoveredPoint?.i === i ? 'hovered' : ''}`}
            onMouseEnter={() => setHoveredPoint({ i, x: p.x, y: p.y, point: p })}
            onMouseLeave={() => setHoveredPoint(null)}
          />
        ))}
      </svg>

      <div className="card-subtext">Daily {metric} · dashed blue: 7-day moving average (available days at range start)</div>

      {/* Floating Hover Tooltip */}
      {hoveredPoint && (
        <div
          className="chart-tooltip"
          style={{
            left: `${(hoveredPoint.x / width) * 100}%`,
            top: `${(hoveredPoint.y / height) * 100}%`
          }}
        >
          <div className="tooltip-date">{hoveredPoint.point.data.date}</div>
          <div className="tooltip-main-val">{formatValue(hoveredPoint.point.val)}</div>
          <div className="tooltip-sub">7-day average: {formatValue(hoveredPoint.point.data.moving_average?.[averageKey] ?? hoveredPoint.point.val)}</div>
          <div className="tooltip-sub">
            {hoveredPoint.point.data.sessions} sessions · {hoveredPoint.point.data.turns} turns
          </div>
        </div>
      )}
    </div>
  );
}
