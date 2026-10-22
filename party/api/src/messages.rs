//! Every message party sends, accepts or stores. Tags are protobuf field
//! numbers and CBOR integer keys: add fields with new numbers, never
//! renumber. Stored messages (`Stored*`) are the flash records' schema
//! (`spec/07-storage-limits.md`).
use miniframework::message;

// ---- API: board, accounts ---------------------------------------------------

message! {
    /// `GET /api/v1/status` (no sign-in needed).
    pub struct Status {
        1 app: String,
        2 version: String,
        3 build: String,
        /// No account exists yet: the first visitor sets the board up.
        4 setup_needed: bool,
        5 signup_open: bool,
        6 clock_synced: bool,
        7 setup_code_required: bool,
        8 secure: bool,
        /// The instance's name ("party").
        9 instance: String,
        /// Mastodon instances to sign in with.
        10 providers: Vec<ProviderButton>,
        /// The board can send email ("forgot PIN?").
        11 mail: bool,
        /// The bot's handle ("" = no bot).
        12 bot: String,
    }
}

message! {
    /// `POST /api/v1/setup`, `POST /api/v1/auth/signup`.
    pub struct NewAccount {
        1 display_name: String,
        /// Optional: party works for people without email.
        2 email: String,
        3 pin: String,
        4 setup_code: String,
        /// An invitation token: sign up with it even when sign-up is closed.
        5 invite: String,
    }
}

message! {
    /// `POST /api/v1/auth/pin`. `login` is an email or a display name.
    pub struct PinSignIn {
        1 login: String,
        2 pin: String,
    }
}

message! {
    /// `GET /api/v1/auth/me`: who is signed in.
    pub struct Me {
        1 id: u32,
        2 display_name: String,
        3 email: String,
        /// `admin` or `member` (instance role).
        4 role: String,
        5 bio: String,
        6 household: u32,
        7 household_name: String,
        /// `nobody`, `members` or `public`.
        8 discoverable: String,
        9 sessions: u32,
        10 created: u64,
        11 has_pin: bool,
        /// Linked Mastodon accounts (`@user@host`).
        12 identities: Vec<String>,
        13 email_notify: bool,
        14 dm_notify: bool,
        /// The personal link for the companion site ("" until it's set up).
        15 personal_link: String,
    }
}

message! {
    /// `PATCH /api/v1/me`.
    pub struct MeUpdate {
        1 display_name: Option<String>,
        /// "" removes it.
        2 email: Option<String>,
        3 bio: Option<String>,
        4 discoverable: Option<String>,
        5 email_notify: Option<bool>,
        6 dm_notify: Option<bool>,
    }
}

message! {
    /// `PUT /api/v1/me/pin`.
    pub struct PinChange {
        1 current: String,
        2 pin: String,
    }
}

message! {
    /// Someone as another person may see them.
    pub struct PersonView {
        1 id: u32,
        2 display_name: String,
        3 bio: String,
        4 household: u32,
        5 household_name: String,
    }
}

message! {
    /// `GET /api/v1/people`: a page; `next` is the `after` for the next one (0 = end).
    pub struct PersonList {
        1 people: Vec<PersonView>,
        2 next: u32,
    }
}

message! {
    /// One account on the admin page.
    pub struct AdminPerson {
        1 id: u32,
        2 display_name: String,
        3 email: String,
        4 role: String,
        /// `active`, `disabled` or `invited`.
        5 status: String,
        6 household: u32,
        7 has_pin: bool,
        8 sessions: u32,
        9 created: u64,
        10 last_seen: u64,
        /// Seconds until a PIN lockout ends (0 = none).
        11 locked_secs: u32,
    }
}

message! {
    pub struct AdminPersonList {
        1 people: Vec<AdminPerson>,
        2 next: u32,
    }
}

message! {
    /// `PATCH /api/v1/admin/people/{id}`: only the fields given change.
    pub struct AdminPersonUpdate {
        1 role: Option<String>,
        2 disabled: Option<bool>,
        /// A temporary PIN the admin tells them.
        3 pin: Option<String>,
        4 display_name: Option<String>,
        5 sign_out: Option<bool>,
        /// Household id, 0 = none.
        6 household: Option<u32>,
    }
}

message! {
    /// `GET/PATCH /api/v1/admin/settings`.
    pub struct Settings {
        1 signup_open: bool,
        /// Only admins may create groups.
        2 groups_admin_only: bool,
    }
}

message! {
    pub struct SettingsUpdate {
        1 signup_open: Option<bool>,
        2 groups_admin_only: Option<bool>,
    }
}

message! {
    /// `GET /api/v1/admin/storage`.
    pub struct StorageInfo {
        /// Records on flash (rows, sessions, meta, indexes).
        1 records: u32,
        /// Bytes the last commit wrote (journal included).
        2 last_commit_bytes: u32,
        /// Largest commit since boot.
        3 max_commit_bytes: u32,
        4 commits: u64,
        5 people: u32,
        6 max_people: u32,
        7 sessions: u32,
        8 max_sessions: u32,
        9 groups: u32,
        10 max_groups: u32,
        11 households: u32,
        12 max_households: u32,
        /// Records that failed to load (left on flash, not served).
        13 quarantined: Vec<String>,
    }
}

message! {
    /// `GET /api/v1/admin/audit`: newest first.
    pub struct AuditList {
        1 entries: Vec<StoredAuditEntry>,
    }
}

message! {
    /// A body-less success.
    pub struct Done {
        1 ok: bool,
    }
}

// ---- API: households ----------------------------------------------------------

message! {
    pub struct HouseholdView {
        1 id: u32,
        2 name: String,
        3 members: Vec<PersonView>,
    }
}

message! {
    pub struct HouseholdList {
        1 households: Vec<HouseholdView>,
    }
}

message! {
    /// `POST /api/v1/households`, `PATCH /api/v1/households/{id}`.
    pub struct HouseholdInput {
        1 name: String,
    }
}

// ---- API: groups and membership -------------------------------------------------

message! {
    /// `POST /api/v1/groups` (name required), `PATCH /api/v1/groups/{id}`
    /// (only the fields given change; `rev` guards against stale edits).
    pub struct GroupInput {
        1 name: Option<String>,
        2 about: Option<String>,
        /// `club`, `circle` or `one-off`.
        3 kind: Option<String>,
        /// `public`, `unlisted` or `private`.
        4 visibility: Option<String>,
        /// `open`, `approval` or `invite`.
        5 join: Option<String>,
        6 rev: Option<u32>,
        /// `active` or `archived` (organizers only).
        7 status: Option<String>,
        /// Plain members may create events.
        8 members_create_events: Option<bool>,
    }
}

message! {
    /// A group as the caller may see it, with what they may do.
    pub struct GroupView {
        1 id: u32,
        2 slug: String,
        3 name: String,
        4 about: String,
        5 kind: String,
        6 visibility: String,
        7 join: String,
        8 status: String,
        /// Active members.
        9 members: u32,
        /// The caller's role and membership status ("" = not a member).
        10 my_role: String,
        11 my_status: String,
        /// May edit the group and manage members (organizer, co-organizer, admin).
        12 can_manage: bool,
        /// May archive, delete, transfer and appoint organizers (organizer, admin).
        13 can_own: bool,
        14 created: u64,
        15 rev: u32,
        /// Join requests waiting (managers only, else 0).
        16 requests: u32,
        17 members_create_events: bool,
        /// The caller may create events here.
        18 can_create_events: bool,
    }
}

message! {
    pub struct GroupList {
        1 groups: Vec<GroupView>,
        2 next: u32,
    }
}

message! {
    pub struct MemberView {
        1 person: u32,
        2 display_name: String,
        /// `organizer`, `co-organizer` or `member`.
        3 role: String,
        /// `active`, `requested`, `left` or `removed`.
        4 status: String,
        5 joined: u64,
    }
}

message! {
    pub struct MemberList {
        1 members: Vec<MemberView>,
    }
}

message! {
    /// `PUT /api/v1/groups/{id}/members/{person}`: approve a request
    /// (`status: active`), remove someone (`removed`), or change a role.
    /// Adds the person directly if they aren't a member (managers only).
    pub struct MemberUpdate {
        1 role: Option<String>,
        2 status: Option<String>,
    }
}

message! {
    /// `POST /api/v1/groups/{id}/transfer`: the target becomes organizer,
    /// the caller co-organizer.
    pub struct Transfer {
        1 to: u32,
    }
}

// ---- API: events and RSVPs (Sprints 3-5) ----------------------------------------

message! {
    /// `POST /api/v1/groups/{g}/events` and `PATCH /api/v1/events/{id}`: only
    /// the fields given change. A list given replaces the whole list; items
    /// with id 0 are new. `rev` guards against stale edits.
    pub struct EventInput {
        /// A preset (`generic`, `party`, `dinner`, `potluck`, `outing`,
        /// `meeting`, `workday`, `poll`): sets modules and defaults on create.
        1 kind: Option<String>,
        2 title: Option<String>,
        3 about: Option<String>,
        4 hosts: Option<Vec<u32>>,
        5 start: Option<u64>,
        6 end: Option<u64>,
        /// `place`, `home` or `tbd`.
        7 where_kind: Option<String>,
        8 place: Option<String>,
        9 address: Option<String>,
        10 map: Option<String>,
        /// Whose home (a household id), for `where_kind: home`.
        11 home: Option<u32>,
        /// `group`, `invite-only` or `public`.
        12 visibility: Option<String>,
        13 invitees: Option<Vec<u32>>,
        /// Heads, 0 = no limit.
        14 capacity: Option<u32>,
        /// Answers lock at this time (0 = never).
        15 rsvp_by: Option<u64>,
        /// `none`, `limited` or `unlimited`.
        16 plus_ones: Option<String>,
        17 plus_max: Option<u32>,
        /// `draft` or `scheduled` (cancel with `POST .../cancel`).
        18 status: Option<String>,
        /// Enabled modules (see `events::MODULES`).
        19 modules: Option<Vec<String>>,
        20 bring: Option<Vec<StoredSlot>>,
        21 diet_prompt: Option<String>,
        22 questions: Option<Vec<StoredQuestion>>,
        23 cost: Option<String>,
        24 rides_note: Option<String>,
        25 when: Option<Vec<StoredOption>>,
        26 shifts: Option<Vec<StoredShift>>,
        27 link: Option<String>,
        /// Reminder this long before the start (0 = none).
        28 reminder_secs: Option<u64>,
        29 rev: Option<u32>,
    }
}

message! {
    /// One event as the caller may see it.
    pub struct EventView {
        1 event: StoredEvent,
        2 group_name: String,
        3 group_slug: String,
        4 host_names: Vec<String>,
        /// Heads going (people + their guests).
        5 heads: u32,
        6 going: u32,
        7 maybe: u32,
        8 no: u32,
        9 waitlist: u32,
        /// The caller's own answer, if any.
        10 mine: Option<RsvpView>,
        /// May edit, cancel and see everyone's details (hosts, organizers, admins).
        11 can_edit: bool,
        /// May answer for themselves now (not locked, not cancelled).
        12 can_answer: bool,
        /// People the caller may answer for (household; everyone invited, for hosts).
        13 answer_for: Vec<PersonView>,
        /// How many have claimed each bring-list slot, by slot id.
        14 claimed: Vec<SlotCount>,
        /// Yes-votes per time option (`when`), by option id.
        15 votes: Vec<SlotCount>,
        /// Takers per shift, by shift id.
        16 taken: Vec<SlotCount>,
        /// Answers locked (past `rsvp_by`, or the event is over or cancelled).
        17 locked: bool,
    }
}

message! {
    pub struct SlotCount {
        1 id: u32,
        2 count: u32,
        3 names: Vec<String>,
    }
}

message! {
    pub struct EventSummary {
        1 id: u32,
        2 group: u32,
        3 group_name: String,
        4 title: String,
        5 kind: String,
        6 start: u64,
        7 end: u64,
        8 status: String,
        9 heads: u32,
        10 capacity: u32,
        /// The caller's answer status ("" = none).
        11 my_status: String,
        12 series: u32,
        /// A time poll that hasn't been locked in yet.
        13 polling: bool,
    }
}

message! {
    pub struct EventList {
        1 events: Vec<EventSummary>,
        2 next: u32,
    }
}

message! {
    /// One person's answer, as the viewer may see it (`diet` and question
    /// answers only for the person and the hosts).
    pub struct RsvpView {
        1 person: u32,
        2 display_name: String,
        /// `going`, `maybe`, `no` or `waitlist`.
        3 status: String,
        4 plus: u32,
        5 kids: u32,
        6 names: String,
        7 bring: Vec<u32>,
        8 bring_other: String,
        9 diet: String,
        10 answers: Vec<StoredAnswer>,
        /// `need`, `offer`, `own` or "".
        11 ride: String,
        12 seats: u32,
        13 when: Vec<u32>,
        14 shifts: Vec<u32>,
        15 note: String,
        /// Who answered, if not the person ("answered for you by Sam").
        16 by_name: String,
        /// `web`, `household`, `host`.
        17 via: String,
        18 updated: u64,
        /// More guests than the event now allows (the policy changed since).
        19 over_policy: bool,
    }
}

message! {
    pub struct RsvpList {
        1 rsvps: Vec<RsvpView>,
    }
}

message! {
    /// `PUT /api/v1/events/{id}/rsvps/me` (or `/rsvps/{person}`, answering
    /// for someone). Module fields left out keep their previous answer.
    pub struct RsvpInput {
        /// `going`, `maybe` or `no`.
        1 status: String,
        2 plus: Option<u32>,
        3 kids: Option<u32>,
        4 names: Option<String>,
        5 bring: Option<Vec<u32>>,
        6 bring_other: Option<String>,
        7 diet: Option<String>,
        8 answers: Option<Vec<StoredAnswer>>,
        9 ride: Option<String>,
        10 seats: Option<u32>,
        11 when: Option<Vec<u32>>,
        12 shifts: Option<Vec<u32>>,
        13 note: Option<String>,
        /// The event `rev` the form was filled in against.
        14 rev_seen: u32,
    }
}

message! {
    /// `GET /api/v1/events/{id}/rides` (hosts): who needs seats and who offers them.
    pub struct RideBoard {
        1 needs: Vec<RideEntry>,
        2 offers: Vec<RideEntry>,
        /// Seats offered minus seats needed.
        3 spare: i32,
    }
}

message! {
    pub struct RideEntry {
        1 person: u32,
        2 display_name: String,
        3 seats: u32,
    }
}

message! {
    /// `POST /api/v1/events/{id}/lock`: pick the winning time of a poll.
    pub struct LockIn {
        1 option: u32,
    }
}

// ---- API: series, notifications, calendars ---------------------------------------

message! {
    /// `POST /api/v1/groups/{g}/series`: an `EventInput` for the first
    /// occurrence plus the rule. `PATCH /api/v1/series/{id}` changes the
    /// rule's end or window only; edit occurrences with `scope=following`.
    pub struct SeriesInput {
        1 event: Option<EventInput>,
        /// `weekly` (every `every` weeks on the first one's weekday),
        /// `monthly-nth` (the `nth` weekday of the month; -1 = last),
        /// `monthly-date` (the same date each month).
        2 rule: Option<String>,
        3 every: Option<u32>,
        4 nth: Option<i32>,
        /// Stop after this time (0 = open-ended)â€¦
        5 until: Option<u64>,
        /// â€¦or after this many occurrences (0 = open-ended).
        6 count: Option<u32>,
        /// Occurrences kept ahead (1-8, default 4).
        7 window: Option<u32>,
    }
}

message! {
    pub struct SeriesView {
        1 id: u32,
        2 group: u32,
        3 rule: String,
        4 every: u32,
        5 nth: i32,
        6 until: u64,
        7 count: u32,
        8 window: u32,
        9 status: String,
        /// "Every 2nd Friday at 19:00" in the instance zone.
        10 summary: String,
        11 regular: bool,
        12 regulars: u32,
        13 upcoming: Vec<EventSummary>,
        14 can_edit: bool,
    }
}

message! {
    /// `PUT /api/v1/series/{id}/regular`.
    pub struct Regular {
        1 on: bool,
    }
}

message! {
    pub struct NoticeView {
        1 id: u32,
        /// `invited`, `changed`, `cancelled`, `promoted`, `reminder`,
        /// `request`, `answered_for`.
        2 kind: String,
        3 event: u32,
        4 event_title: String,
        5 group: u32,
        6 group_name: String,
        7 actor_name: String,
        8 created: u64,
        9 read: bool,
        /// How many times it happened while unread (coalesced).
        10 count: u32,
    }
}

message! {
    pub struct NoticeList {
        1 notices: Vec<NoticeView>,
    }
}

message! {
    /// `POST /api/v1/notifications/read`: up to and including `up_to` (0 = all).
    pub struct MarkRead {
        1 up_to: u32,
    }
}

message! {
    /// `GET /api/v1/me/summary` (the bell, polled).
    pub struct Summary {
        1 unread: u32,
        /// Upcoming events I can answer but haven't.
        2 awaiting: u32,
        3 latest: u32,
    }
}

message! {
    /// `POST /api/v1/me/calendar`: a new personal feed URL (the old one stops).
    pub struct FeedUrl {
        1 path: String,
    }
}

message! {
    /// `GET/PATCH /api/v1/admin/zone`.
    pub struct ZoneSetting {
        /// POSIX TZ, e.g. `EST5EDT,M3.2.0,M11.1.0`.
        1 zone: String,
        /// The offset from UTC now, in seconds.
        2 offset_secs: i32,
    }
}

// ---- API: invitations, sign-in, channels (Sprints 6-8) -----------------------------

message! {
    /// `POST /api/v1/invitations`.
    pub struct InviteInput {
        /// `group` or `event`.
        1 target_kind: String,
        2 target: u32,
        /// Who it's for (any of them, or none for a shareable link).
        3 email: String,
        4 handle: String,
        /// An existing person (e.g. an email-only relative an organizer made).
        5 person: u32,
        /// Valid this many days (default 14, at most 90).
        6 days: u32,
        /// How many people may use it (default 1, at most 50).
        7 uses: u32,
    }
}

message! {
    pub struct InviteView {
        1 id: u32,
        2 target_kind: String,
        3 target: u32,
        4 target_name: String,
        5 issuer_name: String,
        6 email: String,
        7 handle: String,
        8 person: u32,
        9 expires: u64,
        10 uses_left: u32,
        /// `open`, `used`, `revoked` or `expired`.
        11 status: String,
        /// Only right after creating it: the token and its link.
        12 token: String,
        13 link: String,
        /// The invitation went out by email / DM (queued).
        14 emailed: bool,
        15 messaged: bool,
    }
}

message! {
    pub struct InviteList {
        1 invites: Vec<InviteView>,
    }
}

message! {
    /// `GET /api/v1/invitations/{token}`: what it's for, before accepting.
    pub struct InvitePreview {
        1 target_kind: String,
        2 target: u32,
        3 target_name: String,
        4 issuer_name: String,
        5 valid: bool,
        /// Why not, when not valid.
        6 reason: String,
        /// Mastodon instances one may accept with.
        7 providers: Vec<ProviderButton>,
    }
}

message! {
    /// `POST /api/v1/people`: an organizer adds someone without a PIN
    /// (an email-only relative). They're `invited` until they sign in.
    pub struct NewPerson {
        1 display_name: String,
        2 email: String,
        3 household: u32,
    }
}

message! {
    pub struct ProviderButton {
        1 id: u32,
        2 name: String,
    }
}

message! {
    /// `POST /api/v1/admin/providers` (and `PATCH .../{id}`).
    pub struct ProviderInput {
        1 name: String,
        /// `https://mastodon.social`.
        2 base: String,
        /// `public` or `household` (the instance's certificate is from the household CA).
        3 trust: String,
    }
}

message! {
    pub struct ProviderList {
        1 providers: Vec<StoredProvider>,
    }
}

message! {
    /// `POST /api/v1/auth/oauth/start`.
    pub struct OAuthStart {
        1 provider: u32,
        /// `signin`, `link` (signed in) or `redeem` (with `token`).
        2 purpose: String,
        3 token: String,
    }
}

message! {
    pub struct OAuthTicket {
        1 ticket: String,
    }
}

message! {
    /// `GET /api/v1/auth/oauth/{ticket}`.
    pub struct OAuthStatus {
        /// `waiting`, `redirect`, `done` or `failed`.
        1 state: String,
        2 location: String,
        3 message: String,
    }
}

message! {
    /// `POST /api/v1/auth/pin-reset`.
    pub struct PinResetStart {
        1 login: String,
    }
}

message! {
    pub struct PinResetConfirm {
        1 login: String,
        2 code: String,
        3 pin: String,
    }
}

message! {
    /// `PUT /api/v1/admin/mail`: a Brevo API key (`xkeysib-…`), checked on the worker.
    pub struct MailSetup {
        1 api_key: String,
        /// A verified Brevo sender; "" = the first one.
        2 sender_email: String,
        3 sender_name: String,
    }
}

message! {
    pub struct MailView {
        /// `off`, `checking`, `ready` or `failed`.
        1 state: String,
        2 sender_email: String,
        3 sender_name: String,
        /// The key's last four characters.
        4 key_hint: String,
        5 error: String,
        6 sent_today: u32,
        7 daily_cap: u32,
    }
}

message! {
    /// `PUT /api/v1/admin/bot`: the bot account's access token, checked on the worker.
    pub struct BotSetup {
        1 instance: String,
        2 token: String,
        /// `public` or `household`.
        3 trust: String,
        /// Answer direct messages (commands). Off: the bot only sends.
        4 commands: bool,
        /// Seconds between polls (30-600).
        5 poll_secs: u32,
    }
}

message! {
    pub struct BotView {
        1 state: String,
        2 instance: String,
        3 handle: String,
        4 commands: bool,
        5 poll_secs: u32,
        6 last_poll: u64,
        7 error: String,
        8 token_hint: String,
    }
}

message! {
    pub struct JobView {
        1 id: u32,
        /// `mail`, `dm`.
        2 kind: String,
        3 to: String,
        4 subject: String,
        /// `queued`, `sent`, `failed`.
        5 state: String,
        6 attempts: u32,
        7 next_at: u64,
        8 error: String,
        9 created: u64,
    }
}

message! {
    pub struct Delivery {
        1 jobs: Vec<JobView>,
        /// Notifications dropped because the queue was full.
        2 dropped: u32,
    }
}

message! {
    /// One command the bot handled (RAM only, newest first).
    pub struct InboxEntry {
        1 at: u64,
        2 from: String,
        3 text: String,
        4 result: String,
    }
}

message! {
    pub struct InboxLog {
        1 entries: Vec<InboxEntry>,
    }
}

message! {
    /// `PUT /api/v1/admin/publish`.
    pub struct PublishSetup {
        /// `owner/name` of the data repo.
        1 repo: String,
        2 branch: String,
        /// A fine-grained token with Contents read/write on that repo only.
        3 token: String,
        /// Where the companion site lives (`https://ourparty.github.io/party-site/`).
        4 site_url: String,
        /// Minutes between publications (10-1440).
        5 every_minutes: u32,
    }
}

message! {
    pub struct PublishView {
        1 state: String,
        2 repo: String,
        3 branch: String,
        4 site_url: String,
        5 every_minutes: u32,
        6 last_published: u64,
        7 generation: u32,
        8 files: u32,
        9 last_error: String,
        10 token_hint: String,
        11 last_commit: String,
    }
}

message! {
    /// `GET /api/v1/sync/status` (public).
    pub struct SyncStatus {
        1 published: u64,
        2 generation: u32,
        3 bot_polled: u64,
        4 site_url: String,
    }
}

// ---- Published (Sprint 8): only `public`-class fields ------------------------------

message! {
    /// `data/manifest.json`.
    pub struct Manifest {
        1 v: u32,
        2 generation: u32,
        3 created: u64,
        4 instance: String,
        5 bot: String,
        6 public: Vec<String>,
    }
}

message! {
    pub struct PublicGroup {
        1 id: u32,
        2 slug: String,
        3 name: String,
        4 about: String,
        5 kind: String,
        6 join: String,
        7 member_count: u32,
        8 hashtag: String,
    }
}

message! {
    pub struct PublicGroups {
        1 groups: Vec<PublicGroup>,
    }
}

message! {
    pub struct PublicEvent {
        1 id: u32,
        2 group: u32,
        3 title: String,
        4 about: String,
        5 kind: String,
        6 start: u64,
        7 end: u64,
        /// The place's name only ("Riverside Park"); never an address.
        8 area: String,
        9 status: String,
        10 going: u32,
        11 capacity: u32,
        12 series: u32,
    }
}

message! {
    pub struct PublicEvents {
        1 events: Vec<PublicEvent>,
    }
}

message! {
    pub struct PublicPerson {
        1 id: u32,
        2 display_name: String,
        3 bio: String,
    }
}

message! {
    pub struct PublicPeople {
        1 people: Vec<PublicPerson>,
    }
}

// ---- Remote services' JSON (only the fields read) ----------------------------------

message! {
    pub struct RemoteAccount {
        1 id: String,
        /// `user` on the bot's own instance, `user@host` from elsewhere.
        2 acct: String,
        3 username: String,
    }
}

message! {
    pub struct RemoteStatus {
        1 id: String,
        2 content: String,
        3 visibility: String,
    }
}

message! {
    pub struct Mention {
        1 id: String,
        2 account: RemoteAccount,
        3 status: Option<RemoteStatus>,
    }
}

message! {
    pub struct Mentions {
        1 items: Vec<Mention>,
    }
}

message! {
    pub struct StatusPost {
        1 status: String,
        2 visibility: String,
    }
}

message! {
    pub struct BrevoSender {
        1 email: String,
        2 active: bool,
    }
}

message! {
    pub struct BrevoSenders {
        1 senders: Vec<BrevoSender>,
    }
}

message! {
    pub struct GitObject {
        1 sha: String,
    }
}

message! {
    pub struct GitRef {
        1 object: GitObject,
    }
}

message! {
    pub struct GitCommit {
        1 sha: String,
        2 tree: GitObject,
    }
}

// ---- Stored -------------------------------------------------------------------

message! {
    /// `p<id>`: one person.
    pub struct StoredPerson {
        1 id: u32,
        2 display_name: String,
        /// Lower-cased; "" = none. Unique when set.
        3 email: String,
        /// PIN hash (`pin::hash`); "" = no PIN.
        4 pin: String,
        5 admin: bool,
        /// `active`, `disabled` or `invited`.
        6 status: String,
        /// Household id, 0 = none.
        7 household: u32,
        /// `nobody`, `members` or `public`.
        8 discoverable: String,
        9 bio: String,
        10 created: u64,
        11 last_seen: u64,
        /// SHA-256 (hex) of the personal calendar-feed token; "" = none.
        12 feed_hash: String,
        13 identities: Vec<StoredIdentity>,
        /// 256-bit key (hex) for the personal link (`spec/05`); "" until made.
        14 personal_key: String,
        15 email_off: bool,
        16 dm_off: bool,
    }
}

message! {
    /// `h<id>`: one household. Who is in it lives on each person.
    pub struct StoredHousehold {
        1 id: u32,
        2 name: String,
        3 created: u64,
    }
}

message! {
    pub struct StoredMember {
        1 person: u32,
        2 role: String,
        3 status: String,
        4 joined: u64,
    }
}

message! {
    /// `g<id>`: one group with its members (≤ `Limits::members_per_group`).
    pub struct StoredGroup {
        1 id: u32,
        2 slug: String,
        3 name: String,
        4 about: String,
        5 kind: String,
        6 visibility: String,
        7 join: String,
        8 members: Vec<StoredMember>,
        9 status: String,
        10 created: u64,
        11 updated: u64,
        12 rev: u32,
        13 created_by: u32,
        /// Plain members may create events, not just organizers.
        14 members_create_events: bool,
    }
}

message! {
    pub struct StoredAuditEntry {
        1 seq: u64,
        2 at: u64,
        /// Who did it (0 = the board).
        3 actor: u32,
        4 action: String,
        5 detail: String,
    }
}

message! {
    /// `l<id>`: one block of the audit ring (see `state::AUDIT_BLOCK`).
    pub struct StoredAuditBlock {
        1 id: u32,
        2 entries: Vec<StoredAuditEntry>,
    }
}

message! {
    pub struct StoredSession {
        /// SHA-256 of the cookie token, hex.
        1 hash: String,
        2 account: u32,
        3 created: u64,
        4 last_seen: u64,
    }
}

message! {
    /// `s<id>`: one person's sessions.
    pub struct StoredSessions {
        1 sessions: Vec<StoredSession>,
    }
}

message! {
    /// `meta`: everything that isn't a row.
    pub struct StoredMeta {
        1 schema: u32,
        2 next_person: u32,
        3 next_household: u32,
        4 next_group: u32,
        5 signup_closed: bool,
        6 groups_admin_only: bool,
        7 next_audit: u64,
        8 next_event: u32,
        9 next_series: u32,
        10 next_notice: u32,
        /// POSIX TZ string of the instance ("" = the default, US Eastern).
        11 zone: String,
        /// Waitlist order: each going answer that doesn't fit gets the next.
        12 next_queue: u64,
        13 next_invite: u32,
        14 next_job: u32,
    }
}

message! {
    /// `ip`, `ih`, `ig`, `il`, `is`: which rows of one table exist, in order.
    /// Written only when a row is added or removed.
    pub struct StoredIndex {
        1 ids: Vec<u32>,
    }
}

// ---- Stored: events (Sprints 3-5) -------------------------------------------------

message! {
    /// A bring-list slot ("Salad", 2 wanted).
    pub struct StoredSlot {
        1 id: u32,
        2 name: String,
        3 count: u32,
    }
}

message! {
    pub struct StoredQuestion {
        1 id: u32,
        /// `yesno`, `choice` or `text`.
        2 kind: String,
        3 prompt: String,
        4 choices: Vec<String>,
    }
}

message! {
    /// A candidate time in a `when` poll.
    pub struct StoredOption {
        1 id: u32,
        2 start: u64,
        3 end: u64,
    }
}

message! {
    pub struct StoredShift {
        1 id: u32,
        2 name: String,
        3 start: u64,
        4 end: u64,
        5 count: u32,
    }
}

message! {
    /// `e<id>`: one event (its answers are in `r<block>` records).
    pub struct StoredEvent {
        1 id: u32,
        2 group: u32,
        /// Series id, 0 = a one-off.
        3 series: u32,
        4 kind: String,
        5 title: String,
        6 about: String,
        7 hosts: Vec<u32>,
        8 start: u64,
        9 end: u64,
        10 where_kind: String,
        11 place: String,
        12 address: String,
        13 map: String,
        14 home: u32,
        15 visibility: String,
        16 invitees: Vec<u32>,
        17 capacity: u32,
        18 rsvp_by: u64,
        19 plus_ones: String,
        20 plus_max: u32,
        /// `draft`, `scheduled`, `cancelled` or `done`.
        21 status: String,
        /// Bumped by changes that affect answers (time, place, modules).
        22 rev: u32,
        23 created: u64,
        24 updated: u64,
        25 created_by: u32,
        26 modules: Vec<String>,
        27 bring: Vec<StoredSlot>,
        28 diet_prompt: String,
        29 questions: Vec<StoredQuestion>,
        30 cost: String,
        31 rides_note: String,
        32 when: Vec<StoredOption>,
        33 shifts: Vec<StoredShift>,
        34 link: String,
        35 reminder_secs: u64,
        36 reminded: bool,
        /// Which occurrence of its series (0-based).
        37 occurrence: u32,
        /// The next id for slots, questions, options and shifts.
        38 next_item: u32,
        /// Edits of any kind (iCalendar SEQUENCE).
        39 edits: u32,
    }
}

message! {
    pub struct StoredAnswer {
        1 question: u32,
        2 value: String,
    }
}

message! {
    pub struct StoredRsvp {
        1 person: u32,
        2 status: String,
        3 plus: u32,
        4 kids: u32,
        5 names: String,
        6 bring: Vec<u32>,
        7 bring_other: String,
        8 diet: String,
        9 answers: Vec<StoredAnswer>,
        10 ride: String,
        11 seats: u32,
        12 when: Vec<u32>,
        13 shifts: Vec<u32>,
        14 note: String,
        15 rev_seen: u32,
        /// Who answered (the person, a household member or a host).
        16 by: u32,
        17 via: String,
        18 updated: u64,
        /// Waitlist order (smaller = earlier); 0 when not waitlisted.
        19 queued: u64,
    }
}

message! {
    /// `r<id>`: up to `rsvps::BLOCK` answers of one event. Block ids are
    /// `event * rsvps::BLOCKS_PER_EVENT + n`, so an event's blocks are adjacent.
    pub struct StoredRsvpBlock {
        1 id: u32,
        2 event: u32,
        3 rsvps: Vec<StoredRsvp>,
    }
}

message! {
    /// `q<id>`: a recurring event's rule and its template.
    pub struct StoredSeries {
        1 id: u32,
        2 group: u32,
        3 rule: String,
        4 every: u32,
        5 nth: i32,
        6 until: u64,
        7 count: u32,
        8 window: u32,
        /// The first occurrence's start (UTC); later ones keep its wall-clock time.
        9 first: u64,
        /// Length of each occurrence (0 = no end time).
        10 length: u64,
        /// What new occurrences copy (id 0; times from the rule).
        11 template: StoredEvent,
        /// People with a standing "going".
        12 regulars: Vec<u32>,
        /// Occurrences created so far (the next one is number `made`).
        13 made: u32,
        /// `active` or `ended`.
        14 status: String,
        15 created_by: u32,
    }
}

message! {
    pub struct StoredNotice {
        1 id: u32,
        2 kind: String,
        3 event: u32,
        4 group: u32,
        5 actor: u32,
        6 created: u64,
        7 read: bool,
        8 count: u32,
    }
}

message! {
    /// `n<person>`: one person's notifications, newest first.
    pub struct StoredInbox {
        1 id: u32,
        2 notices: Vec<StoredNotice>,
    }
}

// ---- Stored: invitations, identities, outbound, channels (Sprints 6-8) ---------------

message! {
    /// A Mastodon account linked to a person: (host, subject) is the key.
    pub struct StoredIdentity {
        /// The person's home instance.
        1 host: String,
        /// Their account id there.
        2 subject: String,
        /// `@user@host`.
        3 handle: String,
    }
}

message! {
    /// `v<id>`: one invitation. Only the token's hash is kept.
    pub struct StoredInvite {
        1 id: u32,
        2 token_hash: String,
        /// `group` or `event`.
        3 target_kind: String,
        4 target: u32,
        5 issuer: u32,
        6 email: String,
        7 handle: String,
        8 person: u32,
        9 created: u64,
        10 expires: u64,
        11 uses_left: u32,
        /// `open`, `used` or `revoked` (expiry is computed).
        12 status: String,
        13 used_by: Vec<u32>,
    }
}

message! {
    /// `o<id>`: one outbound delivery (email or Mastodon DM).
    pub struct StoredJob {
        1 id: u32,
        /// `mail` or `dm`.
        2 kind: String,
        /// An email address, or `@user@host`.
        3 to: String,
        4 to_name: String,
        5 subject: String,
        6 text: String,
        /// An iCalendar attachment ("" = none) and its file name.
        7 ics: String,
        8 ics_name: String,
        9 reply_to: String,
        /// `queued`, `sent` or `failed`.
        10 state: String,
        11 attempts: u32,
        12 next_at: u64,
        13 error: String,
        14 created: u64,
        15 sent_at: u64,
        /// Lower runs first: 0 command replies, 1 invitations, 2 notifications.
        16 priority: u32,
    }
}

message! {
    pub struct StoredMail {
        1 api_key: String,
        2 sender_email: String,
        3 sender_name: String,
        /// `checking` (the worker verifies the key), `ready` or `failed`.
        4 state: String,
        5 error: String,
        /// Sends on `day` (days since the epoch) against the daily cap.
        6 day: u32,
        7 sent: u32,
    }
}

message! {
    pub struct StoredBot {
        /// `https://mastodon.social`.
        1 instance: String,
        2 trust: String,
        3 token: String,
        /// Filled in by the worker's check.
        4 handle: String,
        5 account_id: String,
        /// `checking`, `ready` or `failed`.
        6 state: String,
        7 error: String,
        8 commands: bool,
        9 poll_secs: u32,
        /// The newest notification seen ("" until the first poll).
        10 cursor: String,
        11 last_poll: u64,
        /// Recently handled status ids (a crash between the change and the
        /// cursor can't run a command twice).
        12 processed: Vec<String>,
    }
}

message! {
    /// The hash of one published file, to push only what changed.
    pub struct StoredFile {
        1 path: String,
        2 crc: u32,
    }
}

message! {
    pub struct StoredPublish {
        1 repo: String,
        2 branch: String,
        3 token: String,
        4 site_url: String,
        5 every_minutes: u32,
        6 last_published: u64,
        7 last_attempt: u64,
        8 generation: u32,
        9 files: Vec<StoredFile>,
        10 last_error: String,
        11 last_commit: String,
        /// An admin asked to publish now.
        12 asked: bool,
    }
}

message! {
    /// A Mastodon instance people may sign in with.
    pub struct StoredProvider {
        1 id: u32,
        2 name: String,
        3 base: String,
        4 trust: String,
    }
}

message! {
    /// `c<id>`: one channel's settings (1 mail, 2 bot, 3 publish, 4 providers).
    pub struct StoredChannel {
        1 id: u32,
        2 mail: Option<StoredMail>,
        3 bot: Option<StoredBot>,
        4 publish: Option<StoredPublish>,
        5 providers: Vec<StoredProvider>,
        /// Notifications dropped because the outbound queue was full.
        6 dropped: u32,
    }
}
