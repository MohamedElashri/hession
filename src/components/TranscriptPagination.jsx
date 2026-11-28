import { useState } from 'react';
import { ChevronFirst, ChevronLast, ChevronLeft, ChevronRight } from 'lucide-react';

export default function TranscriptPagination({ page, totalPages, busy, onPageChange }) {
  const [draft, setDraft] = useState('');
  const target = Number(draft);
  const canJump = !busy && /^[0-9]+$/.test(draft) && target >= 1 && target <= totalPages;
  const controls = [
    ['First', ChevronFirst, 1, page === 1],
    ['Previous', ChevronLeft, page - 1, page === 1],
    ['Next', ChevronRight, page + 1, page === totalPages],
    ['Last', ChevronLast, 'last', false],
  ];
  const navigate = (destination) => {
    setDraft('');
    onPageChange(destination);
  };

  return <nav className="transcript-pagination" aria-label="Transcript pages">
    <div className="transcript-page-buttons">
      {controls.map(([label, Icon, destination, atBoundary]) => (
        <button type="button" className="btn" key={label} aria-label={`${label} transcript page`}
          aria-disabled={busy || atBoundary} onClick={() => { if (!busy && !atBoundary) navigate(destination); }}>
          <Icon size={14}/><span>{label}</span>
        </button>
      ))}
    </div>
    <form className="pagination-jump" onSubmit={event => { event.preventDefault(); if (canJump) navigate(target); }}>
      <span aria-live="polite">Page {page.toLocaleString()} of {totalPages.toLocaleString()}</span>
      <input aria-label="Transcript page number" inputMode="numeric" pattern="[0-9]+" placeholder={String(page)}
        value={draft} readOnly={busy} onChange={event => setDraft(event.target.value)} />
      <button type="submit" className="btn" aria-label="Go to transcript page" aria-disabled={!canJump}>Go</button>
    </form>
  </nav>;
}
