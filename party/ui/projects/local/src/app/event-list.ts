import { Component, input } from '@angular/core';
import { RouterLink } from '@angular/router';
import { EventSummary, headline, KIND_INFO, STATUS_LABEL, when } from 'party-core';

/** A list of events: time, title, how full, and the viewer's answer. */
@Component({
  selector: 'party-event-list',
  imports: [RouterLink],
  template: `
    <ul class="events">
      @for (e of events(); track e.id) {
        <li [class.dim]="e.status === 'cancelled' || e.status === 'done'">
          <span class="when">{{ e.polling ? 'Time to be decided' : time(e) }}</span>
          <a [routerLink]="['/events', e.id]">{{ icon(e) }} {{ e.title }}</a>
          @if (showGroup()) { <span class="aside">{{ e.group_name }}</span> }
          <span class="aside">{{ headline(e.heads, e.capacity) }}</span>
          @if (e.status === 'cancelled') { <span class="status">cancelled</span> }
          @if (e.status === 'draft') { <span class="status">draft</span> }
          @if (e.my_status) { <span class="status" [class.on]="e.my_status === 'going'">{{ label(e) }}</span> }
        </li>
      } @empty {
        <li class="aside">{{ empty() }}</li>
      }
    </ul>
  `,
})
export class EventList {
  readonly events = input.required<EventSummary[]>();
  readonly showGroup = input(false);
  readonly empty = input('Nothing planned yet.');
  readonly headline = headline;

  time(e: EventSummary): string {
    return when(e.start, e.end);
  }

  icon(e: EventSummary): string {
    return KIND_INFO[e.kind]?.icon ?? '📅';
  }

  label(e: EventSummary): string {
    return e.my_status ? STATUS_LABEL[e.my_status] : '';
  }
}
