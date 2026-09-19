// The NanaCoin API client.
//
// One service, HttpClient, no state beyond the bearer token. Everything the
// UI knows about money it learns from here; nothing here decides whether a
// transaction is valid.

import { HttpClient, HttpErrorResponse, HttpHeaders } from '@angular/common/http';
import { Injectable, inject } from '@angular/core';
import { Observable, catchError, firstValueFrom, throwError } from 'rxjs';

import { Accounts } from './accounts';
import { Log } from './log';
import { digestSha256, hasNativeDigest } from './sha256';
import { ApiBase } from './api-base';
import {
  AccountHistory,
  AccountId,
  ApiErrorBody,
  Config,
  LedgerPage,
  Listing,
  LogPage,
  ListingId,
  ListingSide,
  ListingStatus,
  Offer,
  OfferId,
  OfferResult,
  PurchaseResult,
  Status,
  TokenResponse,
  Transaction,
  TransactionId,
  User,
  UserId,
  UserStatus,
} from './models';

/** Carries the server's machine-readable code alongside its message. */
export class ApiError extends Error {
  constructor(
    readonly status: number,
    readonly code: string,
    message: string,
  ) {
    super(message);
    this.name = 'ApiError';
  }

  /**
   * True when the failure is NanaCoin being unreachable rather than NanaCoin
   * refusing the request. Drives the connect screen.
   */
  get isNetwork(): boolean {
    return this.code === 'unreachable';
  }
}

@Injectable({ providedIn: 'root' })
export class NanacoinService {
  private readonly http = inject(HttpClient);
  private readonly apiBase = inject(ApiBase);

  /** Read per request, so changing the server takes effect without a reload. */
  private get base(): string {
    return this.apiBase.current();
  }

  private readonly accounts = inject(Accounts);
  private readonly log = inject(Log);

  /**
   * The bearer token of whichever account is active.
   *
   * Read through Accounts rather than held here, so that switching accounts is
   * a pointer move in one place and every request afterwards is automatically
   * made as the new person. A stale copy in this service was the obvious bug
   * to avoid: it would send the previous account's token for exactly one
   * request after a switch.
   */
  private get token(): string | null {
    return this.accounts.token();
  }

  get authenticated(): boolean {
    return this.token !== null;
  }

  // --- auth ---

  status(): Promise<Status> {
    return this.get<Status>('/status');
  }

  provision(
    username: string,
    displayName: string,
    password: string,
    householdName: string,
  ): Promise<User> {
    return this.post<User>('/provision', {
      username,
      display_name: displayName,
      password,
      household_name: householdName,
    });
  }

  /**
   * Runs the Authorization Code + PKCE exchange.
   *
   * The verifier never leaves this function except in the token request, and
   * the challenge is S256 - the server refuses `plain` outright.
   */
  async login(username: string, password: string): Promise<User> {
    this.log.info('auth', 'logging in', {
      username,
      digest: hasNativeDigest() ? 'crypto.subtle' : 'fallback (insecure origin)',
      secureContext: window.isSecureContext,
    });

    let verifier: string;
    let challenge: string;
    try {
      verifier = newVerifier();
      challenge = await challengeFor(verifier);
    } catch (e) {
      // Everything here runs before the first request, so a failure produces
      // no network activity at all - which is exactly how this looked when
      // crypto.subtle was missing: a generic error and an empty network panel.
      this.log.error('auth', 'could not prepare the login challenge', {
        error: String(e),
        secureContext: window.isSecureContext,
      });
      throw new ApiError(0, 'crypto_unavailable',
        'This browser would not let the page prepare a secure login on an ' +
        'insecure connection. Reload the page and try again.');
    }
    const redirectUri = location.origin + location.pathname;

    const authz = await this.post<{ code: string }>('/auth/authorize', {
      username,
      password,
      code_challenge: challenge,
      code_challenge_method: 'S256',
      redirect_uri: redirectUri,
    });

    const tok = await this.post<TokenResponse>('/auth/token', {
      code: authz.code,
      code_verifier: verifier,
      redirect_uri: redirectUri,
    });

    this.log.info('auth', 'logged in', {
      username: tok.user.username,
      role: tok.user.role,
      accountsHeld: this.accounts.all().length + 1,
    });
    this.accounts.add({
      userId: tok.user.id,
      username: tok.user.username,
      displayName: tok.user.display_name,
      role: tok.user.role,
      token: tok.access_token,
    });
    return tok.user;
  }

  /**
   * Ends the active session and forgets it, leaving any others intact.
   *
   * Telling the server is a courtesy - it revokes the token now rather than at
   * expiry - but a failed request must not leave someone still signed in on a
   * shared computer, which is the whole reason they pressed the button.
   */
  async logout(): Promise<void> {
    try {
      await this.post<void>('/auth/logout', {});
    } finally {
      this.accounts.invalidateActive();
    }
  }

  /**
   * Ends every session this browser holds.
   *
   * Each token is revoked separately because the server has no "log out
   * everywhere" endpoint - they were issued independently and are independent.
   * One failing revocation must not strand the rest, so failures are ignored
   * and the local store is cleared regardless.
   */
  async logoutAll(): Promise<void> {
    const held = this.accounts.all();
    const active = this.accounts.active();

    for (const account of held) {
      this.accounts.activate(account.userId);
      try {
        await this.post<void>('/auth/logout', {});
      } catch {
        // An already-dead session is the common case here, and is fine.
      }
    }
    if (active) this.accounts.activate(active.userId);
    this.accounts.clear();
  }

  me(): Promise<User> {
    return this.get<User>('/me');
  }

  /**
   * Distinguishes "nothing is there" from "it is there but the browser is not
   * allowed to read it".
   *
   * A CORS-blocked response and an unreachable host look identical to
   * JavaScript - both surface as status 0 with no detail, deliberately, so
   * that a page cannot use fetch to probe a private network. But a `no-cors`
   * request is dispatched anyway and its promise resolves (with an opaque
   * response) when the server answered, and rejects when nothing did. That is
   * enough to tell the two apart and say something useful.
   *
   * Only used after a failure, to explain it.
   */
  async probe(): Promise<'reachable' | 'absent'> {
    try {
      await fetch(`${this.base}/status`, { mode: 'no-cors', cache: 'no-store' });
      return 'reachable';
    } catch {
      return 'absent';
    }
  }

  // --- users ---

  users(): Promise<{ users: User[] }> {
    return this.get<{ users: User[] }>('/users');
  }

  createUser(
    username: string,
    displayName: string,
    password: string,
    grant: boolean,
  ): Promise<User> {
    return this.post<User>('/users', {
      username,
      display_name: displayName,
      password,
      grant,
    });
  }

  setUserStatus(id: UserId, status: UserStatus): Promise<User> {
    return this.patch<User>(`/users/${encodeURIComponent(id)}`, { status });
  }

  // --- money ---

  accountHistory(id: AccountId, limit = 50): Promise<AccountHistory> {
    return this.get<AccountHistory>(
      `/accounts/${encodeURIComponent(id)}/transactions?limit=${limit}`,
    );
  }

  ledger(limit = 100): Promise<LedgerPage> {
    return this.get<LedgerPage>(`/transactions?limit=${limit}`);
  }

  transaction(id: TransactionId): Promise<Transaction> {
    return this.get<Transaction>(`/transactions/${encodeURIComponent(id)}`);
  }

  /**
   * Sends coins. The idempotency key is the caller's to generate, once per
   * attempted operation, so that a retry after a dropped connection is
   * recognisably the same transfer rather than a second one.
   */
  transfer(
    to: AccountId,
    amount: number,
    memo: string,
    idempotencyKey: string,
  ): Promise<Transaction> {
    return this.post<Transaction>('/transfers', { to, amount, memo }, idempotencyKey);
  }

  issue(
    to: AccountId,
    amount: number,
    reason: string,
    idempotencyKey: string,
  ): Promise<Transaction> {
    return this.post<Transaction>('/admin/issue', { to, amount, reason }, idempotencyKey);
  }

  retire(
    from: AccountId,
    amount: number,
    reason: string,
    idempotencyKey: string,
  ): Promise<Transaction> {
    return this.post<Transaction>('/admin/retire', { from, amount, reason }, idempotencyKey);
  }

  reverse(id: TransactionId, reason: string, idempotencyKey: string): Promise<Transaction> {
    return this.post<Transaction>(
      `/transactions/${encodeURIComponent(id)}/reverse`,
      { reason },
      idempotencyKey,
    );
  }

  // --- marketplace ---

  listings(status?: ListingStatus): Promise<{ listings: Listing[] }> {
    const q = status ? `?status=${encodeURIComponent(status)}` : '';
    return this.get<{ listings: Listing[] }>(`/listings${q}`);
  }

  createListing(input: {
    title: string;
    description: string;
    price: number;
    kind?: string;
    currency?: string;
    minor_units?: number;
    /** 'BUY' posts a want-ad. Omitted means SELL, which is what older
     *  servers assume. */
    side?: ListingSide;
  }): Promise<Listing> {
    return this.post<Listing>('/listings', input);
  }

  updateListing(
    id: ListingId,
    changes: { title?: string; description?: string; price?: number },
  ): Promise<Listing> {
    return this.patch<Listing>(`/listings/${encodeURIComponent(id)}`, changes);
  }

  purchase(id: ListingId, idempotencyKey: string): Promise<PurchaseResult> {
    return this.post<PurchaseResult>(
      `/listings/${encodeURIComponent(id)}/purchase`,
      {},
      idempotencyKey,
    );
  }

  cancelListing(id: ListingId): Promise<Listing> {
    return this.post<Listing>(`/listings/${encodeURIComponent(id)}/cancel`, {});
  }

  // --- offers ---
  //
  // These call endpoints the board does not serve yet. The UI is being built
  // first deliberately: the server is memory-constrained and changing it is
  // expensive, so the shape is worth settling here - where a mistake costs a
  // rebuild rather than a reflash - before any bytes are committed to it.
  //
  // Until the board catches up every one of these 404s, which the offers page
  // reports as "your NanaCoin does not support offers yet" rather than as a
  // failure. See offersSupported below.

  /** Every offer the caller can see: theirs, and any on their listings. */
  offers(): Promise<{ offers: Offer[] }> {
    return this.get<{ offers: Offer[] }>('/offers');
  }

  /** Offers against one listing. */
  offersFor(listing: ListingId): Promise<{ offers: Offer[] }> {
    return this.get<{ offers: Offer[] }>(
      `/listings/${encodeURIComponent(listing)}/offers`,
    );
  }

  /**
   * Proposes a deal. Money does not move until the listing's owner accepts.
   *
   * Idempotency-keyed like every other money-adjacent call: a retry after a
   * dropped connection must not leave two offers standing.
   */
  makeOffer(
    listing: ListingId,
    amount: number,
    message: string,
    idempotencyKey: string,
  ): Promise<Offer> {
    return this.post<Offer>(
      `/listings/${encodeURIComponent(listing)}/offers`,
      { amount, message },
      idempotencyKey,
    );
  }

  /**
   * Accepts an offer: this is the step that moves the money and closes the
   * listing, in one server-side operation for the same reason Purchase is.
   */
  acceptOffer(id: OfferId, idempotencyKey: string): Promise<OfferResult> {
    return this.post<OfferResult>(
      `/offers/${encodeURIComponent(id)}/accept`,
      {},
      idempotencyKey,
    );
  }

  /** Refuses an offer, leaving the listing open for others. */
  declineOffer(id: OfferId): Promise<Offer> {
    return this.post<Offer>(`/offers/${encodeURIComponent(id)}/decline`, {});
  }

  /** Takes back an offer you made, while it is still open. */
  withdrawOffer(id: OfferId): Promise<Offer> {
    return this.post<Offer>(`/offers/${encodeURIComponent(id)}/withdraw`, {});
  }

  /**
   * Recent server events. No token required - the commonest thing to diagnose
   * is a client that cannot authenticate.
   */
  logs(limit = 60): Promise<LogPage> {
    return this.get<LogPage>(`/logs?limit=${limit}`);
  }

  // --- config ---

  config(): Promise<Config> {
    return this.get<Config>('/admin/config');
  }

  setConfig(changes: Partial<Config>): Promise<Config> {
    return this.patch<Config>('/admin/config', changes);
  }

  // --- plumbing ---

  private headers(idempotencyKey?: string): HttpHeaders {
    let h = new HttpHeaders();
    if (this.token) h = h.set('Authorization', `Bearer ${this.token}`);
    if (idempotencyKey) h = h.set('Idempotency-Key', idempotencyKey);
    return h;
  }

  private get<T>(path: string): Promise<T> {
    return this.traced('GET', path, () =>
      firstValueFrom(
        this.http.get<T>(this.base + path, { headers: this.headers() }).pipe(this.mapError()),
      ),
    );
  }

  private post<T>(path: string, body: unknown, idempotencyKey?: string): Promise<T> {
    return this.traced(
      'POST',
      path,
      () =>
        firstValueFrom(
          this.http
            .post<T>(this.base + path, body, { headers: this.headers(idempotencyKey) })
            .pipe(this.mapError()),
        ),
      body,
    );
  }

  /**
   * Records one request and how it ended.
   *
   * Every call goes through here, rather than logging at each call site, so
   * that a request cannot be added later and quietly not be traced. The
   * duration is included because "slow" and "failed" look identical from the
   * UI, and on a board serving over weak WiFi the difference matters.
   *
   * The body is logged for writes only, and redacted on the way in - a login
   * POST carries a password, and this log is meant to be pasteable.
   */
  private async traced<T>(
    method: string,
    path: string,
    run: () => Promise<T>,
    body?: unknown,
  ): Promise<T> {
    const started = performance.now();
    this.log.debug('http', `${method} ${path}`, body === undefined ? undefined : { body });

    try {
      const result = await run();
      this.log.info('http', `${method} ${path} ok`, {
        ms: Math.round(performance.now() - started),
      });
      return result;
    } catch (e) {
      const ms = Math.round(performance.now() - started);
      if (e instanceof ApiError) {
        // The whole error, including the code, because "which failure was it"
        // is the question this log exists to answer.
        this.log.error('http', `${method} ${path} failed`, {
          ms,
          status: e.status,
          code: e.code,
          message: e.message,
        });
      } else {
        this.log.error('http', `${method} ${path} threw`, { ms, error: String(e) });
      }
      throw e;
    }
  }

  private patch<T>(path: string, body: unknown): Promise<T> {
    return firstValueFrom(
      this.http
        .patch<T>(this.base + path, body, { headers: this.headers() })
        .pipe(this.mapError()),
    );
  }

  /**
   * Turns an HttpErrorResponse into an ApiError carrying the server's code.
   *
   * A 401 clears the stored token: it means the session is gone - expired, or
   * lost when the board rebooted, since sessions are RAM-only by design.
   */
  private mapError<T>() {
    return catchError<T, Observable<never>>((err: unknown) => {
      if (!(err instanceof HttpErrorResponse)) {
        return throwError(() => new ApiError(0, 'unknown', 'Something went wrong.'));
      }

      // A 200 that could not be parsed as JSON means something answered, but
      // it was not a NanaCoin.
      //
      // The case this exists for: the site is served from a board that only
      // serves files, the API address has not been set, so the client asks
      // its own origin - and the file server's single-page fallback hands
      // back index.html with a 200. HttpClient then fails *parsing*, which
      // surfaces as a status-200 error with no server message, and the whole
      // thing used to read "Something went wrong." with nothing in the
      // console and no failed request in the network panel, because from
      // HTTP's point of view nothing failed.
      if (err.status === 200) {
        // The exact case that produced "Something went wrong." with nothing in
        // the console: a file server answering an API path with its index
        // page. Naming it here is the difference between a five-minute fix and
        // an afternoon.
        this.log.error('http', 'a 200 that was not JSON - wrong server?', {
          url: this.apiBase.label(),
          hint: 'a static file server answering /api paths returns index.html',
        });
        return throwError(
          () =>
            new ApiError(
              200,
              'not_nanacoin',
              `${this.apiBase.label()} answered, but it is not a NanaCoin API. ` +
                'Give the address of the board that is.',
            ),
        );
      }
      if (err.status === 0 || err.status === 502 || err.status === 503 || err.status === 504) {
        // Status 0 is no HTTP response at all. A 502/503/504 is a proxy or
        // gateway answering on NanaCoin's behalf because it could not reach
        // it - the dev server's proxy does exactly this when the Go server is
        // not running. Neither is NanaCoin refusing anything, and the raw
        // "Bad Gateway" tells a household member nothing, so both are
        // reported as what they are: cannot reach it.
        //
        // The request may or may not have arrived, which is why the wording
        // stops short of claiming it did not.
        return throwError(
          () =>
            new ApiError(
              err.status,
              'unreachable',
              `Could not reach NanaCoin at ${this.apiBase.label()}. It may be offline, or the address may be wrong.`,
            ),
        );
      }
      // A 401 forgets the account that made the request, not every account.
      // The session is gone - expired, or lost when the board rebooted, since
      // sessions are RAM-only by design - but the others were issued
      // separately. If the board did reboot they are equally dead, which the
      // next request each makes will discover on its own.
      if (err.status === 401) {
        this.log.warn('auth', 'session rejected, dropping this account', {
          account: this.accounts.active()?.username,
          remaining: this.accounts.all().length - 1,
        });
        this.accounts.invalidateActive();
      }

      const body = err.error as ApiErrorBody | null;
      return throwError(
        () => new ApiError(err.status, body?.error ?? 'error', body?.message ?? err.statusText),
      );
    });
  }
}

// --- PKCE and keys ----------------------------------------------------------

function base64url(bytes: Uint8Array): string {
  let s = '';
  for (const b of bytes) s += String.fromCharCode(b);
  return btoa(s).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

/** 32 random bytes as base64url: 43 characters, the RFC's minimum. */
function newVerifier(): string {
  const bytes = new Uint8Array(32);
  crypto.getRandomValues(bytes);
  return base64url(bytes);
}

/**
 * The S256 challenge for a PKCE verifier.
 *
 * Goes through digestSha256 rather than crypto.subtle directly, because
 * crypto.subtle is undefined outside a secure context - and this app is served
 * over plain http from a board at an IP or an mDNS name, neither of which
 * qualifies. localhost is exempt, which is why this worked in development and
 * failed on the board: the same code, a different origin.
 */
async function challengeFor(verifier: string): Promise<string> {
  const digest = await digestSha256(new TextEncoder().encode(verifier));
  return base64url(digest);
}

/**
 * An idempotency key for one attempted operation.
 *
 * Generated before the first attempt and reused for every retry of the same
 * operation - that is the whole point. A key made per request would defeat it.
 */
export function newIdempotencyKey(): string {
  const bytes = new Uint8Array(16);
  crypto.getRandomValues(bytes);
  return base64url(bytes);
}


