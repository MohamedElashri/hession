import { highlightSyntax } from './syntax-engine.js';

self.onmessage = ({ data }) => {
  if (typeof data?.text !== 'string' || !Number.isSafeInteger(data.id)) return;
  self.postMessage({ id: data.id, nodes: highlightSyntax(data.text, data.language) });
};
