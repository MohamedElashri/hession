import { Terminal } from 'lucide-react';
import OptionFilter from './OptionFilter.jsx';

export default function ToolFilter({ tools = [], selected = [], ...props }) {
  const values = [...new Set(tools.map(item => typeof item === 'object' && item !== null ? item.value : item).filter(Boolean))];
  const options = values.map(value => ({ value, label: value }));
  return <OptionFilter {...props} label="Tool" plural="Tools" icon={Terminal} options={options} value={selected} multiple/>;
}
