import { Component, inject, input, OnInit, signal } from '@angular/core';
import { Router, RouterLink } from '@angular/router';
import { InvitePreview } from 'party-core';
import { Api, problem } from '../api';
import { SignUp } from './auth';

/** `/invite/:token`: what it's for, then accept it (signed in, by sign-up, or with Mastodon). */
@Component({
  selector: 'party-invite',
  imports: [RouterLink, SignUp],
  template: `
    @if (error()) { <p class="error card" role="alert">{{ error() }}</p> }
    @if (preview(); as p) {
      <section class="card narrow">
        @if (p.valid) {
          <h1>You're invited</h1>
          <p><strong>{{ p.issuer_name }}</strong> invited you to the {{ p.target_kind }} <strong>{{ p.target_name }}</strong>.</p>
          @if (api.me(); as me) {
            <button type="button" (click)="accept()" [disabled]="busy()">Accept as {{ me.display_name }}</button>
          } @else {
            @for (prov of p.providers; track prov.id) {
              <button type="button" class="secondary wide" (click)="mastodon(prov.id)" [disabled]="busy()">Accept with {{ prov.name }}</button>
            }
            <p class="aside">Already have an account here? <a routerLink="/signin">Sign in</a> and open this link again.</p>
          }
        } @else {
          <h1>Invitation</h1>
          <p class="error">{{ p.reason }}</p>
        }
      </section>
      @if (p.valid && !api.me()) {
        <party-signup [invite]="token()" />
      }
    }
  `,
})
export class Invite implements OnInit {
  readonly api = inject(Api);
  private readonly router = inject(Router);
  readonly token = input<string>('');
  readonly preview = signal<InvitePreview | null>(null);
  readonly error = signal('');
  readonly busy = signal(false);

  async ngOnInit(): Promise<void> {
    await this.api.refresh().catch(() => undefined);
    try {
      this.preview.set(await this.api.get<InvitePreview>(`/api/v1/invitations/${this.token()}`, 'InvitePreview'));
    } catch (e) {
      this.error.set(problem(e));
    }
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

  accept(): Promise<void> {
    return this.run(async () => {
      const p = await this.api.send<InvitePreview>('POST', `/api/v1/invitations/${this.token()}/redeem`, undefined, 'InvitePreview');
      await this.router.navigate(p.target_kind === 'event' ? ['/events', p.target] : ['/groups', p.target]);
    });
  }

  mastodon(provider: number): Promise<void> {
    return this.run(() => this.api.startMastodon(provider, 'redeem', this.token()));
  }
}
