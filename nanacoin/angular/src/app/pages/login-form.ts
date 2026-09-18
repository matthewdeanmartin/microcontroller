import { Component, inject, input, output, signal } from '@angular/core';
import { FormsModule } from '@angular/forms';

import { Session } from '../api/session';
import { Toasts } from '../ui/toasts';

@Component({
  selector: 'app-login-form',
  imports: [FormsModule],
  template: `
    <div class="panel">
      <h1>{{ household() }}</h1>

      <form (ngSubmit)="submit()">
        <label>
          Username
          <input name="username" [(ngModel)]="username" required autocomplete="username" />
        </label>
        <label>
          PIN or password
          <input name="password" type="password" [(ngModel)]="password" required
                 autocomplete="current-password" />
        </label>
        <button class="btn" type="submit" [disabled]="busy()">
          {{ busy() ? 'Logging in…' : 'Log in' }}
        </button>
      </form>
    </div>
  `,
})
export class LoginForm {
  private readonly session = inject(Session);
  private readonly toasts = inject(Toasts);

  readonly household = input('NanaCoin');
  readonly done = output<void>();

  protected username = '';
  protected password = '';
  protected readonly busy = signal(false);

  protected async submit(): Promise<void> {
    if (this.busy()) return;
    this.busy.set(true);
    try {
      await this.session.login(this.username.trim(), this.password);
      // Clear the password whatever happens next; it has served its purpose
      // and there is no reason for it to sit in a component field.
      this.password = '';
      this.done.emit();
    } catch (e) {
      this.password = '';
      this.toasts.fromError(e);
    } finally {
      this.busy.set(false);
    }
  }
}
