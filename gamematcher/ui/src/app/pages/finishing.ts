import { Component, inject, input, OnInit, signal } from '@angular/core';
import { Router, RouterLink } from '@angular/router';
import { Api, OAuthStatus, problem } from '../api';

/** Back from the Mastodon instance: wait for the board to finish signing in. */
@Component({
  selector: 'gm-finishing',
  imports: [RouterLink],
  template: `
    <section class="card narrow">
      @if (error()) {
        <h1>Sign-in didn't work</h1>
        <p class="error" role="alert">{{ error() }}</p>
        <a class="button" routerLink="/signin">Back to sign in</a>
      } @else {
        <h1>Signing you in…</h1>
        <div class="spinner" aria-hidden="true"></div>
        <p class="aside" role="status">Checking with your instance. This takes a few seconds.</p>
      }
    </section>
  `,
})
export class Finishing implements OnInit {
  readonly t = input<string>('');
  private readonly api = inject(Api);
  private readonly router = inject(Router);
  readonly error = signal('');

  async ngOnInit(): Promise<void> {
    if (!this.t()) return this.error.set('Nothing to finish; start again.');
    try {
      for (let i = 0; i < 60; i++) {
        const s = await this.api.get<OAuthStatus>(`/api/v1/auth/oauth/${this.t()}`, 'OAuthStatus');
        if (s.state === 'done') {
          await this.api.refresh();
          await this.router.navigateByUrl(s.result === 'linked' ? '/me' : '/');
          return;
        }
        if (s.state === 'failed') return this.error.set(s.message);
        await new Promise((r) => setTimeout(r, 700));
      }
      this.error.set('Your instance is taking too long; try again.');
    } catch (e) {
      this.error.set(problem(e));
    }
  }
}
