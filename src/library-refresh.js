/** One refresh in flight, with at most one trailing refresh for later writes. */
export function createLibraryRefresh(refresh) {
  let worker = refresh || (() => {});
  let pending = null, running = false, dirty = false, closed = false;
  return {
    request() {
      if (closed) return Promise.resolve();
      if (pending) { if (running) dirty = true; return pending; }
      pending = Promise.resolve().then(async () => {
        if (closed) return;
        let failure;
        do {
          dirty = false; running = true;
          try { await worker(); failure = null; } catch (error) { failure = error; }
        } while (dirty && !closed);
        if (failure) throw failure;
      }).finally(() => { running = false; pending = null; });
      return pending;
    },
    close() { closed = true; dirty = false; },
    open() { closed = false; },
    setWorker(next) { worker = next; },
  };
}
