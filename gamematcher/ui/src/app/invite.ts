import { PlanView } from './api';
import { when } from './fmt';

/**
 * Invitations go out from the person's own mail app (a `mailto:` link), so
 * the board needs no mail server or password, and the email comes from
 * someone the guest knows. Phones that can share (HTTPS only) get the
 * share sheet too.
 */
export interface Invite {
  subject: string;
  body: string;
}

export function planInvite(plan: PlanView, origin: string, inviter: string): Invite {
  const title = plan.title || (plan.games.length === 1 ? plan.games[0].title : 'Game night');
  const chosen = plan.times.find((t) => t.id === plan.chosen_time);
  const times = chosen ? [chosen] : plan.times;
  const lines = [
    `${inviter} is inviting you to ${title}!`,
    '',
    plan.games.length > 1 ? `Games: ${plan.games.map((g) => g.title).join(', ')}` : `Game: ${plan.games[0]?.title ?? ''}`,
    times.length
      ? (chosen ? 'When: ' : 'When (pick what works): ') + times.map((t) => when(t.start, t.end)).join('; ')
      : 'When: to be decided',
    `Where: ${plan.place}`,
  ];
  if (plan.notes) lines.push('', plan.notes);
  lines.push(
    '',
    'Say if you can make it on Game Matcher, our home game planner:',
    `${origin}/plans/${plan.id}`,
    '',
    "It only works on our home Wi-Fi. No account yet? It takes a minute: pick any email as your sign-in name and a PIN.",
  );
  return { subject: `Game night invite: ${title}`, body: lines.join('\n') };
}

export function siteInvite(origin: string, inviter: string): Invite {
  return {
    subject: 'Join us on Game Matcher',
    body: [
      `${inviter} wants you on Game Matcher, our home game-night planner.`,
      '',
      `Open ${origin}/signup on our home Wi-Fi, pick any email as your sign-in name and a PIN, and you're in.`,
    ].join('\n'),
  };
}

/** A `mailto:` URL with no recipient: the person picks who in their mail app. */
export function mailto(invite: Invite): string {
  return `mailto:?subject=${encodeURIComponent(invite.subject)}&body=${encodeURIComponent(invite.body)}`;
}

export interface ShareTarget {
  id: string;
  label: string;
  href: string;
}

/**
 * "Intents": links that open a messaging or social app with the invite
 * already written (the pattern mawkingbird and NanaCoin use). Messaging
 * first: these invites are for people who will come to the house.
 * `mastodon` is the instance to post from (the person's own if linked).
 */
export function shareTargets(invite: Invite, mastodon = 'https://mastodon.social'): ShareTarget[] {
  const text = `${invite.subject}\n\n${invite.body}`;
  const q = (params: Record<string, string>) => new URLSearchParams(params).toString();
  const instance = mastodon.replace(/\/+$/, '');
  return [
    { id: 'email', label: '✉️ Email', href: mailto(invite) },
    // `sms:?&body=` is the form both iOS and Android accept.
    { id: 'sms', label: '💬 Text message', href: `sms:?&body=${encodeURIComponent(text)}` },
    { id: 'whatsapp', label: 'WhatsApp', href: `https://wa.me/?${q({ text })}` },
    { id: 'telegram', label: 'Telegram', href: `https://t.me/share/url?${q({ url: firstUrl(invite.body), text })}` },
    { id: 'mastodon', label: 'Mastodon', href: `${instance}/share?${q({ text })}` },
    { id: 'bluesky', label: 'Bluesky', href: `https://bsky.app/intent/compose?${q({ text })}` },
  ];
}

function firstUrl(text: string): string {
  return /https?:\/\/\S+/.exec(text)?.[0] ?? '';
}

/** The share sheet where the browser has one (secure contexts only). */
export function canShare(): boolean {
  return typeof navigator !== 'undefined' && typeof navigator.share === 'function';
}

export async function share(invite: Invite): Promise<void> {
  await navigator.share({ title: invite.subject, text: invite.body });
}
