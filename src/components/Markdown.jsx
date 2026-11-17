import { Children, memo, useMemo, useState, useTransition } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import remarkBreaks from 'remark-breaks';
import { textPreview } from '../text-preview.js';
import HighlightedCode from './HighlightedCode.jsx';

const standardPlugins = [remarkGfm];
const lineBreakPlugins = [remarkGfm, remarkBreaks];
const components = {
  pre({ children }) {
    const code = Children.toArray(children)[0];
    const language = code?.props?.className?.match(/(?:^|\s)language-([^\s]+)/)?.[1];
    return language && typeof code.props.children === 'string'
      ? <pre><HighlightedCode text={code.props.children} language={language}/></pre> : <pre>{children}</pre>;
  },
};

export default memo(function Markdown({ text, preserveLines = false }) {
  const [expanded, setExpanded] = useState(false);
  const [pending, startTransition] = useTransition();
  const preview = useMemo(() => textPreview(text), [text]);
  const long = preview.length < (text?.length || 0);
  const body = <ReactMarkdown components={components} remarkPlugins={preserveLines ? lineBreakPlugins : standardPlugins}>{long && !expanded ? preview : text}</ReactMarkdown>;
  if (!long) return body;
  return <div className={`clamp ${expanded ? 'clamp-open' : ''}`}>
    <div className="clamp-body">{body}</div>
    <button type="button" className="text-button clamp-toggle" disabled={pending} aria-expanded={expanded}
      onClick={() => startTransition(() => setExpanded(value => !value))}>{pending ? 'Opening…' : expanded ? 'Show less' : 'Show more'}</button>
  </div>;
});
