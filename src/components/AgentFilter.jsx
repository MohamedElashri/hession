import { Cpu } from 'lucide-react';
import OptionFilter from './OptionFilter.jsx';

export default function AgentFilter({ agents = [], value = '', ...props }) {
  const options = agents.map(agent => {
    const family = typeof agent === 'string' ? agent : agent.family || agent.value;
    const product = agent.product || family;
    return { value: family, label: product, badge: product !== family ? family : '', title: product !== family ? `${product} (${family})` : family };
  }).filter(option => option.value);
  return <OptionFilter {...props} label="Agent" plural="Agents" icon={Cpu} options={options} value={value} anyLabel="All agents"/>;
}
