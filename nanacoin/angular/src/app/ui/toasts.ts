// Transient messages. One service so that any page can report an outcome
// without owning a banner.

import { Injectable, signal } from '@angular/core';

import { ApiError } from '../api/nanacoin.service';

export interface Toast {
  id: number;
  text: string;
  kind: 'ok' | 'error';
}

@Injectable({ providedIn: 'root' })
export class Toasts {
  readonly items = signal<Toast[]>([]);
  private nextId = 1;

  ok(text: string) {
    this.push(text, 'ok');
  }

  error(text: string) {
    this.push(text, 'error');
  }

  /**
   * Reports a thrown value. An ApiError already carries a sentence written for
   * a person, so it is shown as-is; anything else gets a generic line, because
   * a stack trace is not useful to whoever is trying to pay their sibling.
   */
  fromError(e: unknown) {
    if (e instanceof ApiError) this.push(e.message, 'error');
    else this.push('Something went wrong.', 'error');
  }

  dismiss(id: number) {
    this.items.update((list) => list.filter((t) => t.id !== id));
  }

  private push(text: string, kind: Toast['kind']) {
    const id = this.nextId++;
    this.items.update((list) => [...list, { id, text, kind }]);
    // Errors linger, because they usually need reading; confirmations do not.
    window.setTimeout(() => this.dismiss(id), kind === 'error' ? 8000 : 4000);
  }
}
