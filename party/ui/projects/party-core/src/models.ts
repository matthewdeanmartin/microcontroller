// Shapes of the board's messages (api/src/messages.rs), shared by the local
// app and the companion site. Clients decode any wire format with the
// server's schema; the UI asks for JSON.

export interface Status {
  app: string;
  version: string;
  build: string;
  setup_needed: boolean;
  signup_open: boolean;
  clock_synced: boolean;
  setup_code_required: boolean;
  secure: boolean;
  instance: string;
  providers: { id: number; name: string }[];
  /** The board can send email ("forgot PIN?"). */
  mail: boolean;
  /** The bot's handle, "" = none. */
  bot: string;
}

export type Discoverable = 'nobody' | 'members' | 'public';

export interface Me {
  id: number;
  display_name: string;
  email: string;
  role: 'admin' | 'member';
  bio: string;
  household: number;
  household_name: string;
  discoverable: Discoverable;
  sessions: number;
  created: number;
  has_pin: boolean;
  /** Linked Mastodon accounts. */
  identities: string[];
  email_notify: boolean;
  dm_notify: boolean;
  /** The companion site's personal link ("" until made, or no site yet). */
  personal_link: string;
}

export interface PersonView {
  id: number;
  display_name: string;
  bio: string;
  household: number;
  household_name: string;
}

export interface PersonList {
  people: PersonView[];
  next: number;
}

export interface AdminPerson {
  id: number;
  display_name: string;
  email: string;
  role: 'admin' | 'member';
  status: 'active' | 'disabled' | 'invited';
  household: number;
  has_pin: boolean;
  sessions: number;
  created: number;
  last_seen: number;
  locked_secs: number;
}

export interface Settings {
  signup_open: boolean;
  groups_admin_only: boolean;
}

export interface StorageInfo {
  records: number;
  last_commit_bytes: number;
  max_commit_bytes: number;
  commits: number;
  people: number;
  max_people: number;
  sessions: number;
  max_sessions: number;
  groups: number;
  max_groups: number;
  households: number;
  max_households: number;
  quarantined: string[];
}

export interface AuditEntry {
  seq: number;
  at: number;
  actor: number;
  action: string;
  detail: string;
}

export interface HouseholdView {
  id: number;
  name: string;
  members: PersonView[];
}

export type GroupKind = 'club' | 'circle' | 'one-off';
export type Visibility = 'public' | 'unlisted' | 'private';
export type JoinPolicy = 'open' | 'approval' | 'invite';
export type GroupRole = 'organizer' | 'co-organizer' | 'member';
export type MemberStatus = 'active' | 'requested' | 'removed';

export interface GroupView {
  id: number;
  slug: string;
  name: string;
  about: string;
  kind: GroupKind;
  visibility: Visibility;
  join: JoinPolicy;
  status: 'active' | 'archived';
  members: number;
  my_role: GroupRole | '';
  my_status: MemberStatus | '';
  can_manage: boolean;
  can_own: boolean;
  created: number;
  rev: number;
  requests: number;
  members_create_events: boolean;
  can_create_events: boolean;
}

export interface GroupList {
  groups: GroupView[];
  next: number;
}

export interface GroupInput {
  name?: string;
  about?: string;
  kind?: GroupKind;
  visibility?: Visibility;
  join?: JoinPolicy;
  rev?: number;
  status?: 'active' | 'archived';
  members_create_events?: boolean;
}

export interface MemberView {
  person: number;
  display_name: string;
  role: GroupRole;
  status: MemberStatus;
  joined: number;
}

export interface InviteView {
  id: number;
  target_kind: 'group' | 'event';
  target: number;
  target_name: string;
  issuer_name: string;
  email: string;
  handle: string;
  person: number;
  expires: number;
  uses_left: number;
  status: 'open' | 'used' | 'revoked' | 'expired';
  token: string;
  link: string;
  emailed: boolean;
  messaged: boolean;
}

export interface InvitePreview {
  target_kind: 'group' | 'event';
  target: number;
  target_name: string;
  issuer_name: string;
  valid: boolean;
  reason: string;
  providers: { id: number; name: string }[];
}

export interface MailView { state: string; sender_email: string; sender_name: string; key_hint: string; error: string; sent_today: number; daily_cap: number }
export interface BotView { state: string; instance: string; handle: string; commands: boolean; poll_secs: number; last_poll: number; error: string; token_hint: string }
export interface PublishView {
  state: string; repo: string; branch: string; site_url: string; every_minutes: number; last_published: number;
  generation: number; files: number; last_error: string; token_hint: string; last_commit: string;
}
export interface JobView { id: number; kind: string; to: string; subject: string; state: string; attempts: number; next_at: number; error: string; created: number }
export interface InboxEntry { at: number; from: string; text: string; result: string }
export interface Provider { id: number; name: string; base: string; trust: string }
