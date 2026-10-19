import { Component, computed, inject, input, OnInit, signal } from '@angular/core';
import { Router, RouterLink } from '@angular/router';
import { Api, problem } from '../api';

function value(event: Event): string {
  return (event.target as HTMLInputElement).value;
}

/** First run: whoever submits this becomes admin. */
@Component({
  selector: 'gm-setup',
  template: `
    <section class="card narrow">
      <h1>Set up Game Matcher</h1>
      <p>You're the first one here, so you'll be the <strong>admin</strong>. Your email is your sign-in name
        @if (api.status()?.mail) { and where "forgot PIN?" codes go. } @else { (nothing is sent to it). }</p>
      <form (submit)="submit($event)">
        <label>Email <input type="email" autocomplete="username" required (input)="email.set(v($event))" /></label>
        <label>Your name <input autocomplete="nickname" maxlength="32" required (input)="name.set(v($event))" /></label>
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
  readonly email = signal('');
  readonly name = signal('');
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
      await this.api.setup(this.email(), this.name(), this.pin(), this.code());
      await this.router.navigateByUrl('/');
    } catch (e) {
      this.error.set(problem(e));
    } finally {
      this.busy.set(false);
    }
  }
}

@Component({
  selector: 'gm-signin',
  imports: [RouterLink],
  template: `
    <section class="card narrow">
      <h1>Sign in</h1>
      <form (submit)="submit($event)">
        <label>Email <input type="email" autocomplete="username" required (input)="email.set(v($event))" /></label>
        <label>PIN
          <input type="password" inputmode="numeric" pattern="[0-9]*" autocomplete="current-password" maxlength="8" required (input)="pin.set(v($event))" /></label>
        @if (error()) { <p class="error" role="alert">{{ error() }}</p> }
        <button type="submit" [disabled]="busy()">Sign in</button>
      </form>
      @if (api.status()?.mail) {
        @if (!forgot()) {
          <p class="aside"><button type="button" class="link" (click)="forgot.set(true)">Forgot your PIN?</button></p>
        } @else {
          <form (submit)="reset($event)">
            <h2>New PIN</h2>
            @if (!codeSent()) {
              <p class="aside">We'll email a code to the address you sign in with.</p>
              <label>Email <input type="email" autocomplete="username" required [value]="email()" (input)="email.set(v($event))" /></label>
              <button type="submit" class="secondary" [disabled]="busy()">Email me a code</button>
            } @else {
              <p class="aside">If {{ email() }} has an account, a 6-digit code is on its way (check spam too).</p>
              <label>Code <input inputmode="numeric" autocomplete="one-time-code" maxlength="6" required (input)="code.set(v($event))" /></label>
              <label>New PIN (4–8 digits)
                <input type="password" inputmode="numeric" pattern="[0-9]*" autocomplete="new-password" maxlength="8" required (input)="pin.set(v($event))" /></label>
              <button type="submit" [disabled]="busy()">Set PIN and sign in</button>
            }
          </form>
        }
      }
      @if (api.status()?.providers?.length) {
        <div class="or" aria-hidden="true">or</div>
        @for (p of api.status()?.providers; track p.id) {
          <button type="button" class="secondary wide" (click)="mastodon(p.id)" [disabled]="busy()">Sign in with {{ p.name }}</button>
        }
      }
      @if (api.status()?.signup_open) {
        <p class="aside">New here? <a routerLink="/signup">Make an account</a>. Guests welcome.</p>
      } @else {
        <p class="aside">New here? Ask the admin to make you an account.</p>
      }
    </section>
  `,
})
export class SignIn implements OnInit {
  readonly api = inject(Api);
  private readonly router = inject(Router);
  /** `?error=` from a failed Mastodon sign-in. */
  readonly errorParam = input<string>('', { alias: 'error' });

  ngOnInit(): void {
    if (this.errorParam()) this.error.set(this.errorParam());
  }

  async mastodon(provider: number): Promise<void> {
    this.busy.set(true);
    this.error.set('');
    try {
      await this.api.startMastodon(provider);
    } catch (e) {
      this.error.set(problem(e));
      this.busy.set(false);
    }
  }
  readonly email = signal('');
  readonly pin = signal('');
  readonly error = signal('');
  readonly busy = signal(false);
  readonly forgot = signal(false);
  readonly codeSent = signal(false);
  readonly code = signal('');
  readonly v = value;

  async reset(event: Event): Promise<void> {
    event.preventDefault();
    this.busy.set(true);
    this.error.set('');
    try {
      if (!this.codeSent()) {
        await this.api.send('POST', '/api/v1/auth/pin-reset', { email: this.email() }, 'Done');
        this.codeSent.set(true);
      } else {
        await this.api.send('POST', '/api/v1/auth/pin-reset/confirm', { email: this.email(), code: this.code(), pin: this.pin() }, 'Me');
        await this.api.refresh();
        await this.router.navigateByUrl('/');
      }
    } catch (e) {
      this.error.set(problem(e));
    } finally {
      this.busy.set(false);
    }
  }

  async submit(event: Event): Promise<void> {
    event.preventDefault();
    this.busy.set(true);
    this.error.set('');
    try {
      await this.api.signIn(this.email(), this.pin());
      await this.router.navigateByUrl('/');
    } catch (e) {
      this.error.set(problem(e));
    } finally {
      this.busy.set(false);
    }
  }
}

@Component({
  selector: 'gm-signup',
  imports: [RouterLink],
  template: `
    <section class="card narrow">
      <h1>Make an account</h1>
      @if (!open()) {
        <p>Sign-up is closed. Ask the admin to make you an account.</p>
      } @else {
        <p>Your email is your sign-in name@if (api.status()?.mail) {, and where plan news and "forgot PIN?" codes go (you can turn plan news off)} @else {; nothing is sent to it}. Pick a PIN you'll remember.</p>
        <form (submit)="submit($event)">
          <label>Email <input type="email" autocomplete="username" required (input)="email.set(v($event))" /></label>
          <label>Your name <input autocomplete="nickname" maxlength="32" required (input)="name.set(v($event))" /></label>
          <label>PIN (4–8 digits)
            <input type="password" inputmode="numeric" pattern="[0-9]*" autocomplete="new-password" maxlength="8" required (input)="pin.set(v($event))" /></label>
          <label>PIN again
            <input type="password" inputmode="numeric" pattern="[0-9]*" autocomplete="new-password" maxlength="8" required (input)="pin2.set(v($event))" /></label>
          @if (error()) { <p class="error" role="alert">{{ error() }}</p> }
          <button type="submit" [disabled]="busy()">Make account</button>
        </form>
      }
      <p class="aside">Already have one? <a routerLink="/signin">Sign in</a></p>
    </section>
  `,
})
export class SignUp {
  readonly api = inject(Api);
  private readonly router = inject(Router);
  readonly open = computed(() => this.api.status()?.signup_open ?? true);
  readonly email = signal('');
  readonly name = signal('');
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
      await this.api.signUp(this.email(), this.name(), this.pin());
      await this.router.navigateByUrl('/');
    } catch (e) {
      this.error.set(problem(e));
    } finally {
      this.busy.set(false);
    }
  }
}
