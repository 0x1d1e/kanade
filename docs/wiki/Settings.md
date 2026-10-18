# Settings

`kanade settings open [<page>]` opens the Settings window ([CLI](CLI.md)). Its pages are by task, with a search field, Inspect (shows each row's key) and Undo; keys and values are in [Configuration](Configuration.md). This page covers the `niri` page, which is different: it edits niri's own `config.kdl`, not Kanade's.

## The niri page

The page sits under Compositor and edits single options of niri's config (`$NIRI_CONFIG`, else `$XDG_CONFIG_HOME/niri/config.kdl`) through the same engine as `kanade niri-config` ([CLI](CLI.md), [ADR 0119](../adr/0119-kanade-edits-nieris-config-on-request.md)). Nothing else in the file is touched: only the bytes of the option change, so comments, blank lines and line endings stay. `niri validate` must accept the file before and the result after a change; the old bytes are kept as a backup.

Sections: Layout (gaps, centering, struts), Focus ring and border (switches, widths, colors, window shadows), Input, Keyboard (repeat, Num Lock, layouts), Touchpad, Mouse, Cursor, Other (overview zoom, animations, client-side decorations, screenshot path). The search finds them like any setting.

- A row shows the value in the file, or the one niri uses with a `default` mark when the file does not set it. Reset removes the option from the file, so niri's default applies again.
- A change runs off the window's thread, as `niri validate` takes a moment: the row says `Saving…`, then shows the result. When the engine refuses (niri rejects the value, the file changed on disk meanwhile, an include owns the option), the reason shows under the row and in the footer, and the file is unchanged.
- The page reads the file again after each change and when it sees the file, or any include it reads, change, so an edit in your editor shows within a second while the page is open.
- The switches write the flags niri reads: a ring or shadow is on or off by `on`/`off` nodes and, when neither is there, niri's default. The border is the one exception: niri reads a `border { ... }` block in your own config with neither flag as on, so turning it off writes an explicit `off`, and a block with no flag shows as on. Reset on it writes `off` too.
- Undo in the footer undoes the last change made since the window opened, while the file is still as that change left it; otherwise it says why not. A second press does not go further back: `kanade niri-config backups` and `restore` reach the older bytes and the first ones Kanade saw (`orig`).
- niri picks up the change itself within about half a second, and keeps its old config if it rejects the file. The page does not wait for niri or say that it took the change.
- A value an include sets is shown read-only, as `Set by appearance.sync` for `kanade.kdl` (the wallpaper colors, which `appearance.sync` rewrites) or `Set by <file>, an include`.
- If the config cannot be edited (a symlink or hard link, only `/etc/niri/config.kdl`, KDL v2 syntax, a file or directory you cannot write) one notice says why and every row is read-only, still showing what the file holds. If it cannot be read, or niri rejects it as it is, the notice says that.

Not on the page: key bindings, window rules, `spawn-at-startup` and `environment` (see [Limitations](Limitations.md)).

### Outputs

The first section of the niri page has a card for each monitor niri lists (asked through the same IPC as the rest of the shell, again every 2 s while the page shows, so a monitor plugged in or out appears). A card names the monitor by make and model and its connector, and has:

- **Enabled**, a switch. The last monitor that is on cannot be turned off; the switch is disabled and says why. That is the only rule on what may be turned off.
- **Mode**, a list of the modes niri reports as `1920 × 1080 @ 144.001 Hz`, the current and the preferred marked. A mode is written as `WxH@R` with three decimals, as `niri msg output <name> mode` takes it. A monitor on a custom mode (a modeline) is refused a mode change with the reason: it is not in the list, so a revert could not name it; change it in the config.
- **Scale**, from 0.5 to 4 in steps of 0.25, by `-`/`+` or typed (Enter). A value outside the range is refused. niri rounds a scale to one the monitor can show; the card shows what niri applied.
- **Transform**: Normal, 90°, 180°, 270° and the four flipped ones.
- **Variable refresh rate**: Off, On or On demand, only for a monitor niri says supports it. niri reports on-demand as off until a window wants it, so the card takes it from the config.
- **Position**, x and y as whole numbers (negative allowed), written together by Apply or Enter. There is no layout canvas yet. Text you type in a field is yours while a trial runs: the countdown and niri's poll do not replace it, only an Apply, Keep or Revert does.

A change is tried at once in niri (`niri msg output <name> ...`, which lasts only until niri reloads its config) and nothing is written to the file. About 300 ms later the card asks niri again and shows what it applied. A bar in the footer then asks **Keep** or **Revert** with 15 seconds on it; each further change starts the 15 seconds over, and while it runs only that monitor can be changed.

- **Keep** writes every change of the trial to the monitor's `output` block in one validated change with one backup (the footer's Undo takes it back), then waits for niri to reload the config (up to about 2 s, watching for `ConfigLoaded` on the event stream). If niri's reload fails, the monitor is put back and the footer says so. If niri shows something other than what was written (a scale it rounded, say) the footer says so and the pending file is kept as a record. If niri agrees, the pending file is deleted. If no reload is seen within the window (the event stream may be down), the keep is not undone, and a comparison is run instead. Since the config was written, the file is marked saved: a later start clears it without putting the monitor back.
- **Revert** is the button, Esc, the 15 seconds running out, turning to another page or searching, or closing the window or exiting Kanade: the monitor goes back to what niri reported before the first change, never to what the file says, as the file may differ from what was on. A revert niri does not accept, or that cannot ask niri which outputs there are, keeps the trial and the footer says so; it is tried again every 5 s and by Revert. A revert never turns off the last monitor that is on: if the trial turned a monitor on and every other one has since gone, it stays on and the footer says it was left on.
- Before the first change of a trial the state to go back to is written to `$XDG_STATE_HOME/kanade/outputs-pending.json` (default `~/.local/state/kanade/`), and deleted when the trial ends. SIGTERM and SIGINT (`systemctl --user restart kanade`, `scripts/dev`, Ctrl+C) put the monitor back before Kanade exits, for up to 3 s. If Kanade is killed in a trial (SIGKILL, a crash, the power), the next start puts the monitor back from that file, forcing the fields the trial changed so a variable-refresh-rate mode niri reports as off (on demand) goes back too; niri that lists no outputs yet is waited for, and the file stays. A file that records a saved keep is cleared instead, with the monitor left as the config now has it. A file older than 5 minutes is deleted and not applied: it is not from a crash during a trial (they last 15 s), and applying it could undo what you did since.
- A rejected change shows niri's message and writes nothing. A monitor whose block an include sets, or that two blocks match, is read-only with the file named; Reset on a card removes the keys Kanade manages from its block (and the block, if nothing else is in it). The block is named `make model serial` when niri knows all three (a built-in panel `eDP-*` and anything else by its connector).

A monitor that is off cannot be changed but for being turned on.
