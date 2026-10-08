# 17. Weather from Open-Meteo

Status: accepted (roadmap 10 Desktop/data, #152). Settles the provider, location and refresh of `docs/design.md` Weather.

## Context

The design asks for current conditions and a short forecast, an explicit location, bounded refresh, and a stale/error state. A disabled `weather` Module must make no requests and run no timer. Network is allowed only for enabled remote-backed features, and the shell's idle cost should stay near zero.

## Decision

- **Open-Meteo's forecast API, no account or key.** One HTTPS GET per fetch (`/v1/forecast`) asks for the current temperature, feels-like temperature, humidity, wind, WMO weather code and day/night, plus 5 days of code, high, low and highest chance of precipitation, in the location's own time zone, so the days are named by the location's date. The answer is about 1 KiB. Data is CC BY 4.0, so the Surface credits Open-Meteo whenever it shows a forecast, stale or not.
- **Explicit location only.** `weather.location = [latitude, longitude]`, checked to ±90/±180. `weather.place` is an optional name for the Surface's title, and `weather.units` switches the display between metric and imperial. Units are converted on display, so changing them fetches nothing. No IP or GeoClue lookup, and no geocoding of a place name. Only the coordinates, to 4 decimals, leave the machine, and they are never logged.
- **Two Modules, like the calendar.** `weather` owns the fetch thread, the config keys and `weather refresh|status`. `weather-surface` requires it and owns `weather open|close|toggle`. Off, `weather` starts no thread. On without a location, its thread blocks on its queue, with no timer and no request.
- **Bounded refresh.** The thread fetches at start, then every 30 minutes, on `weather refresh`, and when a config reload changes the location. A failed fetch retries after 1 minute, doubling to 30 minutes, so offline costs about 7 wakeups in the first hour, then 2 an hour. The wait is monotonic and stands still during suspend. Opening the Surface therefore fetches when the forecast is 30 minutes old or more, but not within a minute of the last try. Messages that fetch nothing, like a reload with the same location, never put the next fetch off. Requests that pile up during a fetch are answered by it.
- **Stale, not blank.** A failed fetch keeps the last good forecast for the same location and shows it with "Offline · last updated 14:05". A forecast older than an hour without a failure shows as "Last updated". With no forecast, the Surface shows the reason: no location, the first fetch on its way, offline, or unavailable, with Open-Meteo's own message in `weather status`. A new location drops the old forecast, since it is for somewhere else. Forecasts live in memory only.
- **HTTPS through ureq**, already used for Google Calendar (ADR 0016). One blocking agent on the thread, 30 s per request, 64 KiB per answer.

## Alternatives

**MET Norway (api.met.no).** Free and keyless, but it requires an identifying User-Agent with contact details and caching by `Expires`/`If-Modified-Since`, and its symbol codes need its own icon mapping. Open-Meteo is simpler for one small request.

**OpenWeatherMap and other keyed APIs.** Each user would need an account and a key, and a key is a secret the design keeps out of TOML.

**Location by place name, through Open-Meteo's geocoding.** Names are ambiguous: "Paris" would resolve silently to the most populous match. Coordinates are explicit, and the name is only a label.

**Automatic location (GeoClue, IP lookup).** That is implicit, can be wrong, and sends more about the user to more parties. The design asks for an explicit location.

**Cache the forecast on disk.** It would show at once after a restart. But a restart is rare, a fetch is about 1 KiB and fast, and memory-only state needs no cache invalidation or file permissions. Revisit if a restart without network often matters.

## Consequences

- The weather needs `weather.location` set. Until then, the Surface says so and nothing is fetched.
- With the network up, a change in the weather shows within 30 minutes, or at once with `weather refresh`.
- Open-Meteo's free tier is for non-commercial use. 48 requests a day per user is far under its limits.
