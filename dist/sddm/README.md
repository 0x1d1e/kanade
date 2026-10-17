# Kanade's SDDM theme

`kanade/` is the login screen, drawn to match the lock screen (`crates/lock/src/paint.rs` is the
reference: where the two differ, the lock screen is right and this is the bug). MIT, written from
scratch in QML; see `kanade/NOTICE`. `kanade sddm` installs it (below).

## Layout

| file | what |
| --- | --- |
| `metadata.desktop` | `QtVersion=6`. A Qt 5 SDDM needs only this line changed (`scripts/sddm-preview stage DIR --qt 5`) |
| `theme.conf` | the config keys below, each with its default |
| `Main.qml` | SDDM's entry: adapts its globals to a host object; a Loader falls back to a plain form if `Greeter.qml` fails |
| `Greeter.qml` | the screen, a pure component that reads everything from `host` |
| `Pill.qml` `Chip.qml` `Avatar.qml` `Line.qml` `TimeLine.qml` `CapsGlyph.qml` `Glyph.qml` | its parts |
| `Sanitize.js` | validation of every config value, the date and time text, the shadow's falloff |

`theme.conf.user` beside `theme.conf` overrides it. Keys: `backdrop`, `face`, `shade`, `font`,
`clock24`, `scale`, `session`, `surface`. Every value is validated; a bad one takes its default.
Images are paths relative to the theme directory, without `..`.

## Installing: `kanade sddm`

SDDM reads its theme as the `sddm` user, so Kanade writes what it reads to an overlay it owns, outside
`/usr/share`: `/usr/local/share/kanade-sddm/` (`themes/<theme>/` and `bg/backdrop.png`). The theme is
chosen and styled by the `[sddm]` keys of the config (`sddm.theme`, `sddm.background`, `sddm.blur`, ...).

```sh
kanade sddm status   # what is there, what SDDM reads, and the root commands still missing
kanade sddm sync     # writes the overlay: the theme, the picture, theme.conf.user
```

Kanade never runs sudo and never writes outside what you own. Root does two things once, which
`status` prints for you to run: make the overlay directory owned by you (`install -d -o UID -g GID -m
755 ...`), and write `/etc/sddm.conf.d/zz-kanade.conf` (`[Theme]` `ThemeDir` and `Current`). The file
is named `zz-` so SDDM, which reads a directory's files by name, reads it after any other there; a file
read later still (`/etc/sddm.conf`, or a `zzz-` one) wins, and `status` names it.

- Kanade's own theme is copied from the one built into the binary. Any other installed theme is a
  directory of links to its files, with its `theme.conf.user` kept and only `background` replaced.
  Other themes are best effort: one that imports from a sibling or parent directory, or names
  `/usr/share/sddm/themes` paths, may not work from the overlay. Run `status`, then look at the login
  screen.
- The picture is the wallpaper (or `sddm.background_path`) blurred and dimmed as the lock screen does,
  re-encoded so it holds none of the original's metadata. The login screen's user and anyone who can
  read the overlay can read it.
- Every directory Kanade makes is 0755 and every file 0644 whatever your umask, since `sddm` is not you;
  `status` flags any that is not, and `sync` mends it. `sync` builds a theme beside the old one and
  swaps it in atomically where the filesystem supports it (`renameat2` with `RENAME_EXCHANGE`),
  falling back to two renames on others, so a failure leaves the overlay that was there.

Trade-off: the overlay is yours, so any process running as you can change code and QML the login screen
loads, which then runs as `sddm` (the greeter, before login). On a machine with one user, that adds
little to what such a process can already do; on a shared machine, or with untrusted code under your
account, it is a trade-off. The alternative is a root-owned copy that this command cannot refresh.

## Rules

- One QML source for Qt 5.15 and Qt 6: versioned `2.15` imports, no `function onX` in `Connections`,
  no `Effects`/`MultiEffect`, no Qt5Compat, no QtMultimedia, no SddmComponents. The shadow is stacked
  translucent rectangles.
- No `Process`, no network, no external call. Every `config.*` read has a default; every global is
  guarded with `typeof`.
- Essential controls are never under a layer effect. Every screen gets the whole screen with a live
  field (SDDM does not say which output a person types at), and each guards its own submit.
- `Greeter.qml` knows nothing of SDDM: its host is `users`, `sessions`, `defaultUser`,
  `defaultSession`, `capsLock`, `layouts`, `layoutIndex`, `info`, `canSuspend`/`canReboot`/
  `canPowerOff`, `options`, `cycleLayout()`, `login(user, password, sessionIndex)`, `suspend()`,
  `reboot()`, `powerOff()` and the signals `loginFailed`, `loginSucceeded`. The preview mocks it.
- The default session is SDDM's `sessionModel.lastIndex` unless `session` names one.

## Preview

```sh
scripts/sddm-preview [--out DIR] [--only NAME ...]
```

Renders 40 states (idle, typing, caps lock, wrong password through its shake, 12-hour clock, long
names, a backdrop and face, garbage config, ...) to PNGs under `$XDG_RUNTIME_DIR/sddm-preview` (or `--out DIR`), at 1920x1080 and
at 3840x2160, under Qt 6 and Qt 5, and offscreen with the software renderer. It measures what was
drawn against the lock screen's numbers (1 px), and it runs the real `sddm-greeter-qt6` and
`sddm-greeter` with `--test-mode` and types into them. Everything runs under Xvfb on a free display
with `WAYLAND_DISPLAY` unset, or Qt's offscreen platform: it never opens a window in your session,
and never starts or stops sddm. Needs `Xvfb`, `qml6`, optionally `qml-qt5`, Python 3, ImageMagick.
Not part of CI.

## Limitations

- A hidden or LDAP-only account that SDDM does not list can sign in through the manual username
  entry. When the user list is empty, a visible username field takes its place above the password.
  When there are listed users, pressing the "Other user" chip past the last one moves to a cleared
  username field; pressing it again returns to the list. With one listed user the same chip appears,
  so the manual entry is always reachable with the keyboard alone.
- Several screens each show the whole login screen. The password typed at one is not mirrored to the
  others. Each instance guards its own submit (`Checking`). SDDM probably gives each screen its own
  QML engine, in which case nothing is shared between screens; the guard across instances of one
  engine exists but may never apply on a real multi-screen setup.
- Not seen on real hardware: multi-monitor under a live SDDM, the GPU path, the daemon's PAM flow.
