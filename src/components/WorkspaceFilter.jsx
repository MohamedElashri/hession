import { Layers } from 'lucide-react';
import { shortPath } from '../labels.js';
import OptionFilter from './OptionFilter.jsx';

export default function WorkspaceFilter({ projects = [], value = '', ...props }) {
  const options = projects.filter(project => project.path).map(project => ({
    value: project.path,
    label: project.display_name || shortPath(project.path),
    badge: project.display_name !== project.path ? shortPath(project.path) : '',
    title: project.path,
  }));
  return <OptionFilter {...props} label="Workspace" plural="Workspaces" icon={Layers} options={options} value={value} anyLabel="All workspaces"/>;
}
