import React from 'react';
import DateFilter from './DateFilter.jsx';
import AgentFilter from './AgentFilter.jsx';
import WorkspaceFilter from './WorkspaceFilter.jsx';
import { REPORT_DATE_PRESETS, changeReportFilters } from '../analytics-data.js';

export default function ReportFilters({ navigation, onChange, agents, projects, metadataState = {} }) {
  const update = patch => onChange(changeReportFilters(patch));
  return <>
    <DateFilter presets={REPORT_DATE_PRESETS} value={navigation.dateFilter === 'custom' ? navigation.customSince : navigation.dateFilter || ''}
      onChange={value => update(REPORT_DATE_PRESETS.some(p => p.value === value) ? { dateFilter: value, customSince: '' } : { dateFilter: 'custom', customSince: value })} />
    <AgentFilter agents={agents} value={navigation.agentFilter || ''} onChange={agentFilter => update({ agentFilter })} loading={metadataState.agents === 'loading'} error={metadataState.agents === 'error'} />
    <WorkspaceFilter projects={projects} value={navigation.workspaceFilter || ''} onChange={workspaceFilter => update({ workspaceFilter })} loading={metadataState.projects === 'loading'} error={metadataState.projects === 'error'} />
  </>;
}
