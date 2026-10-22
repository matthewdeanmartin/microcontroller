import { Component, inject, OnInit, signal } from '@angular/core';
import { RouterLink } from '@angular/router';
import { day, NOTICE_TEXT, NoticeView } from 'party-core';
import { Api, problem } from '../api';

/** The bell: newest first; opening the page marks everything read. */
@Component({
  selector: 'party-notifications',
  imports: [RouterLink],
  template: `
    <h1>Notifications</h1>
    @if (error()) { <p class="error card" role="alert">{{ error() }}</p> }
    <section class="card">
      <ul class="events">
        @for (n of notices(); track n.id) {
          <li [class.unread]="!n.read">
            <span class="when">{{ day(n.created) }}</span>
            <span>
              @if (n.actor_name) { <strong>{{ n.actor_name }}</strong> }
              {{ text(n) }}
              @if (n.event) { <a [routerLink]="['/events', n.event]">{{ n.event_title || 'an event' }}</a> }
              @else if (n.group) { <a [routerLink]="['/groups', n.group]">{{ n.group_name }}</a> }
              @if (n.count > 1) { <small>×{{ n.count }}</small> }
            </span>
          </li>
        } @empty {
          <li class="aside">Nothing yet.</li>
        }
      </ul>
    </section>
  `,
})
export class Notifications implements OnInit {
  private readonly api = inject(Api);
  readonly notices = signal<NoticeView[]>([]);
  readonly error = signal('');
  readonly day = day;

  text(n: NoticeView): string {
    return NOTICE_TEXT[n.kind] ?? n.kind;
  }

  async ngOnInit(): Promise<void> {
    try {
      const list = await this.api.get<{ notices: NoticeView[] }>('/api/v1/notifications', 'NoticeList');
      this.notices.set(list.notices);
      if (list.notices.some((n) => !n.read)) {
        await this.api.send('POST', '/api/v1/notifications/read', { up_to: 0 }, 'Done');
        void this.api.refreshSummary();
      }
    } catch (e) {
      this.error.set(problem(e));
    }
  }
}
