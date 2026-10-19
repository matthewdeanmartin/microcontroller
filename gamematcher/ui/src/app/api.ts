import { Injectable, signal } from '@angular/core';
import { ApiError, MfClient } from 'miniframework-ng';

// Shapes of the board's messages (api/src/messages.rs). The client decodes
// any wire format with the server's schema; the UI asks for JSON.
export interface Status {
  app: string;
  version: string;
  build: string;
  setup_needed: boolean;
  signup_open: boolean;
  clock_synced: boolean;
  setup_code_required: boolean;
  secure: boolean;
  providers: ProviderButton[];
  /** The household Mastodon bot, "" when there is none. */
  bot: string;
  /** The board can send email. */
  mail: boolean;
  public_bot: string;
}

export interface ProviderButton {
  id: number;
  name: string;
}

export interface Me {
  id: number;
  email: string;
  display_name: string;
  role: 'admin' | 'member';
  has_pin: boolean;
  sessions: number;
  created: number;
  /** Linked Mastodon account, `@user@host`, or ''. */
  mastodon: string;
  /** Plan news by email. */
  email_notify: boolean;
}

export interface UserSummary {
  id: number;
  display_name: string;
  role: string;
}

export interface AdminUser {
  id: number;
  email: string;
  display_name: string;
  role: 'admin' | 'member';
  disabled: boolean;
  has_pin: boolean;
  sessions: number;
  created: number;
  last_seen: number;
  locked_secs: number;
  mastodon: string;
}

export interface StorageInfo {
  generation: number;
  snapshot_bytes: number;
  commits: number;
  accounts: number;
  max_accounts: number;
  sessions: number;
  max_sessions: number;
  loaded_slot: number;
  fallback_used: boolean;
}

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
  /** Unread notifications and plans awaiting an answer (polled). */
  readonly summary = signal<Summary | null>(null);
  /** Two polls in a row failed: the UI says the board isn't answering. */
  readonly offline = signal(false);
  private failures = 0;
  private lastPoll = 0;

  constructor() {
    // The one background request: at most once a minute, only while the
    // page is visible, and right away when it becomes visible again.
    if (typeof document === 'undefined') return;
    setInterval(() => void this.poll(), 15_000);
    document.addEventListener('visibilitychange', () => void this.poll(true));
    window.addEventListener('focus', () => void this.poll(true));
  }

  async poll(soon = false): Promise<void> {
    if (!this.me() || document.visibilityState !== 'visible') return;
    const wait = soon ? 10_000 : 60_000;
    if (Date.now() - this.lastPoll < wait) return;
    this.lastPoll = Date.now();
    try {
      this.summary.set(await this.get<Summary>('/api/v1/me/summary', 'Summary'));
      this.failures = 0;
      this.offline.set(false);
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) {
        this.me.set(null);
        return;
      }
      this.failures += 1;
      this.offline.set(this.failures >= 2);
    }
  }

  async get<T>(path: string, message: string): Promise<T> {
    return (await this.client.get<T>(path, { message })).data;
  }

  async send<T>(method: string, path: string, body: unknown, message: string): Promise<T> {
    return (await this.client.send<T>(method, path, body, { message })).data;
  }

  /** Reloads status and the signed-in user. */
  async refresh(): Promise<void> {
    try {
      this.status.set(await this.get<Status>('/api/v1/status', 'Status'));
      try {
        this.me.set(await this.get<Me>('/api/v1/me', 'Me'));
        this.lastPoll = 0;
        void this.poll();
      } catch (e) {
        if (e instanceof ApiError && e.status === 401) this.me.set(null);
        else throw e;
      }
    } finally {
      this.ready.set(true);
    }
  }

  async setup(email: string, display_name: string, pin: string, setup_code: string): Promise<void> {
    this.me.set(await this.send<Me>('POST', '/api/v1/setup', { email, display_name, pin, setup_code }, 'Me'));
    await this.refresh();
  }

  async signUp(email: string, display_name: string, pin: string): Promise<void> {
    this.me.set(await this.send<Me>('POST', '/api/v1/auth/signup', { email, display_name, pin, setup_code: '' }, 'Me'));
  }

  async signIn(email: string, pin: string): Promise<void> {
    this.me.set(await this.send<Me>('POST', '/api/v1/auth/pin', { email, pin }, 'Me'));
  }

  /**
   * "Sign in with Mastodon": asks the board to start (it may first register
   * with the instance, a few seconds), then sends the browser there. The
   * instance comes back to /auth/callback, which lands on /signin/finishing.
   */
  async startMastodon(provider: number, link = false): Promise<void> {
    const { ticket } = await this.send<{ ticket: string }>('POST', '/api/v1/auth/oauth/start', { provider, link }, 'OAuthTicket');
    for (let i = 0; i < 40; i++) {
      const s = await this.get<OAuthStatus>(`/api/v1/auth/oauth/${ticket}`, 'OAuthStatus');
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

// ---- Games and plans (api/src/messages.rs) ----------------------------------

export type Kind = 'board' | 'card' | 'video' | 'rpg' | 'other';
export const KINDS: Kind[] = ['board', 'card', 'video', 'rpg', 'other'];

export interface GameView {
  id: number;
  title: string;
  kind: Kind;
  min_players: number;
  max_players: number;
  minutes: number;
  notes: string;
  added_by: number;
  added_by_name: string;
  archived: boolean;
  interested: number;
  mine: boolean;
  editable: boolean;
}

export interface GameDetail {
  game: GameView;
  interested: UserSummary[];
  plans: PlanSummary[];
}

export interface GameInput {
  title?: string;
  kind?: Kind;
  min_players?: number;
  max_players?: number;
  minutes?: number;
  notes?: string;
  archived?: boolean;
}

export type Overall = 'in' | 'maybe' | 'out';
export type Answer = 'yes' | 'if_needed' | 'no';
export type PlanStatus = 'open' | 'on' | 'done' | 'cancelled';

export interface TimeAnswer {
  time: number;
  answer: Answer;
}

export interface PlanTime {
  id: number;
  start: number;
  end: number;
  yes: number[];
  if_needed: number[];
  no: number[];
}

export interface PlanGame {
  id: number;
  title: string;
  kind: string;
  min_players: number;
  max_players: number;
  minutes: number;
  archived: boolean;
  ticks: number[];
}

export interface ResponseView {
  account: number;
  name: string;
  overall: Overall;
  times: TimeAnswer[];
  games: number[];
  note: string;
  updated: number;
}

export interface PlanView {
  id: number;
  host: number;
  host_name: string;
  title: string;
  notes: string;
  place: string;
  seats: number;
  status: PlanStatus;
  games: PlanGame[];
  times: PlanTime[];
  responses: ResponseView[];
  chosen_time: number | null;
  chosen_games: number[];
  created: number;
  updated: number;
  manageable: boolean;
  announced: boolean;
  announced_public: boolean;
  series: number[];
  report: PostGameReport | null;
  reportable: boolean;
}

export interface PostGameReport {
  summary: string;
  games: { game: number; winners: number[] }[];
}

export interface PlanSummary {
  id: number;
  title: string;
  status: PlanStatus;
  host_name: string;
  games: string[];
  start: number;
  end: number;
  times: number;
  in_count: number;
  maybe_count: number;
  answered: boolean;
  seats: number;
  updated: number;
}

export interface TimeInput {
  id: number;
  start: number;
  end: number;
}

export interface PlanInput {
  title?: string;
  notes?: string;
  place?: string;
  seats?: number;
  games?: number[];
  times?: TimeInput[];
  announce?: boolean;
  reschedule?: TimeInput;
  repeat?: { frequency: string; count: number };
  announce_public?: boolean;
}

// ---- Mastodon bot -----------------------------------------------------------

export interface OutboxEntry {
  id: number;
  kind: string;
  text: string;
  state: 'queued' | 'sent' | 'failed';
  attempts: number;
  error: string;
  created: number;
  url: string;
  next_at: number;
}

export interface MailView {
  configured: boolean;
  sender: string;
  sent: number;
  sent_today: number;
  daily_limit: number;
  last_error: string;
  setup: string;
  built_in: boolean;
}

export interface BotView {
  configured: boolean;
  handle: string;
  instance: string;
  visibility: string;
  digest: boolean;
  digest_time: string;
  posts: number;
  last_post: number;
  last_error: string;
  setup: string;
  jobs: OutboxEntry[];
  replies: boolean;
  last_poll: number;
  reply_error: string;
}

// ---- Comments, notifications, calendar --------------------------------------

export interface CommentView {
  id: number;
  author: number;
  author_name: string;
  text: string;
  created: number;
  edited: number;
  mine: boolean;
  deletable: boolean;
}

export interface CommentList {
  comments: CommentView[];
  more: boolean;
  open: boolean;
  sent_dms: number;
}

export interface NotificationView {
  id: number;
  kind: string;
  plan: number;
  plan_title: string;
  count: number;
  actor_name: string;
  created: number;
  read: boolean;
}

export interface Summary {
  unread: number;
  awaiting: number;
  latest: number;
}

export interface FeedUrl {
  url: string;
  webcal: string;
  enabled: boolean;
}

export interface ProviderView {
  id: number;
  name: string;
  base: string;
  trust: 'household' | 'invited';
  anchor: 'household-ca' | 'public';
  invites: string[];
}

export interface OAuthStatus {
  state: 'waiting' | 'redirect' | 'done' | 'failed';
  location: string;
  message: string;
  result: '' | 'signed_in' | 'linked';
}
