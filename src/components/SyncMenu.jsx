import { useRef, useState } from 'react';

const modes = [
  { value: 'incremental', label: 'Incremental' },
  { value: 'full', label: 'Full rescan' },
];

export default function SyncMenu({ agents, disabled, autoSync, toggleAutoSync, startSync, agentStatus, checkpointStatus, agentDescription, onClose }) {
  const [mode, setMode] = useState('incremental');
  const tabs = useRef([]);

  const handleTabKey = (event, index) => {
    let next;
    if (event.key === 'ArrowRight') next = (index + 1) % modes.length;
    else if (event.key === 'ArrowLeft') next = (index + modes.length - 1) % modes.length;
    else if (event.key === 'Home') next = 0;
    else if (event.key === 'End') next = modes.length - 1;
    else return;
    event.preventDefault();
    setMode(modes[next].value);
    tabs.current[next]?.focus();
  };

  return (
    <div className="sync-menu" role="dialog" aria-label="Sync options" onKeyDown={event => {
      if (event.key === 'Escape') {
        event.stopPropagation();
        const trigger = event.currentTarget.parentElement.querySelector('[aria-haspopup="dialog"]');
        onClose();
        trigger?.focus();
      }
    }}>
      <button type="button" role="switch" aria-checked={autoSync} className="sync-auto-toggle" onClick={toggleAutoSync}>
        <span>Sync on startup</span>
        <span className={`sync-auto-switch ${autoSync ? 'enabled' : ''}`} aria-hidden="true" />
      </button>
      <div className="sync-menu-divider" />
      <div className="sync-tabs" role="tablist" aria-label="Sync mode">
        {modes.map((tab, index) => (
          <button type="button" role="tab" id={`sync-tab-${tab.value}`} key={tab.value}
            aria-selected={mode === tab.value} aria-controls={`sync-panel-${tab.value}`}
            tabIndex={mode === tab.value ? 0 : -1} ref={element => { tabs.current[index] = element; }}
            onClick={() => setMode(tab.value)} onKeyDown={event => handleTabKey(event, index)}>
            {tab.label}
          </button>
        ))}
      </div>
      {modes.map(tab => (
        <div className="sync-tab-panel" role="tabpanel" id={`sync-panel-${tab.value}`}
          aria-labelledby={`sync-tab-${tab.value}`} key={tab.value} hidden={mode !== tab.value}>
          {agentStatus}
          {tab.value === 'incremental' && checkpointStatus}
          <button type="button" className="sync-all-agents" disabled={disabled} onClick={() => startSync({ mode: tab.value })}>
            All agents
            <small>{tab.value === 'full' ? 'Scan every session source again' : 'Continue since each agent’s last sync'}</small>
          </button>
          {agents.map(agent => (
            <button type="button" disabled={disabled} key={agent.family} onClick={() => startSync({ mode: tab.value, agent: agent.family })}>
              {agent.product}
              <small>{tab.value === 'full' ? 'Rescan all of this agent’s sessions' : agentDescription(agent.family)}</small>
            </button>
          ))}
        </div>
      ))}
    </div>
  );
}
