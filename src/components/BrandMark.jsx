// Single source: the mark's geometry lives in branding/mark.svg (currentColor).
import mark from '../../branding/mark.svg?raw';

export default function BrandMark({ size = 17 }) {
  const svg = mark.replace(/<!--[\s\S]*?-->/g, '').replace(/width="24" height="24"/, `width="${size}" height="${size}"`);
  return <span className="brand-glyph" aria-hidden="true" dangerouslySetInnerHTML={{ __html: svg }}/>;
}
