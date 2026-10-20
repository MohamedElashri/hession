import React from 'react';
import { LoaderCircle } from 'lucide-react';

export default function ReportSyncNotice() {
  return <div className="analytics-loading-placeholder" role="status" aria-live="polite">
    <LoaderCircle size={24} className="spin" />
    <strong>Session sync is in progress</strong>
    <span>Analytics will refresh automatically when it finishes.</span>
  </div>;
}
