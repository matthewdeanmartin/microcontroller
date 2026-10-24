import '@angular/compiler';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { MfClient } from 'miniframework-ng';
import type { Me, Summary } from 'party-core';
import { Api, ApiError, problem } from './api';

const me: Me = {
  id: 7, display_name: 'Alex', email: 'alex@example.com', role: 'member', bio: '',
  household: 0, household_name: '', discoverable: 'members', sessions: 1, created: 0,
  has_pin: true, identities: [], email_notify: false, dm_notify: false, personal_link: '',
};
const summary: Summary = { unread: 3, awaiting: 2, latest: 0 };

describe('API session and polling', () => {
  let api: Api;
  let visibility: { visibilityState: string; addEventListener: ReturnType<typeof vi.fn> };

  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(1_000_000);
    visibility = { visibilityState: 'visible', addEventListener: vi.fn() };
    vi.stubGlobal('document', visibility);
    api = new Api();
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
    vi.useRealTimers();
  });

  it('unwraps client responses and forwards request options', async () => {
    const get = vi.spyOn(MfClient.prototype, 'get').mockResolvedValue({ data: me } as never);
    const send = vi.spyOn(MfClient.prototype, 'send').mockResolvedValue({ data: me } as never);
    expect(await api.get('/me', 'Me')).toEqual(me);
    expect(get).toHaveBeenCalledWith('/me', { message: 'Me' });
    expect(await api.send('POST', '/me', { name: 'Alex' }, 'Me')).toEqual(me);
    expect(send).toHaveBeenCalledWith('POST', '/me', { name: 'Alex' }, { message: 'Me' });
  });

  it('does not poll while signed out or hidden', async () => {
    const get = vi.spyOn(api, 'get');
    await api.poll();
    api.me.set(me);
    visibility.visibilityState = 'hidden';
    await api.poll(true);
    expect(get).not.toHaveBeenCalled();
  });

  it('polls at most once a minute and accepts the exact boundary', async () => {
    api.me.set(me);
    const get = vi.spyOn(api, 'get').mockResolvedValue(summary);
    await api.poll();
    expect(api.summary()).toEqual(summary);
    vi.setSystemTime(1_059_999);
    await api.poll();
    expect(get).toHaveBeenCalledTimes(1);
    vi.setSystemTime(1_060_000);
    await api.poll();
    expect(get).toHaveBeenCalledTimes(2);
    expect(get).toHaveBeenLastCalledWith('/api/v1/me/summary', 'Summary');
  });

  it('refreshes on visibility changes with a ten-second cooldown', async () => {
    api.me.set(me);
    const get = vi.spyOn(api, 'get').mockResolvedValue(summary);
    await api.poll();
    const [event, listener] = visibility.addEventListener.mock.calls[0];
    expect(event).toBe('visibilitychange');
    vi.setSystemTime(1_009_999);
    listener();
    expect(get).toHaveBeenCalledTimes(1);
    vi.setSystemTime(1_010_000);
    listener();
    expect(get).toHaveBeenCalledTimes(2);
  });

  it('explicit refresh bypasses the polling cooldown', async () => {
    api.me.set(me);
    const get = vi.spyOn(api, 'get').mockResolvedValue(summary);
    await api.poll();
    await api.refreshSummary();
    expect(get).toHaveBeenCalledTimes(2);
  });

  it.each([401, 503])('handles polling failure %i without rejecting', async (status) => {
    api.me.set(me);
    vi.spyOn(api, 'get').mockRejectedValue(new ApiError(status, 'test', 'Failed'));
    await expect(api.poll()).resolves.toBeUndefined();
    expect(api.me()).toEqual(status === 401 ? null : me);
  });

  it('marks startup ready even when the board cannot be reached', async () => {
    const error = new TypeError('offline');
    vi.spyOn(api, 'get').mockRejectedValue(error);
    await expect(api.refresh()).rejects.toBe(error);
    expect(api.ready()).toBe(true);
  });

  it('loads the session and summary on successful startup', async () => {
    const board = { setup_needed: false };
    vi.spyOn(api, 'get').mockResolvedValueOnce(board).mockResolvedValueOnce(me)
      .mockResolvedValueOnce(summary);
    await api.refresh();
    expect(api.status()).toEqual(board);
    expect(api.me()).toEqual(me);
    expect(api.summary()).toEqual(summary);
    expect(api.ready()).toBe(true);
  });

  it.each([401, 500])('handles session lookup failure %i', async (status) => {
    api.me.set(me);
    const board = { setup_needed: false };
    const error = new ApiError(status, 'test', 'Failed');
    vi.spyOn(api, 'get').mockResolvedValueOnce(board).mockRejectedValueOnce(error);
    const refresh = api.refresh();
    if (status === 401) await expect(refresh).resolves.toBeUndefined();
    else await expect(refresh).rejects.toBe(error);
    expect(api.status()).toEqual(board);
    expect(api.ready()).toBe(true);
    expect(api.me()).toEqual(status === 401 ? null : me);
  });

  it.each([false, true])('clears the session after logout (everywhere=%s)', async (everywhere) => {
    api.me.set(me);
    const send = vi.spyOn(api, 'send').mockResolvedValue({});
    await api.signOut(everywhere);
    expect(send).toHaveBeenCalledWith('POST', `/api/v1/auth/logout${everywhere ? '?all=1' : ''}`, undefined, 'Done');
    expect(api.me()).toBeNull();
  });

  it('preserves the session when logout fails', async () => {
    api.me.set(me);
    vi.spyOn(api, 'send').mockRejectedValue(new Error('offline'));
    await expect(api.signOut()).rejects.toThrow('offline');
    expect(api.me()).toEqual(me);
  });

  it('redirects only after OAuth registration is ready', async () => {
    const assign = vi.fn();
    vi.stubGlobal('location', { assign });
    vi.spyOn(api, 'send').mockResolvedValue({ ticket: 'ticket-1' });
    const get = vi.spyOn(api, 'get').mockResolvedValueOnce({ state: 'pending' })
      .mockResolvedValueOnce({ state: 'redirect', location: 'https://example.com/auth' });
    const login = api.startMastodon(1, 'signin');
    await vi.advanceTimersByTimeAsync(500);
    await login;
    expect(get).toHaveBeenCalledWith('/api/v1/auth/oauth/ticket-1', 'OAuthStatus');
    expect(assign).toHaveBeenCalledExactlyOnceWith('https://example.com/auth');
  });

  it('surfaces an OAuth provider failure', async () => {
    vi.spyOn(api, 'send').mockResolvedValue({ ticket: 'ticket-1' });
    vi.spyOn(api, 'get').mockResolvedValue({ state: 'failed', message: 'Provider unavailable' });
    await expect(api.startMastodon(1, 'signin')).rejects.toMatchObject({ status: 502, message: 'Provider unavailable' });
  });

  it('times out OAuth after forty pending responses', async () => {
    vi.spyOn(api, 'send').mockResolvedValue({ ticket: 'ticket-1' });
    const get = vi.spyOn(api, 'get').mockResolvedValue({ state: 'pending' });
    const rejected = expect(api.startMastodon(1, 'signin')).rejects.toMatchObject({ status: 504 });
    await vi.advanceTimersByTimeAsync(20_000);
    await rejected;
    expect(get).toHaveBeenCalledTimes(40);
  });
});

it('explains API, network, and unexpected errors', () => {
  expect(problem(new ApiError(403, 'forbidden', 'Members only'))).toBe('Members only');
  expect(problem(new TypeError('fetch failed'))).toContain('home Wi-Fi');
  expect(problem('unexpected')).toBe('unexpected');
});
