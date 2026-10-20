export const REPORT_DATE_PRESETS = [
  { value: '', label: 'All time' }, { value: 'day', label: 'Past 24 hours' },
  { value: 'week', label: 'Past 7 days' }, { value: 'month', label: 'Past 30 days' },
  { value: '3months', label: 'Past 90 days' }, { value: 'year', label: 'Past 12 months' },
];
export const reportError = error => {
  const message = typeof error === 'string' ? error : error?.message || 'Could not load the report.';
  return /database (?:table )?is locked/i.test(message)
    ? 'The database is busy. Please try again in a moment.' : message;
};

// Filter changes reset drill-down state (clears stale pinned boundaries).
export function changeReportFilters(patch) {
  return { periodStart: '', until: '', previousSince: '', evidenceKind: '', evidenceValue: '', ...patch };
}
