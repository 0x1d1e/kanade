# 16. Google Calendar account sync

Status: accepted (roadmap 10 Desktop/data, #151). Settles the design's open item "Define Google Calendar account adapter/auth/storage" for the optional account sync in `docs/design.md` Calendar.

## Context

ADR 0015 keeps the `calendar` Module read-only and offline: it reads iCalendar files, and leaves fetching to vdirsyncer "or a later account Module, whose files Kanade then reads". People who keep their calendar in Google want it on the island without setting up a sync tool. The design also requires that account secrets never reach a log or the TOML config, and are kept by the system secret service. The local calendars must keep working whatever happens to the account.

## Decision

- **A Module of its own, `google-calendar`, requiring `calendar`.** It is on by default but idle until a sign-in: its thread waits on its queue, and asks nothing of the keyring while no sign-in has been kept. A marker file in `$XDG_STATE_HOME/kanade/` records that one has, and holds no secret.
- **The adapter writes files, and the `calendar` Module reads them.** Each calendar the user shows in Google Calendar (`selected`, not `hidden`) becomes one `.ics` file under `$XDG_CACHE_HOME/kanade/google-calendar/`, named by a hash of its id. The directory is 0700 and the files 0600. The `calendar` Module adds the directory to its places once it exists, so Google's events go through the same reader, watch, Surface and bounds as local ones. Nothing else in the shell knows about Google.
- **Google expands recurrences.** The adapter asks the Calendar API for `singleEvents`, from 366 days before now until 732 days after, up to 20 pages of 2500 events per calendar. Each occurrence is written as its own VEVENT: timed ones in UTC, all-day ones as dates, with only the title and place. Cancelled events, and events the user declined, are left out. The window moves with each sync.
- **Sign-in is OAuth 2.0 for installed apps, with the user's own client.** `kanade google-calendar sign-in <client.json>` takes the JSON file Google Cloud gives for a Desktop app OAuth client. The browser opens Google's consent screen and returns the code to a listener on `127.0.0.1` on a free port. PKCE (S256) and a random state tie the code to this sign-in. Only `calendar.readonly` is asked for, with `access_type=offline` and `prompt=consent`, so every sign-in gets a refresh token. The browser has 5 minutes; a newer sign-in or a sign-out cancels a pending one. The opener (`xdg-open`) is never waited on before the callback, as it may run as long as the browser it starts.
- **One thread keeps the credentials.** The sync thread stores, uses and deletes them, handling a sign-in's credentials and a sign-out in the order they arrive. A sign-out cannot interleave with a sign-in being stored, even one waiting on a keyring prompt, and a sign-in cancelled before it is stored is dropped. A sign-in may be another account, so the events synced before are deleted once it is stored, and not before. A sign-in that is not stored leaves the account it would have replaced as it was: credentials, events, sync time and status. Each sign-in gets a random id, kept with the credentials and written beside the synced events (`.account`). At start, events synced for another sign-in, as a crash between storing and deleting would leave them, are deleted. A failed sync keeps the same account's events.
- **Secrets live in the Secret Service only.** The client id, the client secret and the refresh token are one item in the default collection (`application=kanade`, `account=google-calendar`). They are passed over the session bus with the `plain` algorithm, as the bus is the user's own. A locked keyring is unlocked through its own prompt. Access tokens live only in memory, are refreshed a minute before they expire, and are refreshed once more when the API answers 401. No secret, access token or event content is logged or printed, and `Credentials` has no `Debug` for them. Errors name Google's error code and description, or the Secret Service's words.
- **Syncs happen at start, every 15 minutes, and on `kanade google-calendar sync`.** A file whose content did not change is not rewritten, so an idle sync makes the calendar read nothing. That is 4 wakeups an hour while signed in, and none while signed out.
- **Failures are a state, never a crash.** `google-calendar status`, `kanade status` and the Calendar Surface's footer (or its empty state) show the problem:
  - A refused grant (`invalid_grant`: revoked, expired, or a password change) waits for a new sign-in instead of retrying.
  - When Google cannot be reached, the next interval tries again.
  - A refusal shows Google's message, for example that the Calendar API is not enabled for the client's project.
  - Other failures are the keyring having no credentials, or the files not being written.

  The events synced before stay on disk until sign-out, and the local calendars are untouched.
- **Sign-out removes everything.** `kanade google-calendar sign-out` revokes the grant at Google, deletes the keyring item, the marker and the synced files.
- **HTTPS through ureq with rustls and the web PKI roots.** It is one blocking client on the sync thread, with a 60 s limit per request and 32 MiB per answer. ring (already built by rustls) gives PKCE its randomness and SHA-256.

## Alternatives

**CalDAV with OAuth.** Google's CalDAV endpoint returns real iCalendar, rules and time zones included. It needs a second API enabled in the user's project, plus XML (PROPFIND and REPORT) on top of OAuth. The JSON API is the one Google documents for this, and with `singleEvents` it needs no recurrence handling.

**Write each series with its RRULE instead of expanding.** This would show events beyond the window. But Google's exceptions, cancelled instances and EXDATEs without VTIMEZONEs would have to be translated into iCalendar by hand. That is the hand-rolled recurrence ADR 0015 avoids.

**Keep events only in memory.** This writes nothing to disk. But the Surface would be empty after every start until Google answers, and offline it would stay empty. It would also need a second path into `Calendars` beside the files.

**A client id shipped with Kanade.** It would spare the user from making a client. But Google requires app verification for calendar scopes before more than 100 users may sign in, and a shared client's secret is public anyway. With their own client, users control their project and quota.

**Client id and secret in the config.** Google does not treat a Desktop app's secret as confidential. The design still keeps account secrets out of TOML, and the config is often published in dotfiles.

## Consequences

- Signing in needs a Google Cloud project with the Calendar API on and a Desktop app OAuth client. An unpublished ("Testing") consent screen lists the user as a test user. Google ends its refresh tokens after 7 days, so the account then shows "sign in again"; publishing the app to production avoids this.
- Google's events show only within the window: a year back and two years ahead.
- A change made in Google shows within 15 minutes, or at once with `sync`.
- Without a Secret Service (`kanade doctor` says so), signing in fails with its reason, and the local calendars are unaffected.
- The synced events are on disk in the user's cache, readable only by the user, until sign-out.
