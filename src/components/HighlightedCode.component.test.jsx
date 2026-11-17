import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import { highlightSyntax } from '../syntax-engine.js';
import { TextPanel } from './ToolActivity.jsx';
import HighlightedCode from './HighlightedCode.jsx';
import Markdown from './Markdown.jsx';

const service = vi.hoisted(() => ({ requests: [] }));
vi.mock('../syntax-client.js', () => ({ createSyntaxClient: () => ({ request(text, language, callback) {
  const request = { text, language, callback, cancelled: false }; service.requests.push(request);
  return () => { request.cancelled = true; };
} }) }));
beforeEach(() => { service.requests.length = 0; vi.stubGlobal('Worker', class {}); });
afterEach(() => vi.unstubAllGlobals());
const finish = (request = service.requests.at(-1)) => act(() => request.callback(highlightSyntax(request.text, request.language)));

test('highlighting uses token spans while keeping executable-looking markup literal', () => {
  const code = '<img src=x onerror="globalThis.injected=true">\n<script>alert(1)</script>';
  const { container } = render(<HighlightedCode text={code} language="html"/>); finish();
  expect(container.querySelector('code').textContent).toBe(code);
  expect(container.querySelector('.hljs-name')).toBeTruthy();
  expect(container.querySelector('img,script')).toBeNull();
  expect(globalThis.injected).toBeUndefined();
});
test('out-of-order highlighting cannot replace changed live content or a selected plain-text view', () => {
  const { container, rerender } = render(<HighlightedCode text="const oldValue = 1;" language="js"/>);
  const old = service.requests[0]; rerender(<HighlightedCode text="const newValue = 2;" language="js"/>);
  expect(old.cancelled).toBe(true); finish(old);
  expect(container.querySelector('code').textContent).toBe('const newValue = 2;');
  expect(container.querySelector('.hljs-keyword')).toBeNull(); finish();
  expect(container.querySelector('.hljs-keyword')).toBeTruthy();
  rerender(<HighlightedCode text="const newValue = 2;" language="plaintext"/>);
  expect(container.querySelector('.hljs-keyword')).toBeNull();
});
test('highlighted commands retain exact clipboard contents and wrapping with an explicit plain-text override', async () => {
  const writeText = vi.fn().mockResolvedValue(); Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } });
  const command = "printf '%s\\n' \"$HOME\"\r\n# 🦀 café";
  render(<TextPanel label="Command" text={command} language="bash"/>); finish();
  expect(screen.getByLabelText('Command text').querySelector('[data-highlighted="true"]')).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'Copy command' })); expect(writeText).toHaveBeenCalledWith(command);
  fireEvent.click(screen.getByRole('button', { name: 'Wrap command' }));
  expect(screen.getByLabelText('Command text').textContent).toBe(command);
  expect(screen.getByLabelText('Command text').className).toContain('is-unwrapped');
  fireEvent.change(screen.getByRole('combobox', { name: 'Syntax for command' }), { target: { value: 'plaintext' } });
  expect(screen.getByLabelText('Command text').querySelector('.hljs-string')).toBeNull();
  expect(screen.getByLabelText('Command text').textContent).toBe(command);
});
test('large expanded blocks switch to plain text without truncating their content', async () => {
  const text = 'print("🦀")\n'.repeat(3000);
  render(<TextPanel label="Code" text={text} language="python"/>); finish();
  expect(screen.getByLabelText('Code text').querySelector('[data-highlighted="true"]')).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: /Show all/ }));
  await vi.waitFor(() => expect(screen.getByLabelText('Code text').textContent).toBe(text));
  expect(screen.getByText('Large block · plain text')).toBeTruthy();
  expect(screen.getByLabelText('Code text').querySelector('[data-highlighted="true"]')).toBeNull();
});
test('language selection colors plain logs only on request and persists across later output', () => {
  const { rerender } = render(<TextPanel label="Output" text="INFO something happened"/>);
  expect(service.requests).toHaveLength(0);
  fireEvent.change(screen.getByRole('combobox', { name: 'Syntax for output' }), { target: { value: 'python' } });
  rerender(<TextPanel label="Output" text="print('later output')"/>); finish();
  expect(screen.getByRole('combobox', { name: 'Syntax for output' }).value).toBe('python');
  expect(screen.getByLabelText('Output text').querySelector('.hljs-string')).toBeTruthy();
});
test('fenced Markdown code uses its language while inline and unknown code remain literal', () => {
  const { container } = render(<Markdown text={'Use `x < y`.\n\n```python\ndef greet():\n    return "hi"\n```\n\n```unknown\n<script>literal</script>\n```'}/>); finish();
  const codes = container.querySelectorAll('pre code');
  expect(codes[0].dataset.language).toBe('python'); expect(codes[0].querySelector('.hljs-keyword')).toBeTruthy();
  expect(codes[0].textContent).toBe('def greet():\n    return "hi"\n');
  expect(codes[1].dataset.language).toBe('plaintext'); expect(codes[1].textContent).toBe('<script>literal</script>\n');
  expect(container.querySelector('script')).toBeNull(); expect(service.requests).toHaveLength(1);
});
test('a worker failure leaves every source character readable', () => {
  const code = 'const retained = "source";';
  const { container } = render(<HighlightedCode text={code} language="js"/>);
  act(() => service.requests[0].callback(null));
  expect(container.querySelector('code').textContent).toBe(code);
  expect(container.querySelector('code').dataset.highlighted).toBe('false');
});
test('offscreen blocks wait until visible and disconnect on unmount', () => {
  const observers = [];
  vi.stubGlobal('IntersectionObserver', class {
    constructor(callback) { this.callback = callback; this.disconnect = vi.fn(); observers.push(this); }
    observe() {}
  });
  const { unmount } = render(<HighlightedCode text="const visible = true;" language="js"/>);
  expect(service.requests).toHaveLength(0);
  act(() => observers[0].callback([{ isIntersecting: false }])); expect(service.requests).toHaveLength(0);
  act(() => observers[0].callback([{ isIntersecting: true }])); expect(service.requests).toHaveLength(1);
  act(() => observers[0].callback([{ isIntersecting: true }])); expect(service.requests).toHaveLength(1);
  unmount(); expect(observers[0].disconnect).toHaveBeenCalled(); expect(service.requests[0].cancelled).toBe(true);
});
