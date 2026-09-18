import { Component, inject, signal } from '@angular/core';
import { FormsModule } from '@angular/forms';

import { NanacoinService, newIdempotencyKey } from '../api/nanacoin.service';
import { Session } from '../api/session';
import { Toasts } from '../ui/toasts';

@Component({
  selector: 'app-send',
  imports: [FormsModule],
  template: `
    <h2>Send coins</h2>

    @if (session.recipients().length === 0) {
      <p class="muted">
        There is nobody else in the household yet. Nana adds members from the
        Household tab.
      </p>
    } @else {
      <form (ngSubmit)="send()">
        <label>
          To
          <select name="to" [(ngModel)]="to" required>
            <option value="" disabled>Choose someone</option>
            @for (u of session.recipients(); track u.id) {
              <option [value]="u.account">{{ u.display_name }}</option>
            }
          </select>
        </label>
        <label>
          Amount
          <input name="amount" type="number" min="1" step="1" [(ngModel)]="amount" required />
        </label>
        <label>
          What for?
          <input name="memo" [(ngModel)]="memo" maxlength="140"
                 placeholder="Taking out the trash" />
        </label>
        <button class="btn" type="submit" [disabled]="busy()">
          {{ busy() ? 'Sending…' : 'Send' }}
        </button>
      </form>

      <p class="muted small">
        You have {{ session.balance() }} {{ session.balance() === 1 ? 'coin' : 'coins' }}.
      </p>
    }
  `,
})
export class SendPage {
  private readonly api = inject(NanacoinService);
  private readonly toasts = inject(Toasts);
  protected readonly session = inject(Session);

  protected to = '';
  protected amount: number | null = null;
  protected memo = '';
  protected readonly busy = signal(false);

  protected async send(): Promise<void> {
    if (this.busy()) return;

    if (!this.to) {
      // An empty <select required> passes browser validation when it has no
      // options, so this is checked rather than assumed.
      this.toasts.error('Choose who the coins are for.');
      return;
    }
    const amount = Number(this.amount);
    if (!Number.isInteger(amount) || amount <= 0) {
      this.toasts.error('Enter a whole number of coins.');
      return;
    }

    this.busy.set(true);
    // Generated once per attempted transfer, before the request. A retry with
    // this same key returns the original transaction instead of sending twice.
    const key = newIdempotencyKey();
    try {
      await this.api.transfer(this.to, amount, this.memo.trim(), key);
      this.amount = null;
      this.memo = '';
      this.toasts.ok('Sent.');
      await this.session.refresh();
    } catch (e) {
      this.toasts.fromError(e);
    } finally {
      this.busy.set(false);
    }
  }
}
