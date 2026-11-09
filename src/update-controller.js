// One store per app, independent of the dialog's lifetime. All network activity
// starts with check() or install(); subscriptions only read local state.
const activePhases = new Set(['checking', 'downloading', 'verifying', 'installing', 'ready', 'restarting']);
const messageOf = error => error instanceof Error ? error.message : String(error);

export class UpdateController {
  constructor(services) {
    this.services = services;
    this.snapshot = { phase: 'idle', downloaded: 0, info: null };
    this.listeners = new Set();
    this.package = null;
    this.metadataRequest = null;
  }

  getSnapshot = () => this.snapshot;
  subscribe = listener => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };
  publish(patch) {
    this.snapshot = { ...this.snapshot, ...patch };
    this.listeners.forEach(listener => listener());
  }

  async loadInfo() {
    if (!this.metadataRequest) {
      this.metadataRequest = this.services.info().then(info => {
        this.publish({ info, metadataError: undefined });
        return info;
      }).catch(error => {
        this.publish({ metadataError: messageOf(error) });
        this.metadataRequest = null;
        return null;
      });
    }
    return this.metadataRequest;
  }

  async check() {
    if (activePhases.has(this.snapshot.phase)) return;
    this.publish({ phase: 'checking', error: undefined, release: undefined, downloaded: 0, total: undefined });
    try {
      const info = await this.loadInfo();
      if (!info) throw new Error('Could not read this installation. Please try again.');
      if (!info.supported) throw new Error(info.reason);
      if (this.package) await this.package.close();
      this.package = null;
      this.package = await this.services.check();
      const release = this.package ? { version: this.package.version, notes: this.package.body, date: this.package.date } : undefined;
      this.publish({ phase: release ? 'available' : 'current', release });
    } catch (error) {
      this.publish({ phase: 'idle', error: messageOf(error) });
    }
  }

  async install() {
    if (this.snapshot.phase !== 'available' || !this.package) return;
    this.publish({ phase: 'downloading', error: undefined, downloaded: 0, total: undefined });
    try {
      await this.package.download(event => {
        if (!['downloading', 'verifying'].includes(this.snapshot.phase)) return;
        if (event.event === 'Started') this.publish({ total: event.data.contentLength });
        if (event.event === 'Progress') this.publish({ downloaded: this.snapshot.downloaded + event.data.chunkLength });
        if (event.event === 'Finished') this.publish({ phase: 'verifying' });
      }, { timeout: 120_000 });
      // download() verifies the signature and signed version before resolving.
      this.publish({ phase: 'installing' });
      await this.package.install();
      this.publish({ phase: 'ready' });
      await this.package.close().catch(() => {});
      this.package = null;
    } catch (error) {
      this.publish({ phase: 'available', error: messageOf(error), downloaded: 0, total: undefined });
    }
  }

  async restart() {
    if (this.snapshot.phase !== 'ready') return;
    this.publish({ phase: 'restarting', error: undefined });
    try { await this.services.restart(); }
    catch (error) { this.publish({ phase: 'ready', error: messageOf(error) }); }
  }
}
