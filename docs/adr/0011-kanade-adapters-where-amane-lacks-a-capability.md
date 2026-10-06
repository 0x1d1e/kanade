# 11. Kanade adapters where Amane lacks a capability

Status: accepted (design, roadmap 9 Shell essentials). Replaces "Extend Amane first" in `docs/design.md` Runtime + dependencies.

## Context

`docs/design.md` says "Extend Amane first; bypass only when seam belongs to Kanade." Roadmap 9 needs capabilities the pinned Amane does not have:

| Capability | Module | Amane at the pin |
|---|---|---|
| Output and input devices, per-app streams | `audio` (#132) | `Audio`: volume and mute of the default sink and source only |
| StatusNotifierItem host | `tray` (#134) | no tray. `Bus` can call, read and set properties, own a name, serve methods, match and emit signals |
| Clipboard history | `clipboard` (#136) | none |
| Idle inhibit | `caffeine` (#141) | none. `Bus` drops UNIX fds, so it cannot hold the fd logind `Inhibit` returns |

Amane is third party (ADR 0004). A change there lands on its schedule, and Kanade moves the pin only deliberately, so "extend Amane first" leaves each of these Modules waiting. ADRs 0003 and 0004 already worked around Amane case by case with `pw-dump`, `pactl`, `udevadm` and `dbus-monitor`. This ADR makes that the rule.

## Decision

When the pinned Amane lacks a capability a Module needs, Kanade builds an adapter for it and does not wait for Amane. Prefer, in order:

1. **D-Bus through Amane's `Bus`**, when the service speaks D-Bus and `Bus` carries what it needs. The tray's watcher and host.
2. **An external tool** from the package that owns the capability, run and followed as ADRs 0003 and 0004 do: `pw-dump` and `wpctl` for audio, `wl-paste` and `wl-copy` for the clipboard, `systemd-inhibit` for idle inhibit.
3. **A normal Rust crate**, when neither of the above is sound and the crate deepens Kanade. Never a Wayland or render crate.

Every adapter keeps these rules:

- **The Wayland boundary holds.** No Wayland client outside `src/doctor/`, enforced by `src/boundary.rs`. A capability reachable only through a Wayland protocol, like wlr data-control for the clipboard, goes through an external tool that speaks it.
- **External semantics stop at the adapter.** It hands the rest of Kanade Kanade types, through a Service or `Island::write()`. Tool output, D-Bus paths and PipeWire ids go no further. `island/` stays unaware of it.
- **No idle cost.** It wakes on announcements, not a timer (ADR 0004). A child process runs under `wake::run`: `setpriv --pdeathsig KILL` and restart with backoff.
- **A missing tool or service is a soft capability.** The Module degrades with a named reason in `kanade doctor`; the shell stays up. A disabled Module starts no process and opens no bus name (ADR 0008).

When a later pin of Amane gains a capability, Kanade may drop its adapter in that bump. It does not have to.

## Alternatives

**Extend Amane first.** The rule this replaces. Each Module waits on a third party, or Kanade runs a fork.

**Fork Amane.** Kanade would maintain a second codebase, and every pin bump becomes a merge.

**Wayland clients in Kanade.** wlr data-control would give the clipboard without `wl-clipboard`, but it breaks "Amane owns the Wayland seam": a second connection and event loop beside Amane's. Wayland idle inhibit holds only while a surface is visible, which does not fit caffeine anyway; logind does.

**Native clients for everything** (libpipewire, a D-Bus crate with fd passing). Allowed by step 3 where a tool or `Bus` is not sound, but more code to own than `pw-dump` or `systemd-inhibit`, so not the default.

## Consequences

- `docs/design.md` Runtime + dependencies points here instead of "Extend Amane first".
- #132, #134, #136 and #141 build adapters under this ADR.
- `wpctl` (WirePlumber), `wl-paste` and `wl-copy` (wl-clipboard) and `systemd-inhibit` (systemd) become optional runtime tools, listed by `kanade doctor` as their Module's needs.
- More child processes, each blocked on a read while idle.
- Kanade owns more protocol knowledge (SNI, PipeWire graph, clipboard MIME types) than a fuller Amane would ask of it.
