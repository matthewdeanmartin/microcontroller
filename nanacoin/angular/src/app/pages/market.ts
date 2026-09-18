// The marketplace: what is for sale, and the form for offering something.

import { Component, inject, signal } from '@angular/core';
import { FormsModule } from '@angular/forms';

import { Listing } from '../api/models';
import { NanacoinService, newIdempotencyKey } from '../api/nanacoin.service';
import { Session } from '../api/session';
import { Toasts } from '../ui/toasts';

@Component({
  selector: 'app-market',
  imports: [FormsModule],
  templateUrl: './market.html',
})
export class MarketPage {
  private readonly api = inject(NanacoinService);
  private readonly toasts = inject(Toasts);
  protected readonly session = inject(Session);

  protected title = '';
  protected description = '';
  protected price: number | null = null;

  protected readonly posting = signal(false);

  /** The listing currently being bought, so only its own button shows a spinner. */
  protected readonly buying = signal<string | null>(null);

  protected mine(l: Listing): boolean {
    return l.seller === this.session.me()?.account;
  }

  protected affordable(l: Listing): boolean {
    return this.session.balance() >= l.price;
  }

  /** For a currency listing: '5.00 USD'. */
  protected cashAmount(l: Listing): string {
    return `${((l.minor_units ?? 0) / 100).toFixed(2)} ${l.currency}`;
  }

  protected async post(): Promise<void> {
    const price = Number(this.price);
    if (!Number.isInteger(price) || price <= 0) {
      // Coins are whole numbers; there is no fractional NanaCoin.
      this.toasts.error('Enter a whole number of coins.');
      return;
    }
    this.posting.set(true);
    try {
      await this.api.createListing({
        title: this.title.trim(),
        description: this.description.trim(),
        price,
      });
      this.title = '';
      this.description = '';
      this.price = null;
      this.toasts.ok('Listed.');
      await this.session.refresh();
    } catch (e) {
      this.toasts.fromError(e);
    } finally {
      this.posting.set(false);
    }
  }

  /**
   * Buying is one request. The client never transfers and then marks the
   * listing sold - those could partially succeed - and the server does not
   * offer that shape anyway.
   *
   * The idempotency key is made here, before the attempt, so that a retry
   * after a dropped connection is recognisably the same purchase.
   */
  protected async buy(l: Listing): Promise<void> {
    if (this.buying()) return;
    this.buying.set(l.id);
    const key = newIdempotencyKey();
    try {
      const res = await this.api.purchase(l.id, key);
      this.toasts.ok(`Bought ${res.listing.title} for ${res.listing.price} coins.`);
      await this.session.refresh();
    } catch (e) {
      this.toasts.fromError(e);
    } finally {
      this.buying.set(null);
    }
  }

  protected async cancel(l: Listing): Promise<void> {
    try {
      await this.api.cancelListing(l.id);
      this.toasts.ok('Listing cancelled.');
      await this.session.refresh();
    } catch (e) {
      this.toasts.fromError(e);
    }
  }
}
