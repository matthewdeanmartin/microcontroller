// Your own transactions, newest first.

import { Component, computed, inject, resource } from '@angular/core';

import { Transaction } from '../api/models';
import { NanacoinService } from '../api/nanacoin.service';
import { Session } from '../api/session';

/** One row, already reduced to what this account actually experienced. */
interface Row {
  txn: Transaction;
  /** This account's net change: what happened to you, not the gross amount. */
  delta: number;
  /** The other party's display name, where there is a single one. */
  other: string;
  label: string;
}

@Component({
  selector: 'app-history',
  template: `
    <h2>Your history</h2>

    @if (history.isLoading()) {
      <p class="muted">Loading…</p>
    } @else if (history.error()) {
      <p class="muted">Could not load your history.</p>
    } @else if (rows().length === 0) {
      <p class="muted">No transactions yet.</p>
    } @else {
      <div class="history">
        @for (r of rows(); track r.txn.id) {
          <div class="txn" [class.txn--in]="r.delta >= 0" [class.txn--out]="r.delta < 0">
            <div class="txn__main">
              <span class="txn__desc">{{ r.txn.description || r.label }}</span>
              @if (r.other) {
                <span class="txn__who">
                  {{ r.delta < 0 ? 'to' : 'from' }} {{ r.other }}
                </span>
              }
            </div>

            <div class="txn__side">
              <span class="txn__amount">{{ r.delta >= 0 ? '+' : '' }}{{ r.delta }}</span>
              <span class="txn__when">{{ when(r.txn.created_at) }}</span>
            </div>

            @if (r.txn.reversed_by) {
              <span class="tag tag--warn">reversed</span>
            }
            @if (r.txn.kind === 'REVERSAL') {
              <span class="tag">correction</span>
            }
          </div>
        }
      </div>
    }
  `,
})
export class HistoryPage {
  private readonly api = inject(NanacoinService);
  private readonly session = inject(Session);

  /**
   * Reloads whenever the signed-in account changes. The balance in the top bar
   * comes from Session; this is the only page that needs the transaction list,
   * so it fetches its own rather than putting it in shared state.
   */
  protected readonly history = resource({
    params: () => ({ account: this.session.me()?.account }),
    loader: ({ params }) =>
      params.account
        ? this.api.accountHistory(params.account, 50)
        : Promise.resolve({ account: '', balance: 0, transactions: [] }),
  });

  protected readonly rows = computed<Row[]>(() => {
    const account = this.session.me()?.account;
    const txns = this.history.value()?.transactions ?? [];
    if (!account) return [];

    return txns.map((txn) => {
      // Sum this account's own postings: a transaction may touch it more than
      // once, and the net effect is what the user experienced.
      let delta = 0;
      for (const p of txn.postings) if (p.account === account) delta += p.amount;

      const other = txn.postings.find((p) => p.account !== account);
      return { txn, delta, other: other?.name ?? '', label: kindLabel(txn.kind) };
    });
  });

  protected when(unixSeconds: number): string {
    return new Date(unixSeconds * 1000).toLocaleString(undefined, {
      month: 'short',
      day: 'numeric',
      hour: 'numeric',
      minute: '2-digit',
    });
  }
}

function kindLabel(kind: Transaction['kind']): string {
  switch (kind) {
    case 'ISSUE':
      return 'Issued';
    case 'RETIRE':
      return 'Retired';
    case 'PURCHASE':
      return 'Purchase';
    case 'REVERSAL':
      return 'Correction';
    default:
      return 'Transfer';
  }
}
