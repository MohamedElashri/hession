import React, { useMemo, useState } from 'react';
import { ArrowUp, ArrowDown, ArrowUpDown } from 'lucide-react';

export const numericSortValue = (row, key) => {
  if (key === 'typical_cost') {
    const input = numericSortValue(row, 'inPer1M');
    const output = numericSortValue(row, 'outPer1M');
    return input == null || output == null ? null : (80000 * input + 20000 * output) / 1000000;
  }
  const value = key.split('.').reduce((current, part) => current?.[part], row);
  return value == null || value === '' || !Number.isFinite(Number(value)) ? null : Number(value);
};

export function useNumericSort(rows) {
  const [sort, setSort] = useState(null);
  const sortedRows = useMemo(() => {
    if (!sort) return rows;
    return [...rows].sort((a, b) => {
      const left = numericSortValue(a, sort.key);
      const right = numericSortValue(b, sort.key);
      // Unavailable values stay at the bottom in either direction.
      if (left == null) return right == null ? 0 : 1;
      if (right == null) return -1;
      return (left - right) * (sort.direction === 'ascending' ? 1 : -1);
    });
  }, [rows, sort]);
  const toggleSort = key => setSort(current => ({ key,
    direction: current?.key === key && current.direction === 'ascending' ? 'descending' : 'ascending' }));
  return { rows: sortedRows, sort, toggleSort };
}

export function NumericSortHeader({ column, children, sorting }) {
  const direction = sorting.sort?.key === column ? sorting.sort.direction : 'none';
  const Icon = direction === 'ascending' ? ArrowUp : direction === 'descending' ? ArrowDown : ArrowUpDown;
  return <th className="num-col" aria-sort={direction}>
    <button type="button" className={`analytics-sort-header ${direction !== 'none' ? 'active' : ''}`}
      onClick={() => sorting.toggleSort(column)}
      title={`Sort ${children} ${direction === 'ascending' ? 'descending' : 'ascending'}`}>
      <span>{children}</span><Icon size={12} aria-hidden="true" />
    </button>
  </th>;
}
