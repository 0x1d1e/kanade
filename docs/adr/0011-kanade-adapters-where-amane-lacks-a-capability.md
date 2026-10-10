# 11. Kanade adapters where Amane lacks a capability

Status: accepted (design, roadmap 9 Shell essentials). Replaces "Extend Amane first" in `docs/design.md` Runtime + dependencies.

## Context

`docs/design.md` says "Extend Amane first; bypass only when seam belongs to Kanade." Roadmap 9 needs capabilities the pinned Amane does not have:

| Capability | Module | Amane at the pin |
|---|---|---|
| Output and input devices, per-app streams | `audio` (#132) | `Audio`: volume and mute of the default sink and source only |
| StatusNotifierItem host | `tray` (#134) | no tray. `Bus` can call, read and set properties, own a name, serve methods, match and emit signals, but a served method does not learn its caller |
| Clipboard history | `clipboard` (#136) | none |
| Idle inhibit | `caffeine` (#141) | none. `Bus` drops UNIX fds, so it cannot hold the fd logind `Inhibit` returns |
| Suspend, reboot, power off, log out | `session` (#156) | `Bus` calls logind but answers a refusal, like polkit's, with nothing, so the island cannot say why. zbus, as for the tray |

Amane is third party (ADR 0004). A change there lands on its schedule, and Kanade moves the pin only deliberately, so "extend Amane first" leaves each of these Modules waiting. ADRs 0003 and 0004 already worked around Amane case by case with `pw-dump`, `pactl`, `udevadm` and `dbus-monitor`. This ADR makes that the rule.

## Decision

When the pinned Amane lacks a capability a Module needs, Kanade builds an adapter for it and does not wait for Amane. Prefer, in order:

1. **D-Bus through Amane's `Bus`**, when the service speaks D-Bus and `Bus` carries what it needs.
2. **An external tool** from the package that owns the capability: `pw-dump`, `pw-metadata` and `wpctl` for audio, `wl-paste` and `wl-copy` for the clipboard, `systemd-inhibit` for idle inhibit.
3. **A normal Rust crate**, when neither of the above is sound and the crate deepens Kanade. Never a Wayland or render crate (amended by [ADR 0020](0020-liquid-glass-over-its-own-connection.md): `crates/glass` captures the screen for liquid glass). The tray's watcher and host use zbus, which Amane already builds: an item that registers with only its object path, as libappindicator's do, lives at its caller's bus name, which `Bus` does not give.

Every adapter keeps these rules:

- **The Wayland boundary holds.** No Wayland client outside `src/doctor/`, enforced by `src/boundary.rs`, and the crates with a connection of their own (ADR 0025, 0028). A capability reachable only through a Wayland protocol, like wlr data-control for the clipboard, goes through an external tool that speaks it.
- **External semantics stop at the adapter.** It hands the rest of Kanade Kanade types, through a Service or `Island::write()`. Tool output, D-Bus paths and PipeWire ids go no further. `island/` stays unaware of it.
- **No idle cost.** It wakes on announcements, not a timer (ADR 0004).
- **A child process's lifetime follows its role.** Every one dies with Kanade (`setpriv --pdeathsig KILL`) and stays in the foreground, so the kernel can kill it.
  - A **follower** reads announcements for as long as its Module is on (`pw-dump --monitor`, `wl-paste --watch`). It runs under `wake::run`, which restarts it with backoff when its output ends, as ADRs 0003 and 0004 do.
  - An **action** does one thing and exits (`wpctl set-volume`, `wpctl set-default`), or reads one thing a follower's announcement leaves out (`pw-dump <id>` for a whole object). It is never restarted; a failure is reported to whoever asked.
  - A **holder** is state: it lives exactly as long as what it stands for (`systemd-inhibit` while caffeine is on, `wl-copy --foreground` while its entry owns the selection). The adapter starts and stops it. When it exits on its own, the state has ended and the adapter says so; it never restarts it, which could hold a lock nobody asked for or put back a stale selection.
- **A missing adapter dependency never takes down the shell.** It follows the Module rules in `docs/design.md`: a Module's required backend missing disables that Module with a named reason (`clipboard` without `wl-paste`, `caffeine` without `systemd-inhibit`); one that provides only an optional capability degrades it (`audio` without `wpctl` keeps Amane's master volume and mute). `kanade doctor` reports either case. A disabled Module starts no process and opens no bus name (ADR 0008).

When a later pin of Amane gains a capability, Kanade may drop its adapter in that bump. It does not have to.

## Alternatives

**Extend Amane first.** The rule this replaces. Each Module waits on a third party, or Kanade runs a fork.

**Fork Amane.** Kanade would maintain a second codebase, and every pin bump becomes a merge.

**Wayland clients in Kanade.** wlr data-control would give the clipboard without `wl-clipboard`, but it breaks "Amane owns the Wayland seam": a second connection and event loop beside Amane's. Wayland idle inhibit holds only while a surface is visible, which does not fit caffeine anyway; logind does.

**Native clients for everything** (libpipewire, a D-Bus crate with fd passing). Allowed by step 3 where a tool or `Bus` is not sound, but more code to own than `pw-dump` or `systemd-inhibit`, so not the default.

## Consequences

- `docs/design.md` Runtime + dependencies points here instead of "Extend Amane first".
- #132, #134, #136, #141 and #156 build adapters under this ADR.
- `wpctl` (WirePlumber), `wl-paste` and `wl-copy` (wl-clipboard) and `systemd-inhibit` (systemd) become runtime tools, each a required backend or an optional capability of its Module, listed by `kanade doctor`.
- More child processes: followers blocked on a read while idle, holders idle until their state ends.
- Kanade owns more protocol knowledge (SNI, PipeWire graph, clipboard MIME types) than a fuller Amane would ask of it.
