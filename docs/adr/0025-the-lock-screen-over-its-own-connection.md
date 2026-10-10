# 25. The lock screen over its own connection

Status: accepted. Amends ADR 0018, whose lock was Amane's `Lock`, ADR 0022, whose lock screen field did not repeat, ADR 0024, whose lock screen was an Amane window and whose panes kept capturing under it, and the `wayland-client` rule in `AGENTS.md` for one more crate, `crates/lock`.

Validated on niri 26.04: `kanade lock` returns 57 ms after it is run, niri holding the lock with no red between, and the password unlocks it.

## Context

`kanade lock` took a few seconds to lock, with niri's red screen between. niri waits 1 s for a lock surface on every output and shows red where none came; Amane's `Lock` makes its surfaces only after niri says `locked`, then draws them on its own thread, a frame later. ADR 0018 also left every unlock running the `login` PAM stack twice, Kanade's check and Amane's. Amane cannot be patched (AGENTS.md).

## Decision

- **`crates/lock` (`kanade-lock`) holds `ext-session-lock` over a Wayland connection of its own**, on its own thread, as `crates/glass` did screencopy before ADR 0037. It makes a lock surface per output as it asks for the lock, and draws and commits each the moment niri sizes it, so niri has every surface before it would show red.
- **It draws the lock screen itself, on the CPU** (tiny-skia, text shaped by rustybuzz), from a scene per output the shell hands it while unlocked too: backdrop, date, time, who is signed in, status, colors and font files. The wallpaper is decoded, covered, blurred and shaded ahead, so a lock draws at once. The field's pill and the avatar's disc are liquid glass, their rims bent by `crates/lock/src/refract.rs`, frosting the backdrop they lie on, as the lock screen's backdrop is its own.
- **The shell keeps what the lock means** (`src/lock/`): requests and their confirmation (#196), sleep's gate (#155), PAM, who is signed in. The crate hands it the password on Enter and says when niri locked, ended a lock, unlocked, or the connection is gone; the shell asks it to unlock once PAM accepted. The password is checked once. Each lock asked carries its request's number, and the crate tells news by the newest request the lock serves; the shell sends its asks under its requests, which the crate takes in order, so news of an older request's lock never confirms a newer one. What the crate does on each ask and event is a pure stage (`kanade_lock::stage`), run by the shell's tests against its requests through every interleaving.
- **The scene comes from a hidden window per monitor**, whose view Amane runs as anything it reads changes, so the lock screen follows the wallpaper, theme, clock and status as the shell's windows do.
- **A lost connection is a crash**: the shell exits, so the unit restarts it and locks again (ADR 0018). niri keeps a lock whose client died without unlocking, showing red until the restarted shell's lock replaces it.

## Alternatives

**Show Amane's lock windows sooner.** Needs Amane to make its surfaces before `locked`; Amane cannot be changed.

**A separate `kanade-lock` process.** The same client, plus IPC for the scene, the password and the state the shell keeps, and its own restart path; the crate gets the same speed in the shell process.

**Draw it on the GPU.** A second wgpu device for a few surfaces drawn once a minute or on a key; the CPU draws a 1920x1080 lock frame in about 6 ms (release).

## Consequences

- The lock screen is drawn apart from Amane's widgets: a look Amane gains does not reach it, and its text and glass are kept in `crates/lock`.
- The lock thread wakes once a minute for the clock while unlocked, as the scene's view runs then.
- Each caret blink repaints the whole surface, about 6 ms of CPU a blink in release; debug builds compile `kanade-lock` optimized for it.
- The crate keeps, locked or not, each output's backdrop worked out (a full-size picture and its frost at a sixteenth of it), the last wallpaper decoded and each face at 256 px: about 8.8 MB a 1080p output plus 8.3 MB the 1080p wallpaper, four times that at 4K, up to 133 MB for an 8K wallpaper, the largest decoded. While locked, its shared memory holds two or more frames per output, let go of on unlock. Visual effects first; their cost is for later.
- The face and wallpaper are png, jpeg, webp, gif or svg, decoded by the shell (`wallpaper::decoded`): a webp or gif by its first frame, an svg drawn at 3840 px on its longest side. One that fails to decode, or is over an 8K screen's pixels, shows the initial or the solid color.
- PAM checks one password at a time: one dropped by a lock or sleep, still running, as a `pam_fprintd` stack waiting on a finger, holds the next back until it ends, the lock screen saying Checking and keeping the field; one that never ends needs a restart.
- Held keys repeat in the field at the compositor's rate, as the crate has its own keyboard.
- The pointer over the lock screen is niri's own arrow, asked by `cursor-shape-v1`; a compositor without it would show none, as the crate loads no cursor theme.
- Sleep's gate and the unlock are taken under one lock, so a password PAM accepted unlocks only if no sleep began; an unlock sent just before one is asked over (#155).
- Liquid glass captures nothing from niri's `locked` until the lock ends (`Glass::pause`), so nothing captures the lock screen; then every pane is captured again at once, as what is behind it may have changed.
