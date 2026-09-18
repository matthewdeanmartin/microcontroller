// The NanaCoin API client.
//
// One service, HttpClient, no state beyond the bearer token. Everything the
// UI knows about money it learns from here; nothing here decides whether a
// transaction is valid.

import { HttpClient, HttpErrorResponse, HttpHeaders } from '@angular/common/http';
import { Injectable, inject } from '@angular/core';
import { Observable, catchError, firstValueFrom, throwError } from 'rxjs';

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
  ListingStatus,
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

const TOKEN_KEY = 'nanacoin.token';

@Injectable({ providedIn: 'root' })
export class NanacoinService {
  private readonly http = inject(HttpClient);
  private readonly apiBase = inject(ApiBase);

  /** Read per request, so changing the server takes effect without a reload. */
  private get base(): string {
    return this.apiBase.current();
  }

  private token: string | null = readStoredToken();

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
    const verifier = newVerifier();
    const challenge = await challengeFor(verifier);
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

    this.setToken(tok.access_token);
    return tok.user;
  }

  async logout(): Promise<void> {
    try {
      await this.post<void>('/auth/logout', {});
    } finally {
      // Whatever the server said, this browser is logged out.
      this.setToken(null);
    }
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

  private setToken(token: string | null) {
    this.token = token;
    try {
      if (token) sessionStorage.setItem(TOKEN_KEY, token);
      else sessionStorage.removeItem(TOKEN_KEY);
    } catch {
      // Private browsing and blocked site data both throw. The cost is a
      // re-login on refresh, which beats failing to work at all.
    }
  }

  private headers(idempotencyKey?: string): HttpHeaders {
    let h = new HttpHeaders();
    if (this.token) h = h.set('Authorization', `Bearer ${this.token}`);
    if (idempotencyKey) h = h.set('Idempotency-Key', idempotencyKey);
    return h;
  }

  private get<T>(path: string): Promise<T> {
    return firstValueFrom(
      this.http.get<T>(this.base + path, { headers: this.headers() }).pipe(this.mapError()),
    );
  }

  private post<T>(path: string, body: unknown, idempotencyKey?: string): Promise<T> {
    return firstValueFrom(
      this.http
        .post<T>(this.base + path, body, { headers: this.headers(idempotencyKey) })
        .pipe(this.mapError()),
    );
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
      if (err.status === 401) this.setToken(null);

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

async function challengeFor(verifier: string): Promise<string> {
  const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(verifier));
  return base64url(new Uint8Array(digest));
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

function readStoredToken(): string | null {
  try {
    // sessionStorage, not localStorage: on a shared household computer,
    // closing the tab should end the session.
    return sessionStorage.getItem(TOKEN_KEY);
  } catch {
    return null;
  }
}
