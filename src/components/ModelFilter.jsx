import { Sparkles } from 'lucide-react';
import OptionFilter from './OptionFilter.jsx';

export default function ModelFilter({ models = [], selected = [], ...props }) {
  const byModel = new Map();
  for (const model of models) {
    if (!model.value) continue;
    if (!byModel.has(model.value)) byModel.set(model.value, new Set());
    if (model.provider) byModel.get(model.value).add(model.provider);
  }
  const options = Array.from(byModel, ([value, providers]) => ({ value, label: value, badge: [...providers].join(', ') }));
  return <OptionFilter {...props} label="Model" plural="Models" icon={Sparkles} options={options} value={selected} multiple/>;
}
