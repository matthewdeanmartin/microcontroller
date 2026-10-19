import { Component, inject, OnDestroy, OnInit, signal } from '@angular/core';
import { RouterLink } from '@angular/router';
import { Api, BotView, MailView, problem } from '../api';
interface DeliveryStatus { household: BotView; public: BotView; mail: MailView; }

@Component({
  selector: 'gm-status',
  imports: [RouterLink],
  template: `
    <section class="card">
      <div class="bar"><h1>Status</h1><button type="button" class="secondary" (click)="load()">Refresh</button></div>
      <p>Clock: {{ api.status()?.clock_synced ? 'synced' : 'waiting for sync' }} · Build {{ api.status()?.build }}</p>
      <p class="aside">“Sent” means the service accepted the message, not that the recipient read it. Failed attempts retry automatically.
        Manual retries keep the same delivery identity.</p>
      <a routerLink="/admin">Configure email and bots</a>
      @if (error()) { <p class="error" role="alert">{{ error() }}</p> }
    </section>
    @if (status(); as s) {
      <section class="card">
        <h2>Email</h2><p>{{ s.mail.configured ? s.mail.sender : 'Not configured' }}</p>
        <p>{{ s.mail.sent_today }} / {{ s.mail.daily_limit }} daily allowance · {{ s.mail.sent }} accepted</p>
        @if (s.mail.last_error) { <p class="error">{{ s.mail.last_error }}</p> }
      </section>
      @for (channel of channels; track channel.key) {
        @let bot = channel.key === 'household' ? s.household : s.public;
        <section class="card">
          <h2>{{ channel.label }}</h2>
          <p>{{ bot.configured ? bot.handle : 'Not configured' }}</p>
          <p>Incoming RSVPs: {{ bot.replies ? 'enabled' : 'off' }}
            @if (bot.last_poll) { · last checked {{ when(bot.last_poll) }} }</p>
          @if (bot.reply_error) { <p class="error">Incoming replies: {{ bot.reply_error }}</p> }
          @if (bot.last_error) { <p class="error">Delivery: {{ bot.last_error }}</p> }
          <ul class="outbox">
            @for (job of bot.jobs; track job.id) {
              <li>
                <strong>{{ job.state }}</strong> · {{ job.kind }} · {{ when(job.created) }}
                <p>{{ job.text }}</p>
                <span class="aside">{{ job.attempts }} attempts
                  @if (job.state === 'queued' && job.next_at) { · next attempt {{ when(job.next_at) }} }</span>
                @if (job.error) { <p class="error">{{ job.error }}</p> }
                @if (job.url) { <a [href]="job.url" target="_blank" rel="noopener">View message</a> }
                @if (job.state === 'failed' || (job.state === 'queued' && job.attempts > 0)) {
                  <button type="button" class="secondary" [disabled]="busy()" (click)="retry(channel.path, job.id)">Retry now</button>
                }
              </li>
            } @empty { <li class="aside">No recent deliveries.</li> }
          </ul>
        </section>
      }
    }
  `,
})
export class Status implements OnInit, OnDestroy {
  readonly api = inject(Api);
  readonly status = signal<DeliveryStatus | null>(null);
  readonly error = signal('');
  readonly busy = signal(false);
  readonly channels = [
    { key: 'household', label: 'Household bot and email deliveries', path: '/api/v1/admin/bot' },
    { key: 'public', label: 'Public Mastodon bot', path: '/api/v1/admin/public-bot' },
  ];
  private timer?: ReturnType<typeof setTimeout>;
  private destroyed = false;
  ngOnInit(): void { void this.load(); }
  ngOnDestroy(): void { this.destroyed = true; clearTimeout(this.timer); }
  when(seconds: number): string { return new Date(seconds * 1000).toLocaleString(); }
  async load(): Promise<void> {
    clearTimeout(this.timer);
    try { this.status.set(await this.api.get<DeliveryStatus>('/api/v1/admin/delivery', 'DeliveryStatus')); this.error.set(''); }
    catch (e) { this.error.set(problem(e)); }
    this.schedule();
  }
  private schedule(): void {
    if (!this.destroyed) this.timer = setTimeout(() => {
      if (!document.hidden) void this.load(); else this.schedule();
    }, 30000);
  }
  async retry(path: string, id: number): Promise<void> {
    this.busy.set(true);
    try { await this.api.send('POST', `${path}/retry/${id}`, undefined, 'BotView'); await this.load(); }
    catch (e) { this.error.set(problem(e)); }
    finally { this.busy.set(false); }
  }
}
