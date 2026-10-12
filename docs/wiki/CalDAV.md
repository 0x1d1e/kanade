# CalDAV

The `caldav` Module shows the calendars of CalDAV accounts, like Nextcloud, Fastmail, iCloud, mailbox.org or Radicale, on the Calendar Surface beside the local ones (ADR 0044). It is read-only.

1. Add the account to `~/.config/kanade/config.toml`, or in Settings under Data, Calendar:

   ```toml
   [[calendar.caldav]]
   name = "fastmail"
   url = "https://caldav.fastmail.com"
   username = "you@fastmail.com"
   ```

   `name` is what the CLI calls it. `url` is the server's CalDAV address; the server's own address is often enough, as Kanade looks for `/.well-known/caldav`. It must be `https://`, or `http://` only to `localhost`, `127.0.0.1` or `::1`.

2. Run `kanade caldav login fastmail` and type the password. It is not shown as you type; Ctrl-U clears it, Ctrl-C cancels. Kanade checks it with the server, and only then keeps it in the Secret Service, never in the config or a log. Without a terminal, it reads one line from standard input, as from a password manager: `pass show fastmail | kanade caldav login fastmail`.

Addresses and passwords for common servers:

| Server | `url` | Password |
|---|---|---|
| Nextcloud | `https://cloud.example.com/remote.php/dav` | an app password from Settings, Security |
| Fastmail | `https://caldav.fastmail.com` | an app password with CalDAV access |
| iCloud | `https://caldav.icloud.com` | an app-specific password from account.apple.com |
| mailbox.org | `https://dav.mailbox.org` | the account password, or an app password with two-factor sign-in |
| Radicale | `http://localhost:5232` | the password Radicale is set up with |

Kanade syncs at start, every 15 minutes, when the account changes in the config and on `kanade caldav sync`. When a server cannot be reached, it tries again after a minute, then less often, up to every 15 minutes. It syncs each calendar of events, from a year back to two years ahead, into `~/.cache/kanade/caldav/`, readable only by you. Recurring events keep their rules, and their reminders show as `calendar-reminders` says.

A problem shows under the agenda, in `kanade caldav status`, `kanade status` and `kanade doctor`; the events synced before stay, and the local calendars are not affected. After a wrong password, Kanade waits for `kanade caldav login` instead of trying again. Removing an account from the config deletes its synced events; `kanade caldav logout <name>` also forgets its password.

Kanade only uses HTTP Basic authentication. A server that needs Digest or OAuth, like Google's CalDAV, does not work; Google Calendar has a Module of its own, see [Google Calendar](Google-Calendar.md). A redirect to another server is not followed: `kanade caldav status` names the address to set as `url` instead.
