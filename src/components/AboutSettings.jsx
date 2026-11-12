import { BRAND } from '../brand.js';
import BrandMark from './BrandMark.jsx';
import SoftwareUpdate from './SoftwareUpdate.jsx';

export default function AboutSettings({ syncRunning }) {
  return <>
    <div className="settings-about-brand">
      <span className="brand-mark"><BrandMark size={24}/></span>
      <div><h3>{BRAND.name}</h3><p>{BRAND.tagline}</p></div>
    </div>
    <SoftwareUpdate syncRunning={syncRunning}/>
  </>;
}
