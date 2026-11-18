import { canHighlight, syntaxLanguage, syntaxTreeSize } from './syntax.js';

/** One lazy worker, a bounded queue/cache, and an enforceable timeout per block. */
export function createSyntaxClient(startWorker, { timeout = 2500, idle = 5000, retryDelay = 5000 } = {}) {
  let worker = null, starting = null, active = null, nextId = 0, timer = null, idleTimer = null;
  let cachedCharacters = 0, cachedNodes = 0, retryAfter = 0, generation = 0;
  const queue = [], cache = new Map();
  const stop = () => { worker?.terminate(); worker = null; };
  const remember = (key, text, nodes, count) => {
    const previous = cache.get(key);
    if (previous) { cachedCharacters -= previous.characters; cachedNodes -= previous.count; cache.delete(key); }
    cache.set(key, { nodes, characters: text.length, count }); cachedCharacters += text.length; cachedNodes += count;
    while (cache.size > 8 || cachedCharacters > 128000 || cachedNodes > 10000) {
      const oldest = cache.keys().next().value, value = cache.get(oldest);
      cache.delete(oldest); cachedCharacters -= value.characters; cachedNodes -= value.count;
    }
  };
  const finish = (job, nodes, outcome = 'complete') => {
    if (active !== job) return;
    clearTimeout(timer); active = null;
    if (outcome !== 'complete') stop();
    if (outcome === 'broken') {
      // A blocked import or stuck worker must not charge every queued block
      // another startup/timeout. Keep source visible and retry on later demand.
      retryAfter = performance.now() + retryDelay;
      for (const pending of queue.splice(0)) queueMicrotask(() => { if (!pending.cancelled && pending.generation === generation) pending.callback(null); });
    }
    const count = outcome === 'complete' ? syntaxTreeSize(nodes, job.text) : 0;
    // Cache deterministic plain-text fallbacks too (invalid/over-budget trees).
    if (outcome === 'complete') remember(job.key, job.text, count ? nodes : null, count);
    if (!job.cancelled) job.callback(count ? nodes : null);
    pump();
  };
  const pump = async () => {
    clearTimeout(idleTimer);
    if (active) return;
    if (!queue.length) { idleTimer = setTimeout(stop, idle); return; }
    const job = queue.shift(); active = job;
    const found = cache.get(job.key);
    if (found) {
      cache.delete(job.key); cache.set(job.key, found);
      active = null;
      if (!job.cancelled) job.callback(found.nodes);
      pump(); return;
    }
    timer = setTimeout(() => finish(job, null, 'broken'), timeout);
    try {
      // Navigation can cancel before the lazy import finishes. Share its one
      // worker creation with replacement jobs, instead of spawning one per cancel.
      job.creation = worker ? null : (starting ||= Promise.resolve(startWorker()));
      let available;
      try { available = worker || await job.creation; }
      finally { if (starting === job.creation) starting = null; }
      if (active !== job) {
        if (available !== worker && active?.creation !== job.creation) available.terminate();
        return;
      }
      worker = available;
      worker.onmessage = ({ data }) => { if (data?.id === job.id) finish(job, data.nodes); };
      worker.onerror = event => { event.preventDefault?.(); finish(job, null, 'broken'); };
      worker.onmessageerror = () => finish(job, null, 'broken');
      worker.postMessage({ id: job.id, text: job.text, language: job.language });
    } catch { finish(job, null, 'broken'); }
  };
  return {
    request(text, language, callback) {
      let cancelled = false;
      const requested = generation;
      const cancel = () => { cancelled = true; };
      if (!canHighlight(text, language)) { queueMicrotask(() => { if (!cancelled && requested === generation) callback(null); }); return cancel; }
      const key = `${syntaxLanguage(language)}\0${text}`, found = cache.get(key);
      if (found) {
        cache.delete(key); cache.set(key, found);
        queueMicrotask(() => { if (!cancelled && requested === generation) callback(found.nodes); }); return cancel;
      }
      if (performance.now() < retryAfter || queue.length >= 24) { queueMicrotask(() => { if (!cancelled && requested === generation) callback(null); }); return cancel; }
      const job = { id: ++nextId, key, text, language: syntaxLanguage(language), callback, cancelled: false, generation };
      queue.push(job); pump();
      return () => {
        job.cancelled = true;
        const index = queue.indexOf(job); if (index >= 0) queue.splice(index, 1);
        if (active === job) finish(job, null, 'cancelled');
      };
    },
    dispose() {
      generation++;
      clearTimeout(timer); clearTimeout(idleTimer); stop();
      if (active) active.cancelled = true;
      active = null; starting = null; queue.length = 0; cache.clear(); cachedCharacters = 0; cachedNodes = 0; retryAfter = 0;
    },
  };
}
