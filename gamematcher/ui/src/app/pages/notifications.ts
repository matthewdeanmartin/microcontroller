import { Component, inject, OnInit, signal } from '@angular/core';
import { RouterLink } from '@angular/router';
import { Api, NotificationView, problem } from '../api';
import { dayName } from '../fmt';

/** What a notification says. */
export function sentence(n: NotificationView): string {
  const t = `“${n.plan_title}”`;
  const many = (one: string, more: string) => (n.count === 1 ? one : more.replace('#', String(n.count)));
  switch (n.kind) {
    case 'new_plan':
      return `${n.actor_name} posted ${t}, with a game you're into`;
    case 'responses':
      return many(`${n.actor_name} answered ${t}`, `# new answers on ${t}, latest from ${n.actor_name}`);
    case 'locked':
      return `${n.actor_name} locked in ${t}`;
    case 'reopened':
      return `${t} is open again`;
    case 'changed':
      return `${n.actor_name} changed the times of ${t}`;
    case 'cancelled':
      return `${t} was cancelled`;
    case 'comments':
      return many(`${n.actor_name} commented on ${t}`, `# new comments on ${t}, latest from ${n.actor_name}`);
    case 'host':
      return `${n.actor_name} handed ${t} to you; you're the host now`;
    case 'reminder':
      return `${t} starts soon`;
    default:
      return t;
  }
}

@Component({
  selector: 'gm-notifications',
  imports: [RouterLink],
  template: `
    <section class="card">
      <h1>Notifications</h1>
      @if (error()) { <p class="error" role="alert">{{ error() }}</p> }
      <ul class="plans">
        @for (n of list(); track n.id) {
          <li [class.unread]="!n.read">
            <a [routerLink]="['/plans', n.plan]">{{ sentence(n) }}</a>
            <span class="aside">{{ when(n.created) }}</span>
          </li>
        } @empty {
          <li class="aside">Nothing yet. You'll hear about new plans for games you've starred, and about plans you're in.</li>
        }
      </ul>
    </section>
  `,
})
export class Notifications implements OnInit {
  readonly api = inject(Api);
  readonly list = signal<NotificationView[]>([]);
  readonly error = signal('');
  readonly sentence = sentence;

  async ngOnInit(): Promise<void> {
    try {
      const list = (await this.api.get<{ notifications: NotificationView[] }>('/api/v1/notifications', 'NotificationList'))
        .notifications;
      this.list.set(list);
      // Seen now: mark everything shown as read (unread ones stay highlighted
      // on this page until you leave it).
      if (list.some((n) => !n.read)) {
        await this.api.send('POST', '/api/v1/notifications/read', { up_to: list[0].id }, 'Done');
        this.api.summary.update((s) => (s ? { ...s, unread: 0 } : s));
      }
    } catch (e) {
      this.error.set(problem(e));
    }
  }

  when(seconds: number): string {
    return `${dayName(seconds)} ${new Date(seconds * 1000).toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' })}`;
  }
}
