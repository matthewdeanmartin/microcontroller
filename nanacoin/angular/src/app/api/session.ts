// Who is logged in, what the household looks like, and the one place the rest
// of the app goes to refresh it.
//
// Signals rather than a store library: this is four pieces of state and a
// reload function.

import { Injectable, computed, inject, signal } from '@angular/core';

import { ApiError, NanacoinService } from './nanacoin.service';
import { Listing, Status, User } from './models';

@Injectable({ providedIn: 'root' })
export class Session {
  private readonly api = inject(NanacoinService);

  /** The signed-in user, or null. */
  readonly me = signal<User | null>(null);

  /** Everyone in the household. Ordinary users see names but not balances. */
  readonly household = signal<User[]>([]);

  /** The server's own summary, also readable before logging in. */
  readonly status = signal<Status | null>(null);

  readonly listings = signal<Listing[]>([]);

  /** Set while a refresh is in flight, so views can show it without their own flag. */
  readonly loading = signal(false);

  readonly isNana = computed(() => this.me()?.role === 'nana');
  readonly balance = computed(() => this.me()?.balance ?? 0);
  readonly signedIn = computed(() => this.me() !== null);

  /** The household minus the signed-in user: everyone they could pay. */
  readonly recipients = computed(() => {
    const self = this.me();
    return this.household().filter((u) => u.status === 'ACTIVE' && u.id !== self?.id);
  });

  /** Active listings, which is what the market screen shows. */
  readonly forSale = computed(() => this.listings().filter((l) => l.status === 'ACTIVE'));

  /** Everything no longer for sale, kept out of the way but not hidden. */
  readonly closed = computed(() => this.listings().filter((l) => l.status !== 'ACTIVE'));

  /** Reads the public status. Safe before provisioning and before logging in. */
  async loadStatus(): Promise<Status> {
    const s = await this.api.status();
    this.status.set(s);
    return s;
  }

  /**
   * Restores a session from a stored token, if there is one that still works.
   *
   * Returns false when there is nothing to restore - including the common case
   * of a token that outlived the board's last reboot, since sessions are
   * RAM-only by design.
   */
  async restore(): Promise<boolean> {
    if (!this.api.authenticated) return false;
    try {
      this.me.set(await this.api.me());
      await this.refresh();
      return true;
    } catch {
      this.me.set(null);
      return false;
    }
  }

  async login(username: string, password: string): Promise<void> {
    this.me.set(await this.api.login(username, password));
    await this.refresh();
  }

  /**
   * Logs out locally whatever the server says.
   *
   * Telling the server is a courtesy - it lets the session be revoked
   * immediately rather than at expiry - but a failed request must not leave
   * someone still logged in on a shared computer, which is the whole reason
   * they clicked the button.
   */
  async logout(): Promise<void> {
    try {
      await this.api.logout();
    } catch {
      // Already handled by clearing local state below.
    } finally {
      this.me.set(null);
      this.household.set([]);
      this.listings.set([]);
    }
  }

  /**
   * Reloads everything the signed-in user can see, in parallel.
   *
   * Called after every mutation rather than patching state locally: the server
   * is authoritative about balances and listing status, and re-reading is both
   * simpler and correct when someone else in the house is also clicking.
   */
  async refresh(): Promise<void> {
    if (!this.me()) return;
    this.loading.set(true);
    try {
      const [me, users, listings, status] = await Promise.all([
        this.api.me(),
        this.api.users(),
        this.api.listings(),
        this.api.status(),
      ]);
      this.me.set(me);
      this.household.set(users.users);
      this.listings.set(listings.listings);
      this.status.set(status);
    } finally {
      this.loading.set(false);
    }
  }

  /** The display name behind an account id, for rendering the other side of a posting. */
  nameFor(accountId: string): string {
    const u = this.household().find((h) => h.account === accountId);
    return u?.display_name ?? accountId;
  }
}

/** True when an error means the session is gone and the user must log in again. */
export function isAuthError(e: unknown): boolean {
  return e instanceof ApiError && e.status === 401;
}
