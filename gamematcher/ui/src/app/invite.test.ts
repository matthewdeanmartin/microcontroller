import { describe, expect, it } from 'vitest';
import { PlanView } from './api';
import { mailto, planInvite, shareTargets, siteInvite } from './invite';

const plan: PlanView = {
  id: 7,
  host: 1,
  host_name: 'Dad',
  title: 'Saturday game night',
  notes: 'Bring snacks & drinks',
  place: 'Home',
  seats: 0,
  status: 'open',
  games: [
    { id: 1, title: 'Catan', kind: 'board', min_players: 3, max_players: 4, minutes: 90, archived: false, ticks: [] },
    { id: 2, title: 'Wingspan', kind: 'board', min_players: 1, max_players: 5, minutes: 60, archived: false, ticks: [] },
  ],
  times: [{ id: 1, start: 1_793_000_000, end: 0, yes: [], if_needed: [], no: [] }],
  responses: [],
  chosen_time: null,
  chosen_games: [],
  created: 0,
  updated: 0,
  manageable: true,
};

describe('invites', () => {
  it('writes a plan invite with the link and the Wi-Fi caveat', () => {
    const invite = planInvite(plan, 'http://gamematcher.local', 'Dad');
    expect(invite.subject).toBe('Game night invite: Saturday game night');
    expect(invite.body).toContain('Games: Catan, Wingspan');
    expect(invite.body).toContain('http://gamematcher.local/plans/7');
    expect(invite.body).toContain('home Wi-Fi');
    expect(invite.body).toContain('Bring snacks & drinks');
  });

  it('encodes a mailto with no recipient', () => {
    const url = mailto(siteInvite('http://192.168.1.163', 'Mom'));
    expect(url.startsWith('mailto:?subject=Join%20us%20on%20Game%20Matcher&body=')).toBe(true);
    expect(decodeURIComponent(url.split('body=')[1])).toContain('http://192.168.1.163/signup');
  });

  it('builds an intent per app, text prefilled', () => {
    const targets = shareTargets(planInvite(plan, 'http://gamematcher.local', 'Dad'), 'https://mastomini.local/');
    const by = Object.fromEntries(targets.map((t) => [t.id, t.href]));
    expect(by['sms']).toMatch(/^sms:\?&body=Game%20night%20invite/);
    expect(by['whatsapp']).toMatch(/^https:\/\/wa\.me\/\?text=/);
    expect(by['telegram']).toContain('url=http%3A%2F%2Fgamematcher.local%2Fplans%2F7');
    expect(by['mastodon']).toMatch(/^https:\/\/mastomini\.local\/share\?text=/);
    expect(by['bluesky']).toMatch(/^https:\/\/bsky\.app\/intent\/compose\?text=/);
  });
});
