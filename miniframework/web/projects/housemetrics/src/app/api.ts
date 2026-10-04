import { Injectable, signal } from '@angular/core';
import { CallOptions, Format, Measured, MfClient, WireStats } from 'miniframework-ng';

function stored(key: string, fallback: string, store: Storage | undefined = globalThis.localStorage): string {
  try {
    return store?.getItem(key) ?? fallback;
  } catch {
    return fallback;
  }
}

function save(key: string, value: string, store: Storage | undefined = globalThis.localStorage): void {
  try {
    if (value) store?.setItem(key, value);
    else store?.removeItem(key);
  } catch {
    // Private mode or blocked storage: the setting lasts for this page only.
  }
}

/**
 * The app's view of the server: which format to ask for, whether to gzip,
 * where the API is, the admin password, and a log of recent calls.
 */
@Injectable({ providedIn: 'root' })
export class Api {
  readonly client = new MfClient(stored('hm.base', ''));
  readonly format = signal<Format>((stored('hm.format', 'json') as Format) || 'json');
  readonly gzip = signal(stored('hm.gzip', '') === '1');
  readonly base = signal(this.client.base);
  /** Admin password, for this tab only. */
  readonly admin = signal(stored('hm.admin', '', globalThis.sessionStorage));
  readonly recent = signal<WireStats[]>([]);

  setFormat(f: Format): void {
    this.format.set(f);
    save('hm.format', f);
  }

  setGzip(on: boolean): void {
    this.gzip.set(on);
    save('hm.gzip', on ? '1' : '');
  }

  setBase(base: string): void {
    this.client.setBase(base.trim());
    this.base.set(this.client.base);
    save('hm.base', this.client.base);
  }

  setAdmin(password: string): void {
    this.admin.set(password);
    save('hm.admin', password, globalThis.sessionStorage);
  }

  private record<T>(m: Measured<T>): T {
    this.recent.update((list) => [m.stats, ...list].slice(0, 30));
    return m.data;
  }

  /** GET in the chosen format. */
  async get<T>(path: string, message: string, extra: Partial<CallOptions> = {}): Promise<T> {
    return this.record(
      await this.client.get<T>(path, {
        format: this.format(),
        gzip: this.gzip(),
        message,
        auth: this.admin() || undefined,
        ...extra,
      }),
    );
  }

  /** GET with full measurements (the format lab). */
  measure<T>(path: string, options: CallOptions): Promise<Measured<T>> {
    return this.client.get<T>(path, options);
  }

  async send<T>(method: string, path: string, body: unknown, message: string): Promise<T> {
    return this.record(
      await this.client.send<T>(method, path, body, {
        format: 'json',
        message,
        auth: this.admin() || undefined,
      }),
    );
  }

  /** Where the board's own pages live (trust page, CA, metrics). */
  url(path: string): string {
    return `${this.base()}${path}`;
  }
}
