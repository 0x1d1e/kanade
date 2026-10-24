# Kanade wiki

Kanade is a desktop shell for [niri](https://github.com/niri-wm/niri), centered on an adaptive activity island. See the [README](../../README.md) for what it is.

| Page | Covers |
|---|---|
| [Installation](Installation.md) | requirements, building, running as a systemd user unit, recovering a crashing shell |
| [CLI](CLI.md) | every `kanade <verb>`, and exit statuses |
| [Configuration](Configuration.md) | config layers, every key and its default, Modules |
| [Settings](Settings.md) | the Settings window, and its niri page that edits niri's config |
| [Agent hooks](Agent-hooks.md) | showing a Claude Code session as a live Activity, and adapting another agent |
| [Dictation](Dictation.md) | speaking to text with the Gemini API, what is sent to Google, setting up the key and consent |
| [Google Calendar](Google-Calendar.md) | signing in with your own OAuth client |
| [SDDM login screen](SDDM.md) | `kanade sddm`: Kanade's login screen theme, other themes, the overlay and the two root steps |
| [Keyboard](Keyboard.md) | niri keybinds, idle locking, keys inside each Surface |
| [Limitations](Limitations.md) | known gaps and what runs worse without an optional tool |

For the decisions behind it, see the [ADRs](../adr/), and the archived target design at [design](../archived/design.md). The names used here, such as Island, Surface and Activity, are defined in [CONTEXT.md](../../CONTEXT.md).
