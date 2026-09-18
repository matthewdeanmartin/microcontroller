// The shell. Decides which of the three states the app is in - unprovisioned,
// logged out, or running - and renders the frame around the routed page.

import { Component, computed, inject, signal } from '@angular/core';
import { RouterLink, RouterLinkActive, RouterOutlet } from '@angular/router';

import { ApiBase } from './api/api-base';
import { ApiError } from './api/nanacoin.service';
import { ConnectForm } from './pages/connect-form';
import { LoginForm } from './pages/login-form';
import { LogsPage } from './pages/logs';
import { Session } from './api/session';
import { SetupForm } from './pages/setup-form';
import { ToastList } from './ui/toast-list';
import { Toasts } from './ui/toasts';

type Phase = 'loading' | 'connect' | 'setup' | 'login' | 'app' | 'logs';

@Component({
  selector: 'app-root',
  imports: [
    RouterOutlet,
    RouterLink,
    RouterLinkActive,
    ConnectForm,
    LoginForm,
    LogsPage,
    SetupForm,
    ToastList,
  ],
  templateUrl: './app.html',
  styleUrl: './app.css',
})
export class App {
  protected readonly session = inject(Session);
  protected readonly apiBase = inject(ApiBase);
  private readonly toasts = inject(Toasts);

  protected readonly phase = signal<Phase>('loading');

  /** Why the connect screen is showing, when an error put it there. */
  protected readonly connectReason = signal('');

  protected readonly household = computed(
    () => this.session.status()?.household ?? 'NanaCoin',
  );

  constructor() {
    void this.boot();
  }

  protected async boot(): Promise<void> {
    this.phase.set('loading');
    try {
      const status = await this.session.loadStatus();
      if (!status.provisioned) {
        this.phase.set('setup');
        return;
      }
      this.phase.set((await this.session.restore()) ? 'app' : 'login');
    } catch (e) {
      // Being unable to reach NanaCoin is not a dead end: the address is
      // something the user can supply, so ask for it rather than showing a
      // gateway error they can do nothing about.
      this.connectReason.set(
        e instanceof ApiError ? e.message : 'Could not reach NanaCoin.',
      );
      this.phase.set('connect');
    }
  }

  /** Called once the connect screen has proved an address answers. */
  protected onConnected(): void {
    this.connectReason.set('');
    void this.boot();
  }

  /** Lets someone who is logged in point the app at a different NanaCoin. */
  protected changeServer(): void {
    this.connectReason.set('');
    this.phase.set('connect');
  }

  /**
   * Shows the logs without a session. The failure most worth diagnosing is the
   * one that stops you logging in, so the logs cannot be behind the login.
   */
  protected showLogs(): void {
    this.phase.set('logs');
  }

  /** Back from the standalone logs view to wherever the app belongs. */
  protected leaveLogs(): void {
    void this.boot();
  }

  protected onProvisioned(): void {
    this.toasts.ok('Household created. Log in to continue.');
    void this.boot();
  }

  protected onLoggedIn(): void {
    this.phase.set('app');
  }

  protected async logout(): Promise<void> {
    await this.session.logout();
    this.phase.set('login');
  }

  protected async refresh(): Promise<void> {
    try {
      await this.session.refresh();
    } catch (e) {
      this.toasts.fromError(e);
    }
  }

}
