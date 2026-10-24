import { GitBranch } from 'lucide-react';
import OptionFilter from './OptionFilter.jsx';

export default function BranchFilter({ branches = [], selected = '', ...props }) {
  const values = [...new Set(branches.map(item => typeof item === 'object' && item !== null ? item.value : item).filter(Boolean))];
  const options = values.map(value => ({ value, label: value }));
  return <OptionFilter {...props} label="Branch" plural="Branches" icon={GitBranch} options={options} value={selected} anyLabel="Any branch"/>;
}
