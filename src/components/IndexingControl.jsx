export default function IndexingControl({ observer, detailed = false }) {
  if (!observer) return null;
  const { available, enabled, status, busy, error, toggle, retry } = observer;
  const label = !available ? 'Available in the desktop app' : error ? 'Indexing control needs attention' : !enabled ? 'Automatic indexing paused'
    : status.phase === 'active' ? 'Automatic local indexing on'
      : status.phase === 'degraded' ? 'Indexing with recovery polling' : `Indexing: ${status.phase}`;
  return <section className="indexing-control" aria-label="Automatic local indexing">
    <p role="status">{label}{status.activeSource && <span> · {status.activeSource}</span>}</p>
    <button type="button" className="btn" disabled={!available || busy} onClick={toggle}>
      {busy ? 'Updating indexing…' : enabled ? 'Pause automatic indexing' : 'Resume automatic indexing'}
    </button>
    {error && <><p role="alert">{error}</p><button type="button" className="btn" disabled={busy} onClick={retry}>Retry indexing control</button></>}
    {detailed && <>
      <p>Reads local histories while Hession is open. SSH refresh and private device sync are separate actions.</p>
      <p>{status.pendingSources || 0} pending sources · Last successful import: {status.lastSuccess ? new Date(status.lastSuccess).toLocaleString() : 'none yet'}</p>
      {status.mayBeStale && enabled && <p>Some results may be stale. Check source availability and recent errors.</p>}
      <details><summary>Source roots and recent errors</summary>
        <ul>{(status.roots || []).map(root => <li key={root}>{root}{status.unavailableRoots?.includes(root) ? ' — not present or unavailable' : ''}</li>)}</ul>
        {(status.errors || []).map((message, index) => <p key={index}>{message}</p>)}
      </details>
    </>}
  </section>;
}
