# 136. Weather place search and IP lookup, on request

Status: accepted. Amends [ADR 0017](0017-weather-from-open-meteo.md), which kept the location to typed coordinates and refused both geocoding and IP lookup.

## Context

Typing `[latitude, longitude]` is the hardest step of setting up the Weather. ADR 0017 rejected the alternatives for two reasons: a name is ambiguous ("Paris" resolving silently to the most populous match), and automatic location is implicit, can be wrong, and tells more parties more about the user. Neither reason forbids finding the coordinates for the user when the user asks and sees the result.

## Decision

- **Place search through Open-Meteo's geocoding** (`geocoding-api.open-meteo.com/v1/search`), the provider family of the forecast: no key, no account. Settings, Data, Weather has a field taking "City" or "City, Country" (or region), pressed with Enter or Find. One match, or "Name, Qualifier" fitting exactly one place by name and country or region, is set at once and said ("Weather set to Phnom Penh, Cambodia"). Several are listed as buttons to pick from, so a place is never chosen silently. If a "City, Typo" matches nothing whole, the city alone is tried and its matches listed. No match, offline, or a refusal each show a sentence naming the cause and the next step.
- **IP lookup through ipwho.is** on a Use my location press. It is HTTPS and keyless, and it answered where ipapi.co rate-limited this machine and ip-api.com's free tier is HTTP only. ipinfo.io's keyless tier is a possible fallback, not wired. It gives a city, not a street; a VPN or a CGNAT address gives the exit's city, and the line after the press says "from your IP address (approximate)".
- **Explicit, never implicit.** Both lookups run only on a press in the Settings window, on a worker thread, once per press. Not at start, not on a first run, not on a reload, not on a timer, not when the Weather Surface opens. The IP address is sent to a third party only by Use my location, and the row says so beside the button before it is pressed. The text typed is sent to Open-Meteo only by Find. Neither the text, the address, nor an answer is logged; the IP the service echoes back is dropped.
- **Ownership.** `sources/geocode.rs` makes the two blocking requests (through ureq, with the answer's size limited as the forecast's is); `settings/place.rs` runs them off the draw thread, keeps what they found in the `Settings` Service and writes `weather.location` and `weather.place` to the settings file like any change, so Undo takes it back and the reload watch starts the forecast (ADR 0017's fetch thread is untouched). The `weather` Module being off still means no forecast requests; a lookup is the user's, from Settings.
- **Coordinates still work.** `weather.location`, `weather.place` and their rows are unchanged, so an existing config loads as before and a place can still be typed by hand or set by `kanade config`.
- **The empty Surface** says "Search for your city in Settings"; its pill and Enter open Settings on the Data page, where the finder is.

## Alternatives

**Silent IP lookup on first run.** Rejected: it sends the user's address to a third party without asking.

**GeoClue.** A system service with its own prompts and agent; it is not present on every system and would add D-Bus surface for one setting.

**Picking the most populous match.** Rejected for ADR 0017's reason; the qualified-name rule settles only what the text settles.

**Another IP service as primary.** ipapi.co rate-limited; ip-api.com is HTTP in its free tier; ipinfo.io wants a token past a small allowance.

## Consequences

- The Weather can be set from a fresh state with a name and Enter.
- Two more hosts can be contacted, only by a press: `geocoding-api.open-meteo.com` and `ipwho.is`. The wiki's Weather description says so.
- If ipwho.is changes or disappears, Use my location fails with a message that points to the name search; nothing else depends on it.
