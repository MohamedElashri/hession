// Stored timestamps without an offset (including SQLite timestamps) are UTC.
export function parseDate(value) {
  if (value == null || value === '') return null;
  if (value instanceof Date || typeof value === 'number') {
    const date = new Date(value);
    return Number.isNaN(date.getTime()) ? null : date;
  }
  let text = String(value).trim().replace(' ', 'T');
  if (!/(Z|[+-]\d{2}:?\d{2})$/i.test(text)) text += 'Z';
  const date = new Date(text);
  return Number.isNaN(date.getTime()) ? null : date;
}

export function localDateKey(date = new Date()) {
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, '0')}-${String(date.getDate()).padStart(2, '0')}`;
}

export function filterSince(value) {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(value)) return value;
  const [year, month, day] = value.split('-').map(Number);
  return new Date(year, month - 1, day).toISOString();
}

export function calendarDayDifference(date, now = new Date()) {
  const day = d => Date.UTC(d.getFullYear(), d.getMonth(), d.getDate());
  return Math.round((day(now) - day(date)) / 86400000);
}
