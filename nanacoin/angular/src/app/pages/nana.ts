// Nana's page: the household, the full ledger, and the privileged actions.
//
// Every control here is also enforced server-side. Hiding this tab from
// ordinary users is a courtesy, not the security boundary.

import { Component, inject, signal } from '@angular/core';
import { FormsModule } from '@angular/forms';

import { Transaction } from '../api/models';
import { NanacoinService, newIdempotencyKey } from '../api/nanacoin.service';
import { Session } from '../api/session';
import { Dialogs } from '../ui/dialog';
import { LiveSeeder, defaultSeed } from '../demo/seed-live';
import { Toasts } from '../ui/toasts';

@Component({
  selector: 'app-nana',
  imports: [FormsModule],
  templateUrl: './nana.html',
})
export class NanaPage {
  private readonly api = inject(NanacoinService);
  private readonly toasts = inject(Toasts);
  private readonly dialogs = inject(Dialogs);
  protected readonly seeder = inject(LiveSeeder);
  protected readonly session = inject(Session);

  protected readonly ledger = signal<Transaction[]>([]);
  protected readonly loadingLedger = signal(false);

  // Add a member.
  protected newUsername = '';
  protected newDisplayName = '';
  protected newPassword = '';
  protected grant = true;
  protected readonly adding = signal(false);

  // Issue coins.
  protected issueTo = '';
  protected issueAmount: number | null = null;
  protected issueReason = '';
  protected readonly issuing = signal(false);

  // Issue dollars. Kept separate from the coin form rather than adding a
  // currency dropdown to it: issuing dollars is a different act with a
  // different unit, and a dropdown that silently changes what "500" means is
  // exactly the kind of thing that gets someone issued $5 instead of 5 coins.
  protected usdTo = '';
  protected usdCents: number | null = null;
  protected usdReason = '';
  protected readonly issuingUsd = signal(false);

  protected readonly reversing = signal<string | null>(null);

  constructor() {
    void this.loadLedger();
  }

  protected async loadLedger(): Promise<void> {
    this.loadingLedger.set(true);
    try {
      const page = await this.api.ledger(50);
      this.ledger.set(page.transactions);
    } catch (e) {
      this.toasts.fromError(e);
    } finally {
      this.loadingLedger.set(false);
    }
  }

  protected async addMember(): Promise<void> {
    if (this.adding()) return;
    this.adding.set(true);
    try {
      await this.api.createUser(
        this.newUsername.trim(),
        this.newDisplayName.trim(),
        this.newPassword,
        this.grant,
      );
      this.newUsername = '';
      this.newDisplayName = '';
      this.newPassword = '';
      this.toasts.ok('Member added.');
      await Promise.all([this.session.refresh(), this.loadLedger()]);
    } catch (e) {
      this.toasts.fromError(e);
    } finally {
      this.adding.set(false);
    }
  }

  protected async issue(): Promise<void> {
    if (this.issuing()) return;

    if (!this.issueTo) {
      this.toasts.error('Choose who the coins are for.');
      return;
    }
    const amount = Number(this.issueAmount);
    if (!Number.isInteger(amount) || amount <= 0) {
      this.toasts.error('Enter a whole number of coins.');
      return;
    }

    this.issuing.set(true);
    try {
      await this.api.issue(this.issueTo, amount, this.issueReason.trim(), newIdempotencyKey());
      this.issueAmount = null;
      this.issueReason = '';
      this.toasts.ok('Issued.');
      await Promise.all([this.session.refresh(), this.loadLedger()]);
    } catch (e) {
      this.toasts.fromError(e);
    } finally {
      this.issuing.set(false);
    }
  }

  protected async issueDollars(): Promise<void> {
    if (this.issuingUsd()) return;

    if (!this.usdTo) {
      this.toasts.error('Choose who the dollars are for.');
      return;
    }
    const cents = Number(this.usdCents);
    if (!Number.isInteger(cents) || cents <= 0) {
      this.toasts.error('Enter a whole number of cents.');
      return;
    }

    this.issuingUsd.set(true);
    try {
      await this.api.issueUSD(this.usdTo, cents, this.usdReason.trim(), newIdempotencyKey());
      this.usdCents = null;
      this.usdReason = '';
      this.toasts.ok('Issued.');
      await Promise.all([this.session.refresh(), this.loadLedger()]);
    } catch (e) {
      this.toasts.fromError(e);
    } finally {
      this.issuingUsd.set(false);
    }
  }

  /**
   * Fills the household with a year of plausible history.
   *
   * Confirmed first, and the confirmation says what it will actually do: this
   * writes hundreds of real transactions, and on the board that is minutes of
   * work and a meaningful slice of the ledger's 365-record window.
   */
  protected async seedDemo(): Promise<void> {
    const opts = defaultSeed();
    const ok = await this.dialogs.confirm({
      title: 'Add a year of history?',
      message: 'Everything is written through the ordinary API, so it is all real.',
      detail: [
        `${opts.members} members, each with a starting float`,
        `${opts.listings} listings, some of them want-ads`,
        `about ${opts.weeks * opts.perWeek} transactions`,
        'A couple of minutes against the board.',
      ],
      confirmLabel: 'Add it',
    });
    if (ok === null) return;

    await this.seeder.run(opts);
    await Promise.all([this.session.refresh(), this.loadLedger()]);

    const failure = this.seeder.lastError();
    if (!failure) {
      this.toasts.ok('A year of history added.');
      return;
    }
    // A failed seed used to say so only in text inside this panel - which
    // collapses when the page re-renders, so the run appeared to stop for no
    // reason and with nothing on screen. Say it where every other failure is
    // said.
    this.toasts.error(`Seeding stopped: ${failure}`);
  }

  /**
   * Reverses a transaction by appending its mirror image. The original is
   * never edited, so the household gets an audit trail rather than a rewritten
   * history.
   *
   * This may leave an account negative - if the recipient already spent the
   * money - and that is deliberate: the correction matters more than the
   * invariant, and the negative balance is left visible.
   */
  protected async reverse(t: Transaction): Promise<void> {
    if (this.reversing()) return;

    const reason = await this.dialogs.prompt({
      title: 'Reverse this transaction?',
      message:
        'This appends a correction. Nothing is deleted, and the original stays in the ledger.',
      detail: [t.description || t.kind],
      placeholder: 'Why is this being reversed?',
      confirmLabel: 'Reverse',
      required: true,
      danger: true,
    });
    if (reason === null) return;

    this.reversing.set(t.id);
    try {
      await this.api.reverse(t.id, reason, newIdempotencyKey());
      this.toasts.ok('Reversed.');
      await Promise.all([this.session.refresh(), this.loadLedger()]);
    } catch (e) {
      this.toasts.fromError(e);
    } finally {
      this.reversing.set(null);
    }
  }

  protected async setStatus(id: string, status: 'ACTIVE' | 'DISABLED'): Promise<void> {
    try {
      await this.api.setUserStatus(id, status);
      this.toasts.ok(status === 'DISABLED' ? 'Member disabled.' : 'Member re-enabled.');
      await this.session.refresh();
    } catch (e) {
      this.toasts.fromError(e);
    }
  }

  /** A reversal cannot itself be reversed, and issuance is corrected by retiring. */
  protected reversible(t: Transaction): boolean {
    return !t.reversed_by && t.kind !== 'REVERSAL' && t.kind !== 'ISSUE';
  }

  protected kindLabel(kind: Transaction['kind']): string {
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

  protected when(unixSeconds: number): string {
    return new Date(unixSeconds * 1000).toLocaleString(undefined, {
      month: 'short',
      day: 'numeric',
      hour: 'numeric',
      minute: '2-digit',
    });
  }

  protected journalKb(): string {
    const used = this.session.status()?.journal_used ?? 0;
    return `${(used / 1024).toFixed(1)} KB`;
  }
}
