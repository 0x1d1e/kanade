# SDDM login screen

Kanade ships a login screen theme for [SDDM](https://github.com/sddm/sddm), drawn like the lock screen, and `kanade sddm` writes what SDDM reads for it. It does not replace SDDM or any greeter: SDDM stays your login manager, and Kanade only writes a theme and a picture. Nothing here needs the shell to run, and the `sddm` Module (on by default) starts nothing.

Kanade never runs `sudo` and never writes `/etc` or `/usr`. It prints the two root steps for you to run.

## Quick start

```sh
kanade sddm enable    # writes the theme and picture, then prints the root commands
# run the commands it prints as root, once
kanade sddm status    # what is written, what SDDM reads, anything still missing
```

`enable` prints what is left, in the order to do it. First an `install -d` command: it makes `/usr/local/share/kanade-sddm/` owned by you, so Kanade can write there later without root. Then `kanade sddm sync`, which writes the theme. Last a command that writes `/etc/sddm.conf.d/zz-kanade.conf`, setting `[Theme]` `ThemeDir` to the overlay's `themes/` and `Current` to your theme; it does nothing unless the overlay already holds the theme (`test -f .../metadata.desktop`), so it cannot point SDDM at a theme that is not there. Nothing changes at the login screen until the last has run. Settings has a Login screen page with the same status and a Copy button on each command.

The file is named `zz-` so SDDM, which reads a directory's files in name order, reads it after any other there. A file read later still (`/etc/sddm.conf`, or a `zzz-` name) wins; `status` and `kanade doctor` name it, and only editing that file helps.

## Commands

| Command | Does |
|---|---|
| `kanade sddm status` | what is written, what SDDM reads, and the root commands still missing |
| `kanade sddm sync` | writes the overlay from the `sddm.*` keys: the theme, the picture, `theme.conf.user` |
| `kanade sddm enable` | `sync`, then the root commands that make SDDM read the overlay |
| `kanade sddm disable` | removes the picture, stops following the wallpaper, and prints the root commands that undo `enable` |
| `kanade sddm theme list` | the installed themes, each with its class |
| `kanade sddm theme info <name>` | what Kanade finds in a theme and what `sync` does for it |
| `kanade sddm theme use <name>` | sets `sddm.theme`, writes the overlay for it, prints the root commands |

## Themes

`sddm.theme` names a directory of `/usr/share/sddm/themes`; unset is Kanade's own, `kanade`, built into the binary. Kanade reads a theme's files to tell what it does with a picture:

| Class | The theme | `sync` does |
|---|---|---|
| A | reads a picture from `theme.conf` (a `background` key, or `config.background` in its QML) | writes the picture and sets that key in the overlay's `theme.conf.user` |
| B | draws a fixed image of its own (`background`, `bg`, `wallpaper` or `backdrop`) | links the theme, writes no picture |
| C | has no picture setting, or a plain color | links the theme, writes no picture |
| D | plays a video | links the theme, writes no picture |

Kanade's own theme is a copy; any other theme is a directory of links to its files, with its own `theme.conf.user` kept and only the picture setting replaced. Other themes are best effort: one that imports from a sibling or parent directory, or names `/usr/share/sddm/themes` paths, may not work from the overlay. Run `status`, then look at the login screen.

## The picture

`sddm.background` says what shows behind the clock: the wallpaper (`wallpaper`, a symbolic link to it is followed, as it is only read), an image (`path`, from `sddm.background_path`), the solid color `sddm.surface`, or `none`. The picture is blurred (`sddm.blur`) and darkened (`sddm.dim`) as the lock screen does, sized to the largest output, and re-encoded so it holds none of the original's metadata.

The picture is readable by the `sddm` user and by anyone who can read the overlay directory, so it holds a picture of your desktop. That is why `sddm.sync` is off by default. Turned on, with `sddm.background = "wallpaper"`, the shell writes the picture again whenever the wallpaper changes (changes close together make one write, and nothing polls). `kanade sddm sync` does the same by hand.

The picture Kanade writes is kept under 16 MiB (it shrinks the picture until it is). A picture you set by hand in the overlay's `theme.conf.user` is not checked by Kanade, and a Qt 5 greeter decodes it whatever its size, which can stall or crash the greeter: keep it small, or use `sddm.background_path`, which Kanade re-encodes.

## Security model

SDDM reads its theme as the `sddm` user, so the files it reads cannot be in your home. Kanade writes an overlay it owns, `/usr/local/share/kanade-sddm/` (`themes/<theme>/` and `bg/backdrop.png`), after root makes the directory once.

- Every directory Kanade makes is 0755 and every file 0644 whatever your umask, since `sddm` is not you; `status` and `doctor` flag any that is not, and `sync` mends it.
- Kanade refuses a symbolic link or a file with a second hard link in place of what it writes, and never writes through one.
- A theme is built beside the old one and swapped in, so a failure leaves the overlay that was there. Two writers (the shell and `kanade sddm sync`) take turns on a lock in your state directory.
- A theme's own text (file names, settings) is shown with its control characters removed.

The trade-off: the overlay is yours, so any process running as you can change the QML the login screen loads, which then runs as `sddm` (the greeter, before any login). On a machine with one user that adds little to what such a process can already do; on a shared machine, or with untrusted code under your account, it is a real trade-off. The alternative is a root-owned copy that Kanade cannot refresh. See [ADR 0120](../adr/0120-the-sddm-theme-is-an-overlay-the-user-owns.md).

## After a theme package changes

Another theme's overlay is links to the installed theme's files. When that package is upgraded or removed, the links can lead nowhere and the login screen falls back to SDDM's default or shows no theme. After upgrading or removing a theme package, run `kanade sddm sync` (the Login screen page and `kanade sddm status` say when the theme is out of date). The shell says once at start, on stderr, when SDDM reads the overlay and its theme is not current.

If the login screen is already broken, from a text console (Ctrl+Alt+F3) log in and run:

```sh
sudo rm /etc/sddm.conf.d/zz-kanade.conf
sudo systemctl restart sddm
```

That puts SDDM back on its own theme; nothing else of Kanade's is needed for that.

## Checks

`kanade doctor` (and Settings' Health page), with the `sddm` Module on, says whether the overlay is made, whether SDDM reads it and which file wins if not, whether the `sddm` user can read it by mode, and whether a Qt greeter (`sddm-greeter-qt6` or `sddm-greeter`) is on `PATH`. With no overlay it only notes so.

## Qt 5 and Qt 6

The theme is one QML source for Qt 5.15 and Qt 6. Its `metadata.desktop` says `QtVersion=6`; a Qt 5 SDDM needs that one line changed by hand in the overlay's theme. See [dist/sddm/README.md](../../dist/sddm/README.md) for the theme's files and keys.

## Undoing it

`kanade sddm disable` removes the picture and prints the root commands to remove `zz-kanade.conf` and the overlay directory. Removing the conf file alone puts SDDM back on its own theme.
