# Google Calendar

The `google-calendar` Module shows a Google account's calendars on the Calendar Surface beside the local ones (ADR 0016). Kanade signs in with your own OAuth client, so you make one once:

1. In [Google Cloud](https://console.cloud.google.com/), create a project and enable the Google Calendar API in it.
2. Under Google Auth Platform, set up the consent screen as External and add yourself as a test user.
3. Create an OAuth client of type Desktop app and download its JSON file.
4. Run `kanade google-calendar sign-in ~/Downloads/client_secret_….json` and allow read access in the browser that opens.

The client and the refresh token go to the Secret Service, never to the config or a log, so the downloaded file can be deleted afterwards. Kanade syncs at start, every 15 minutes and on `kanade google-calendar sync`. It syncs each calendar shown in Google Calendar, from a year back to two years ahead, into `~/.cache/kanade/google-calendar/`, readable only by you. A problem, like access revoked or no network, shows under the agenda and in `kanade google-calendar status`. The events synced before stay, and the local calendars are not affected. While the consent screen is in Testing, Google ends the sign-in after 7 days and the Surface asks you to sign in again; publishing it to production avoids that. `kanade google-calendar sign-out` revokes access at Google and deletes the credentials and the synced events.

