import { Component, inject, input, OnInit, signal } from '@angular/core';
import { Router, RouterLink } from '@angular/router';
import { Api, problem } from '../api';

export function value(event: Event): string {
  return (event.target as HTMLInputElement).value;
}

/** First run: whoever submits this becomes admin. */
@Component({
  selector: 'party-setup',
  template: `
    <section class="card narrow">
      <h1>Set up party</h1>
      <p>You're the first one here, so you'll be the <strong>admin</strong>. You sign in with your name (or your
        email, if you give one) and a PIN.</p>
      <form (submit)="submit($event)">
        <label>Your name <input autocomplete="nickname" maxlength="40" required (input)="name.set(v($event))" /></label>
        <label>Email <small>optional</small> <input type="email" autocomplete="email" (input)="email.set(v($event))" /></label>
        <label>PIN (4–8 digits)
          <input type="password" inputmode="numeric" pattern="[0-9]*" autocomplete="new-password" maxlength="8" required (input)="pin.set(v($event))" /></label>
        <label>PIN again
          <input type="password" inputmode="numeric" pattern="[0-9]*" autocomplete="new-password" maxlength="8" required (input)="pin2.set(v($event))" /></label>
        @if (api.status()?.setup_code_required) {
          <label>Setup code <input autocomplete="off" required (input)="code.set(v($event))" /></label>
        }
        @if (error()) { <p class="error" role="alert">{{ error() }}</p> }
        <button type="submit" [disabled]="busy()">Create admin account</button>
      </form>
    </section>
  `,
})
export class Setup {
  readonly api = inject(Api);
  private readonly router = inject(Router);
  readonly name = signal('');
  readonly email = signal('');
  readonly pin = signal('');
  readonly pin2 = signal('');
  readonly code = signal('');
  readonly error = signal('');
  readonly busy = signal(false);
  readonly v = value;

  async submit(event: Event): Promise<void> {
    event.preventDefault();
    if (this.pin() !== this.pin2()) return this.error.set("The two PINs don't match.");
    this.busy.set(true);
    this.error.set('');
    try {
      await this.api.setup(this.name(), this.email(), this.pin(), this.code());
      await this.router.navigateByUrl('/');
    } catch (e) {
      this.error.set(problem(e));
    } finally {
      this.busy.set(false);
    }
  }
}

@Component({
  selector: 'party-signin',
  imports: [RouterLink],
  template: `
    <section class="card narrow">
      <h1>Sign in</h1>
      <form (submit)="submit($event)">
        <label>Name or email <input autocomplete="username" required [value]="login()" (input)="login.set(v($event))" /></label>
        <label>PIN
          <input type="password" inputmode="numeric" pattern="[0-9]*" autocomplete="current-password" maxlength="8" required (input)="pin.set(v($event))" /></label>
        @if (error()) { <p class="error" role="alert">{{ error() }}</p> }
        <button type="submit" [disabled]="busy()">Sign in</button>
      </form>
      @if (api.status()?.providers?.length) {
        <div class="or" aria-hidden="true">or</div>
        @for (p of api.status()?.providers; track p.id) {
          <button type="button" class="secondary wide" (click)="mastodon(p.id)" [disabled]="busy()">Sign in with {{ p.name }}</button>
        }
      }
      @if (api.status()?.mail) {
        @if (!forgot()) {
          <p class="aside"><button type="button" class="link" (click)="forgot.set(true)">Forgot your PIN?</button></p>
        } @else {
          <form (submit)="reset($event)">
            <h2>New PIN</h2>
            @if (!codeSent()) {
              <p class="aside">We'll email a code to the address on your account.</p>
              <button type="submit" class="secondary" [disabled]="busy() || !login()">Email me a code</button>
            } @else {
              <p class="aside">If {{ login() }} has an email here, a 6-digit code is on its way (check spam too).</p>
              <label>Code <input inputmode="numeric" autocomplete="one-time-code" maxlength="6" required (input)="code.set(v($event))" /></label>
              <label>New PIN (4–8 digits)
                <input type="password" inputmode="numeric" pattern="[0-9]*" autocomplete="new-password" maxlength="8" required (input)="pin.set(v($event))" /></label>
              <button type="submit" [disabled]="busy()">Set PIN and sign in</button>
            }
          </form>
        }
      } @else {
        <p class="aside">Forgot your PIN? An admin can set you a new one.</p>
      }
      @if (api.status()?.signup_open) {
        <p class="aside">New here? <a routerLink="/signup">Make an account</a>.</p>
      } @else {
        <p class="aside">New here? Ask an admin, or use your invitation link.</p>
      }
    </section>
  `,
})
export class SignIn implements OnInit {
  readonly api = inject(Api);
  private readonly router = inject(Router);
  /** `?error=` from a failed Mastodon sign-in. */
  readonly errorParam = input<string>('', { alias: 'error' });
  readonly login = signal('');
  readonly pin = signal('');
  readonly code = signal('');
  readonly forgot = signal(false);
  readonly codeSent = signal(false);
  readonly error = signal('');
  readonly busy = signal(false);
  readonly v = value;

  ngOnInit(): void {
    if (this.errorParam()) this.error.set(this.errorParam());
  }

  async submit(event: Event): Promise<void> {
    event.preventDefault();
    await this.run(async () => {
      await this.api.signIn(this.login(), this.pin());
      await this.router.navigateByUrl('/');
    });
  }

  mastodon(provider: number): Promise<void> {
    return this.run(() => this.api.startMastodon(provider, 'signin'));
  }

  async reset(event: Event): Promise<void> {
    event.preventDefault();
    await this.run(async () => {
      if (!this.codeSent()) {
        await this.api.send('POST', '/api/v1/auth/pin-reset', { login: this.login() }, 'Done');
        this.codeSent.set(true);
      } else {
        this.api.me.set(await this.api.send('POST', '/api/v1/auth/pin-reset/confirm', { login: this.login(), code: this.code(), pin: this.pin() }, 'Me'));
        await this.router.navigateByUrl('/');
      }
    });
  }

  private async run(f: () => Promise<void>): Promise<void> {
    this.busy.set(true);
    this.error.set('');
    try {
      await f();
    } catch (e) {
      this.error.set(problem(e));
    } finally {
      this.busy.set(false);
    }
  }
}

@Component({
  selector: 'party-signup',
  imports: [RouterLink],
  template: `
    <section class="card narrow">
      <h1>Make an account</h1>
      <p class="aside">On the home Wi-Fi anyone can join. Your name is also how you sign in.</p>
      <form (submit)="submit($event)">
        <label>Your name <input autocomplete="nickname" maxlength="40" required (input)="name.set(v($event))" /></label>
        <label>Email <small>optional</small> <input type="email" autocomplete="email" (input)="email.set(v($event))" /></label>
        <label>PIN (4–8 digits)
          <input type="password" inputmode="numeric" pattern="[0-9]*" autocomplete="new-password" maxlength="8" required (input)="pin.set(v($event))" /></label>
        <label>PIN again
          <input type="password" inputmode="numeric" pattern="[0-9]*" autocomplete="new-password" maxlength="8" required (input)="pin2.set(v($event))" /></label>
        @if (error()) { <p class="error" role="alert">{{ error() }}</p> }
        <button type="submit" [disabled]="busy()">Make my account</button>
      </form>
      <p class="aside">Already have one? <a routerLink="/signin">Sign in</a>.</p>
    </section>
  `,
})
export class SignUp {
  readonly api = inject(Api);
  private readonly router = inject(Router);
  /** Sign up with an invitation (from the invitation page). */
  readonly invite = input<string>('');
  readonly name = signal('');
  readonly email = signal('');
  readonly pin = signal('');
  readonly pin2 = signal('');
  readonly error = signal('');
  readonly busy = signal(false);
  readonly v = value;

  async submit(event: Event): Promise<void> {
    event.preventDefault();
    if (this.pin() !== this.pin2()) return this.error.set("The two PINs don't match.");
    this.busy.set(true);
    this.error.set('');
    try {
      await this.api.signUp(this.name(), this.email(), this.pin(), this.invite());
      await this.router.navigateByUrl('/');
    } catch (e) {
      this.error.set(problem(e));
    } finally {
      this.busy.set(false);
    }
  }
}

/** Back from the instance: waits for the board to finish the sign-in. */
@Component({
  selector: 'party-finishing',
  imports: [RouterLink],
  template: `
    <section class="card narrow" aria-live="polite">
      @if (error()) {
        <h1>That didn't work</h1>
        <p class="error" role="alert">{{ error() }}</p>
        <p><a routerLink="/signin">Back to sign-in</a></p>
      } @else {
        <h1>Signing you in…</h1>
        <div class="spinner" aria-hidden="true"></div>
      }
    </section>
  `,
})
export class Finishing implements OnInit {
  private readonly api = inject(Api);
  private readonly router = inject(Router);
  readonly t = input<string>('');
  readonly error = signal('');

  async ngOnInit(): Promise<void> {
    for (let i = 0; i < 40; i++) {
      try {
        const s = await this.api.get<{ state: string; location: string; message: string }>(`/api/v1/auth/oauth/${this.t()}`, 'OAuthStatus');
        if (s.state === 'done') {
          await this.api.refresh();
          await this.router.navigateByUrl(s.location || '/');
          return;
        }
        if (s.state === 'failed') return this.error.set(s.message);
      } catch (e) {
        return this.error.set(problem(e));
      }
      await new Promise((r) => setTimeout(r, 500));
    }
    this.error.set('The board is taking too long; try again.');
  }
}
