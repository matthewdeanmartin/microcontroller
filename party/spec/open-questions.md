# Open questions

Assumptions made while writing this spec (October 21, 2026), to confirm.

1. **Which S3 board?** The N16R8s run mastomini, the bots and NanaCoin. Is one
   free for party, or is a new one on order? Sprint 0 needs its MAC. *(Answered
   2026-10-21: the former mastomini S3, `ac:a7:04:2c:29:9c`, freed when the
   household moved to the ESP32-P4. Installed; the gate measurements can run.)*
2. **Login name for PIN sign-in.** Game Matcher uses email. party has
   email-optional people, so this spec assumes email *or* a unique display name.
   Is that OK, or should email stay required for local accounts? *(Built that
   way in Sprint 1: display names are unique case-insensitively and can't
   contain `@`.)*
3. **The GitHub organization.** The spec asks for a dedicated free org so the
   Pages origin isn't shared with your other project sites ([05](05-companion-site.md#own-the-origin)).
   Name? (`ourparty` is a placeholder.)
4. **Bot handle.** `@party@mastodon.social` is a placeholder, and probably taken.
   Also: should the household mastomini host a second bot for people in the
   house, or is one public bot enough?
5. **Comments.** Left out of v0.1 (RSVP notes only). Right call for clubs?
6. **Who can create groups by default:** any active member (assumed) or admins only.
   *(Built: any member, with an admin switch "Only admins can start groups".)*
7. **Who can answer for whom:** household members for each other, and hosts for
   anyone invited (assumed). Should co-organizers who aren't hosts also be able to?
8. **Address default.** Addresses are `direct` by default (DM/email to people who
   are going), never published even encrypted. Too strict for family events at a
   known house?
9. **LLM default.** Off until the admin pastes an OpenRouter key. Once on, it's
   on for everyone who hasn't opted out. Opt-in per person instead?
10. **Time zones.** One instance time zone in v0.1. Are the son's and the
    mother-in-law's houses in the same zone?
11. **Demo data persona.** The demo uses fictional households. Any preference for
    the scenario (book club, family potluck, game club)?
