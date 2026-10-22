import { Injectable, signal } from '@angular/core';

/**
 * Recovers the app after the board was updated while a tab was open
 * (ported from mawkingbird's `update-recovery.ts`).
 *
 * A new firmware build has new hashed chunk names. A tab still running the
 * old bundle then asks for a chunk the board no longer has (404), and the
 * browser reports "error loading dynamically imported module" or a variant.
 * Reloading once fetches the fresh `index.html` (served `no-cache`) and its
 * new chunk names.
 *
 * Conservative on purpose: at most one reload per recovery window. If the
 * reloaded app *also* fails (say, the board rebooted mid-update), stop and
 * let the person press "Try again" instead of looping.
 */

interface RecoveryAttempt {
  attemptedAt: number;
}

/** Message fragments (lowercased) that identify a failed chunk / module load. */
const CHUNK_ERROR_FRAGMENTS = [
  'error loading dynamically imported module',
  'failed to fetch dynamically imported module',
  'importing a module script failed',
  'chunkloaderror',
  'loading chunk',
  'failed to load module script',
];

@Injectable({ providedIn: 'root' })
export class UpdateRecovery {
  private readonly storageKey = 'party.update-recovery';
  private readonly recoveryWindowMs = 60_000;
  private readonly stabilizationPeriodMs = 30_000;
  private recoveryStarted = false;

  /** A reload has been triggered: show the "updating" overlay. */
  readonly updating = signal(false);
  /** Recovery already ran recently and failed again: show the panel. */
  readonly failed = signal(false);

  /**
   * Call once the app has started. The guard is cleared only after the app
   * has run cleanly for a while, because a half-updated board can let the
   * reloaded app start and then fail on the next chunk.
   */
  markApplicationStableAfterDelay(): void {
    globalThis.setTimeout(() => this.clearRecoveryAttempt(), this.stabilizationPeriodMs);
  }

  /** True when `error` looks like a failed JS module / lazy chunk load. */
  isChunkLoadError(error: unknown): boolean {
    const message = this.extractErrorText(error).toLowerCase();
    return (
      CHUNK_ERROR_FRAGMENTS.some((fragment) => message.includes(fragment)) ||
      (message.includes('module') && message.includes('mime type'))
    );
  }

  /**
   * Handles a suspected chunk-load failure. Returns true when it was one (the
   * caller should stop treating it as a crash). Reloads once; a repeat failure
   * within the window shows the "couldn't update" panel instead.
   * `destination`: the URL a failed navigation was heading for.
   */
  recover(error: unknown, destination?: string): boolean {
    if (!this.isChunkLoadError(error)) return false;
    // The same failure can arrive twice (an error and an unhandledrejection).
    if (this.recoveryStarted) return true;
    this.recoveryStarted = true;

    const previous = this.readRecoveryAttempt();
    const now = Date.now();
    if (previous !== null && now - previous.attemptedAt < this.recoveryWindowMs) {
      this.failed.set(true);
      return true;
    }
    this.writeRecoveryAttempt({ attemptedAt: now });
    this.updating.set(true);
    // Let the overlay paint before the browser navigates away. A failed
    // navigation never changed the URL, so go where the person was going.
    globalThis.setTimeout(() => {
      if (destination) globalThis.location.assign(destination);
      else globalThis.location.reload();
    }, 400);
    return true;
  }

  /** The person's explicit "Try again". */
  retry(): void {
    this.clearRecoveryAttempt();
    this.failed.set(false);
    this.updating.set(true);
    globalThis.location.reload();
  }

  private extractErrorText(error: unknown): string {
    if (typeof error === 'string') return error;
    if (error instanceof Error) return `${error.name}: ${error.message}\n${error.stack ?? ''}`;
    if (typeof error === 'object' && error !== null) {
      const e = error as Record<string, unknown>;
      const parts: string[] = [];
      if (typeof e['name'] === 'string') parts.push(e['name']);
      if (typeof e['message'] === 'string') parts.push(e['message']);
      // PromiseRejectionEvent, router NavigationError and Angular wrappers
      // keep the original elsewhere.
      for (const key of ['reason', 'rejection', 'error']) {
        if (key in e) parts.push(this.extractErrorText(e[key]));
      }
      if (parts.length) return parts.join('\n');
      try {
        return JSON.stringify(error);
      } catch {
        return String(error);
      }
    }
    return String(error);
  }

  private readRecoveryAttempt(): RecoveryAttempt | null {
    try {
      const parsed: unknown = JSON.parse(globalThis.sessionStorage.getItem(this.storageKey) ?? 'null');
      if (typeof parsed === 'object' && parsed !== null && typeof (parsed as RecoveryAttempt).attemptedAt === 'number') {
        return { attemptedAt: (parsed as RecoveryAttempt).attemptedAt };
      }
    } catch {
      // Storage can be unavailable (private modes); recovery still works,
      // it just can't persist the loop guard.
    }
    return null;
  }

  private writeRecoveryAttempt(attempt: RecoveryAttempt): void {
    try {
      globalThis.sessionStorage.setItem(this.storageKey, JSON.stringify(attempt));
    } catch {
      // Don't block recovery just because storage is unavailable.
    }
  }

  private clearRecoveryAttempt(): void {
    try {
      globalThis.sessionStorage.removeItem(this.storageKey);
    } catch {
      // Nothing else to do.
    }
  }
}
