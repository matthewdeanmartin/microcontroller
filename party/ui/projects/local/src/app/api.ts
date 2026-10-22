import { Injectable, signal } from '@angular/core';
import { ApiError, MfClient } from 'miniframework-ng';
import type { Me, Status, Summary } from 'party-core';

export { ApiError };

/** A readable sentence for any failed call. */
export function problem(e: unknown): string {
  if (e instanceof ApiError) return e.message;
  if (e instanceof TypeError) return 'The board is not responding. Is it on, and are you on the home Wi-Fi?';
  return String(e);
}

/**
 * The board's API plus who is signed in. Cookies carry the session, so
 * nothing secret is kept in the page.
 */
@Injectable({ providedIn: 'root' })
export class Api {
  private readonly client = new MfClient('');
  readonly status = signal<Status | null>(null);
  readonly me = signal<Me | null>(null);
  /** True once status and session have been checked at least once. */
  readonly ready = signal(false);
  /** Unread notifications and events awaiting an answer (polled). */
  readonly summary = signal<Summary | null>(null);
  private lastPoll = 0;

  constructor() {
    // The one background request: at most once a minute, only while the
    // page is visible, and soon after it becomes visible again.
    if (typeof document === 'undefined') return;
    setInterval(() => void this.poll(), 15_000);
    document.addEventListener('visibilitychange', () => void this.poll(true));
  }

  async poll(soon = false): Promise<void> {
    if (!this.me() || document.visibilityState !== 'visible') return;
    if (Date.now() - this.lastPoll < (soon ? 10_000 : 60_000)) return;
    this.lastPoll = Date.now();
    try {
      this.summary.set(await this.get<Summary>('/api/v1/me/summary', 'Summary'));
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) this.me.set(null);
    }
  }

  /** Re-reads the summary now (after reading notifications or answering). */
  async refreshSummary(): Promise<void> {
    this.lastPoll = 0;
    await this.poll();
  }

  async get<T>(path: string, message: string): Promise<T> {
    return (await this.client.get<T>(path, { message })).data;
  }

  async send<T>(method: string, path: string, body: unknown, message: string): Promise<T> {
    return (await this.client.send<T>(method, path, body, { message })).data;
  }

  /** Reloads status and the signed-in person. */
  async refresh(): Promise<void> {
    try {
      this.status.set(await this.get<Status>('/api/v1/status', 'Status'));
      try {
        this.me.set(await this.get<Me>('/api/v1/auth/me', 'Me'));
        void this.refreshSummary();
      } catch (e) {
        if (e instanceof ApiError && e.status === 401) this.me.set(null);
        else throw e;
      }
    } finally {
      this.ready.set(true);
    }
  }

  async setup(display_name: string, email: string, pin: string, setup_code: string): Promise<void> {
    this.me.set(await this.send<Me>('POST', '/api/v1/setup', { display_name, email, pin, setup_code }, 'Me'));
    await this.refresh();
  }

  async signUp(display_name: string, email: string, pin: string, invite = ''): Promise<void> {
    this.me.set(await this.send<Me>('POST', '/api/v1/auth/signup', { display_name, email, pin, setup_code: '', invite }, 'Me'));
  }

  async signIn(login: string, pin: string): Promise<void> {
    this.me.set(await this.send<Me>('POST', '/api/v1/auth/pin', { login, pin }, 'Me'));
  }

  /**
   * "Sign in with Mastodon": the board starts (it may first register with
   * the instance, a few seconds), then the browser goes there. The instance
   * comes back to /auth/callback, which lands on /signin/finishing.
   * `purpose`: signin, link (signed in) or redeem (with an invitation token).
   */
  async startMastodon(provider: number, purpose: 'signin' | 'link' | 'redeem', token = ''): Promise<void> {
    const { ticket } = await this.send<{ ticket: string }>('POST', '/api/v1/auth/oauth/start', { provider, purpose, token }, 'OAuthTicket');
    for (let i = 0; i < 40; i++) {
      const s = await this.get<{ state: string; location: string; message: string }>(`/api/v1/auth/oauth/${ticket}`, 'OAuthStatus');
      if (s.state === 'redirect') {
        globalThis.location.assign(s.location);
        return;
      }
      if (s.state === 'failed') throw new ApiError(502, 'oauth', s.message);
      await new Promise((r) => setTimeout(r, 500));
    }
    throw new ApiError(504, 'oauth', 'The instance is taking too long to answer; try again');
  }

  async signOut(everywhere = false): Promise<void> {
    await this.send('POST', `/api/v1/auth/logout${everywhere ? '?all=1' : ''}`, undefined, 'Done');
    this.me.set(null);
  }
}
