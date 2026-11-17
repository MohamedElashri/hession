import { canHighlight, syntaxLanguage, syntaxTreeSize } from './syntax.js';

/** One lazy worker, a bounded queue/cache, and an enforceable timeout per block. */
export function createSyntaxClient(startWorker, { timeout = 2500, idle = 5000 } = {}) {
  let worker = null, active = null, nextId = 0, timer = null, idleTimer = null;
  let cachedCharacters = 0, cachedNodes = 0;
  const queue = [], cache = new Map();
  const stop = () => { worker?.terminate(); worker = null; };
  const remember = (key, text, nodes, count) => {
    if (!nodes) return;
    const previous = cache.get(key);
    if (previous) { cachedCharacters -= previous.characters; cachedNodes -= previous.count; cache.delete(key); }
    cache.set(key, { nodes, characters: text.length, count }); cachedCharacters += text.length; cachedNodes += count;
    while (cache.size > 8 || cachedCharacters > 128000 || cachedNodes > 10000) {
      const oldest = cache.keys().next().value, value = cache.get(oldest);
      cache.delete(oldest); cachedCharacters -= value.characters; cachedNodes -= value.count;
    }
  };
  const finish = (job, nodes, broken = false) => {
    if (active !== job) return;
    clearTimeout(timer); active = null;
    if (broken) stop();
    const count = syntaxTreeSize(nodes, job.text);
    if (count) remember(job.key, job.text, nodes, count);
    if (!job.cancelled) job.callback(count ? nodes : null);
    pump();
  };
  const pump = async () => {
    clearTimeout(idleTimer);
    if (active) return;
    if (!queue.length) { idleTimer = setTimeout(stop, idle); return; }
    const job = queue.shift(); active = job;
    timer = setTimeout(() => finish(job, null, true), timeout);
    try {
      const available = worker || await startWorker();
      // A timed-out lazy import may finish after a replacement job has started.
      if (active !== job) { if (available !== worker) available.terminate(); return; }
      worker = available;
      worker.onmessage = ({ data }) => { if (data?.id === job.id) finish(job, data.nodes); };
      worker.onerror = event => { event.preventDefault?.(); finish(job, null, true); };
      worker.onmessageerror = () => finish(job, null, true);
      worker.postMessage({ id: job.id, text: job.text, language: job.language });
    } catch { finish(job, null, true); }
  };
  return {
    request(text, language, callback) {
      let cancelled = false;
      const cancel = () => { cancelled = true; };
      if (!canHighlight(text, language) || queue.length >= 24) { queueMicrotask(() => { if (!cancelled) callback(null); }); return cancel; }
      const key = `${syntaxLanguage(language)}\0${text}`, found = cache.get(key);
      if (found) {
        cache.delete(key); cache.set(key, found);
        queueMicrotask(() => { if (!cancelled) callback(found.nodes); }); return cancel;
      }
      const job = { id: ++nextId, key, text, language: syntaxLanguage(language), callback, cancelled: false };
      queue.push(job); pump();
      return () => { job.cancelled = true; const index = queue.indexOf(job); if (index >= 0) queue.splice(index, 1); };
    },
    dispose() {
      clearTimeout(timer); clearTimeout(idleTimer); stop();
      if (active) active.cancelled = true;
      active = null; queue.length = 0; cache.clear(); cachedCharacters = 0; cachedNodes = 0;
    },
  };
}
