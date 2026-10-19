import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { UpdateRecovery } from './update-recovery';

/** Chunk-error detection and the reload-loop guard. Reload is stubbed. */
describe('UpdateRecovery', () => {
  let reload: ReturnType<typeof vi.fn>;
  let assign: ReturnType<typeof vi.fn>;
  let store: Map<string, string>;

  beforeEach(() => {
    vi.useFakeTimers();
    reload = vi.fn();
    assign = vi.fn();
    store = new Map();
    vi.stubGlobal('location', { reload, assign });
    vi.stubGlobal('sessionStorage', {
      getItem: (k: string) => store.get(k) ?? null,
      setItem: (k: string, v: string) => store.set(k, v),
      removeItem: (k: string) => store.delete(k),
    });
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it('recognizes chunk failures from Firefox, Chromium, Safari and wrappers', () => {
    const r = new UpdateRecovery();
    expect(
      r.isChunkLoadError(new TypeError('error loading dynamically imported module: http://gamematcher.local/chunk-CPL1TWv_.js')),
    ).toBe(true);
    expect(r.isChunkLoadError(new Error('Failed to fetch dynamically imported module: x.js'))).toBe(true);
    expect(r.isChunkLoadError({ reason: new Error('Importing a module script failed.') })).toBe(true);
    expect(r.isChunkLoadError({ error: { name: 'ChunkLoadError', message: 'Loading chunk 5 failed' } })).toBe(true);
    expect(r.isChunkLoadError(new Error('Cannot read properties of undefined'))).toBe(false);
  });

  it('reloads once, then stops and asks', () => {
    const first = new UpdateRecovery();
    expect(first.recover(new TypeError('error loading dynamically imported module'))).toBe(true);
    expect(first.updating()).toBe(true);
    vi.advanceTimersByTime(500);
    expect(reload).toHaveBeenCalledTimes(1);

    // The "reloaded" app fails again within the window: no second reload.
    const second = new UpdateRecovery();
    expect(second.recover(new TypeError('error loading dynamically imported module'))).toBe(true);
    vi.advanceTimersByTime(500);
    expect(reload).toHaveBeenCalledTimes(1);
    expect(second.failed()).toBe(true);

    second.retry();
    expect(reload).toHaveBeenCalledTimes(2);
  });

  it('a stable app clears the guard', () => {
    const r = new UpdateRecovery();
    r.recover(new TypeError('error loading dynamically imported module'));
    expect(store.size).toBe(1);
    r.markApplicationStableAfterDelay();
    vi.advanceTimersByTime(30_000);
    expect(store.size).toBe(0);
  });

  it('goes where a failed navigation was going', () => {
    const r = new UpdateRecovery();
    r.recover(new TypeError('error loading dynamically imported module'), '/games');
    vi.advanceTimersByTime(500);
    expect(assign).toHaveBeenCalledWith('/games');
    expect(reload).not.toHaveBeenCalled();
  });

  it('ignores ordinary errors', () => {
    const r = new UpdateRecovery();
    expect(r.recover(new Error('boom'))).toBe(false);
    vi.advanceTimersByTime(500);
    expect(reload).not.toHaveBeenCalled();
  });
});
