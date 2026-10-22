import { Component, inject } from '@angular/core';
import { NavigationError, Router, RouterLink, RouterLinkActive, RouterOutlet } from '@angular/router';
import { Api } from './api';
import { UpdateRecovery } from './update-recovery';

@Component({
  selector: 'party-root',
  imports: [RouterOutlet, RouterLink, RouterLinkActive],
  template: `
    <header>
      <a class="brand" routerLink="/">🎉 party</a>
      @if (api.me(); as me) {
        <nav aria-label="Main">
          <a routerLink="/" routerLinkActive="active" [routerLinkActiveOptions]="{ exact: true }">Home</a>
          <a routerLink="/groups" routerLinkActive="active">Groups</a>
          <a routerLink="/people" routerLinkActive="active">People</a>
          <a routerLink="/notifications" routerLinkActive="active" class="bell"
            [attr.aria-label]="'Notifications' + (api.summary()?.unread ? ', ' + api.summary()?.unread + ' unread' : '')">🔔@if (api.summary()?.unread) {
              <span class="badge">{{ api.summary()?.unread }}</span>
            }</a>
          <a routerLink="/me" routerLinkActive="active">{{ me.display_name }}</a>
          @if (me.role === 'admin') {
            <a routerLink="/admin" routerLinkActive="active">Admin</a>
          }
          <button type="button" class="link" (click)="signOut()">Sign out</button>
        </nav>
      }
    </header>
    @if (recovery.failed()) {
      <div class="overlay" role="alertdialog" aria-labelledby="update-title">
        <h1 id="update-title">Couldn't finish updating</h1>
        <p>party was updated, but this page couldn't load the new version. The board may still be
          restarting. Try again in a moment.</p>
        <button type="button" (click)="recovery.retry()">Try again</button>
      </div>
    } @else if (recovery.updating()) {
      <div class="overlay" role="status" aria-live="polite">
        <div class="spinner" aria-hidden="true"></div>
        <h1>party was updated</h1>
        <p>Loading the new version…</p>
      </div>
    }
    <main>
      @if (api.ready() && !api.status()) {
        <p class="error card" role="alert">The board is not responding. Is it on, and are you on the home Wi-Fi?</p>
      }
      <router-outlet />
    </main>
    @if (api.status(); as s) {
      <footer>
        {{ s.secure ? 'HTTPS' : 'HTTP' }} · v{{ s.version }} ({{ s.build }})
        @if (!s.clock_synced) {
          · clock not set yet
        }
      </footer>
    }
  `,
})
export class App {
  readonly api = inject(Api);
  readonly recovery = inject(UpdateRecovery);
  private readonly router = inject(Router);

  constructor() {
    // A lazy route whose chunk is gone surfaces as a navigation error.
    this.router.events.subscribe((e) => {
      if (e instanceof NavigationError) this.recovery.recover(e.error, e.url);
    });
    this.recovery.markApplicationStableAfterDelay();
  }

  async signOut(): Promise<void> {
    await this.api.signOut().catch(() => undefined);
    await this.router.navigateByUrl('/signin');
  }
}
