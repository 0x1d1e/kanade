# 4. Wake sources on announcements, poll as fallback

Status: accepted (Phase 6, #40). Answers the plan §3 "revisit if profiling shows cost" row and the §11 idle wakeup risk.

## Context

Amane has no subscription between services, so a source that reads `Audio`, `Brightness`, `Notifications` or `Battery` must read again to learn of a change. Before this ADR they polled: the Transient source every 80 ms, notifications every 100 ms, battery every 1 s. A read that found nothing new posted nothing, so `AMANE_FRAMES=1` stayed silent, but the threads still woke the CPU.

Measured in #40 on niri with a debug build, idle, with threads identified under gdb:

| Thread | Wakeups/s |
|---|---|
| Transient source (`osd::follow`) | 12.65 |
| Notifications source | 10.05 |
| Amane `Brightness`, polls every 500 ms | 2.1 |
| zbus / `blocking` pool, idle threads time out every 500 ms | 2-4 |
| Battery source | 1.0 |
| Rest | under 1 |
| Total | about 29 |

The draw thread woke 0 times. CPU use was about 0.8% of one core, mostly startup.

Every change these sources look for is announced somewhere a std-only process can follow:

| Change | Announced by |
|---|---|
| Volume, mute, default device | `pactl subscribe`: `Event 'change' on sink #N`, `source`, `server` |
| Backlight, from a hotkey or brightnessctl | `udevadm monitor --kernel --subsystem-match=backlight`: `KERNEL[...] change ... (backlight)` |
| Charger in or out | `udevadm monitor --kernel --subsystem-match=power_supply` |
| Notification arrives or closes | `dbus-monitor --session --profile`: `Notify`, `CloseNotification` and `NotificationClosed` on `org.freedesktop.Notifications`. Amane emits `NotificationClosed` on every close, its only way to drop one |

All four flush each line through a pipe.

## Decision

`src/sources/wake.rs` decides when a source reads again. Each source names its announcers and a `Pace`:

| Source | Poll | Settle | Idle | Announcers |
|---|---|---|---|---|
| Transient | 80 ms | 1 s | none | `pactl`, `udevadm` backlight |
| Notifications | 100 ms | 1 s | none | `dbus-monitor` |
| Battery | 1 s | 6 s | 5 s | `udevadm` power_supply |

At idle the source blocks until a line announces a change. Then it reads at `poll` until `settle` passes without an announcement or a changed reading. This is needed because Amane's service learns of the change on its own thread, maybe after the announcement: `Brightness` reads the backlight every 500 ms and `Battery` every 5 s, so settle outlasts each. Battery still reads every 5 s at idle, because a draining battery does not announce its percent on every laptop.

An announcer that cannot run, or is between restarts, leaves its source blind, and a blind source polls at `poll` as before. Announcers restart with the backoff ADR 0003 gave pw-dump, which now shares `wake::run`. Each runs under `setpriv --pdeathsig KILL`, so the kernel kills it when Kanade dies, even by SIGKILL or an `amane dev` rebuild. Without setpriv it runs directly and may outlive Kanade.

## Alternatives

**Keep polling, slower.** This trades latency for wakeups. A held volume key needs 80 ms steps, so the floor stays high.

**Native PulseAudio, D-Bus or netlink clients.** These give the same announcements without child processes. But `amane dev` builds with Amane as the only dependency, so each means a hand-written wire protocol client. Amane's `Bus` can match signals, but not method calls like `Notify`, which Amane itself receives without emitting anything.

**inotify on sysfs (`amane::watch_file`).** sysfs attributes do not raise inotify events when the kernel changes them.

**Fix it in Amane.** A public change subscription would remove the need for all of this. Amane is third party, so Kanade does not wait for it.

## Consequences

- Measured after: total idle wakeups fell from about 29/s to 7.5/s. The Transient, notifications and battery sources wake 0, 0 and 0.2 times a second.
- The rest is Amane's: `Brightness` polling (2/s) and idle `blocking` pool threads (about 2/s each). Kanade cannot change them.
- `pactl` (libpulse or pipewire-pulse), `udevadm` (systemd) and `dbus-monitor` (dbus) are optional runtime tools. Without one, its source polls as before.
- Four more child processes run, each blocked on a read.
- A backlight that firmware changes without the kernel driver raising a uevent shows no brightness Transient. Drivers report hotkey changes through `backlight_force_update`, and sysfs writes always raise one, so this needs a driver that does neither.
