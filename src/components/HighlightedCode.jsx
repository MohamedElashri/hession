import { memo, useEffect, useRef, useState } from 'react';
import { createSyntaxClient } from '../syntax-client.js';
import { canHighlight, syntaxLanguage } from '../syntax.js';
import '../styles/syntax.css';

const syntax = createSyntaxClient(async () => {
  const { default: SyntaxWorker } = await import('../syntax-worker.js?worker&inline');
  return new SyntaxWorker();
});
const spans = nodes => nodes.map((node, index) => typeof node === 'string' ? node
  : <span className={node.className} key={index}>{spans(node.children)}</span>);

export default memo(function HighlightedCode({ text, language = 'plaintext' }) {
  const [result, setResult] = useState(null);
  const element = useRef(null);
  const name = syntaxLanguage(language);
  useEffect(() => {
    if (!canHighlight(text, name) || typeof Worker === 'undefined') return;
    let observer, cancel, started = false, disposed = false;
    const start = () => {
      if (started || disposed) return;
      started = true; observer?.disconnect();
      cancel = syntax.request(text, name, nodes => { if (!disposed) setResult({ text, language: name, nodes }); });
    };
    if (typeof IntersectionObserver === 'function') {
      observer = new IntersectionObserver(entries => { if (entries.some(entry => entry.isIntersecting)) start(); }, { rootMargin: '300px' });
      observer.observe(element.current);
    } else start();
    return () => { disposed = true; observer?.disconnect(); cancel?.(); };
  }, [text, name]);
  const nodes = result?.text === text && result.language === name ? result.nodes : null;
  return <code ref={element} className="syntax-code" data-language={name} data-highlighted={!!nodes}>{nodes ? spans(nodes) : text}</code>;
});
