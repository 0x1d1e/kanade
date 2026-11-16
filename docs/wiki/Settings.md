# Settings

## Opening it

`kanade settings open [<page>]` opens the Settings window ([CLI](CLI.md)). So does the Settings tile in the shell. Its pages are by task, with a search field, Inspect (shows each row's key) and Undo.

Keys and values are in [Configuration](Configuration.md). The compositor pages edit niri's own `config.kdl` or Umbriel's `config.toml`. Only the active compositor's page and search results appear. `kanade settings open compositor`, `niri` or `umbriel` routes to that active page. A request for the inactive compositor does not expose its settings.

## The niri page

The page sits under Compositor and edits single options of niri's config (`$NIRI_CONFIG`, else `$XDG_CONFIG_HOME/niri/config.kdl`). It uses the same engine as `kanade niri-config` ([CLI](CLI.md), [ADR 0119](../adr/0119-kanade-edits-nieris-config-on-request.md)).

Nothing else in the file is touched. Only the bytes of the option change, so comments, blank lines and line endings stay. `niri validate` must accept the file before and the result after a change. The old bytes are kept as a backup. Kanade writes only that file, and only when you press Save. An option an include sets is read-only and names the file.

Sections: Layout (gaps, centering, struts), Focus ring and border (switches, widths, colors, window shadows), Input, Keyboard (repeat, Num Lock, layouts), Touchpad, Mouse, Cursor, Other (overview zoom, animations, client-side decorations, screenshot path). The search finds them like any setting.

### Staged changes

- A row shows the value in the file. When the file does not set it, the row shows the one niri uses, with a `default` mark. Reset removes the option from the file, so niri's default applies again.
- A change is kept in the page until you press **Save**. The row says `Not saved` and shows the value it would write. The file stays byte for byte as it was, and no backup is taken.
- Save writes every change staged since the last Save or Discard, each validated and backed up on its own. A staged key binding rename is written after them, as its own change. The footer says what it wrote.
- If it is refused, the reason shows in the footer. niri may reject the value, the file may have changed on disk meanwhile, or an include may own the option. Nothing on disk changes, and every change stays staged.
- **Discard**, beside Save, drops every staged change and reads the file again. The rows then show what it holds.
- A file that changes while changes are staged (in your editor) is read again. A staged row keeps showing what Save would write until you save or discard it.
- The page reads the file again after each write. It also reads it when it sees the file, or any include it reads, change. So an edit in your editor shows within a second while the page is open.
- The switches write the flags niri reads. A ring or shadow is on or off by `on`/`off` nodes and, when neither is there, niri's default. The border is the one exception: niri reads a `border { ... }` block in your own config with neither flag as on. So turning it off writes an explicit `off`, and a block with no flag shows as on. Reset on it writes `off` too.
- Undo in the footer undoes the last change made since the window opened. It does so while the file is still as that change left it; otherwise it says why not. After a Save of settings it takes the whole batch back, since those were one change with one backup; after a rename it takes that rename back.
- It is not offered while changes are staged: Discard is. A second press does not go further back. `kanade niri-config backups` and `restore` reach the older bytes and the first ones Kanade saw (`orig`).
- niri picks up the change itself within about half a second. It keeps its old config if it rejects the file.
- The page does not wait for it. Once a Save has written, the footer says whether niri reloaded the config, rejected it, or has not yet. A change to `spawn-at-startup` or `environment` says it applies after a restart.
- A value an include sets is shown read-only. It is named as `Set by appearance.sync` for `kanade.kdl` (the wallpaper colors, which `appearance.sync` rewrites) or `Set by <file>, an include`.
- If the config cannot be edited, one notice says why and every row is read-only, still showing what the file holds. The causes are a symlink or hard link, only `/etc/niri/config.kdl`, KDL v2 syntax, and a file or directory you cannot write. If it cannot be read, or niri rejects it as it is, the notice says that.

The recommended installation rules (`blur-xray`, `screencast-banners`, `overview-backdrop`) have switches on the page too. They are not one of its options. A press writes that rule at once, as one validated change of its own, rather than waiting for Save.

### Key bindings

The last section lists the key bindings of niri's own config: each chord as the file writes it, what it runs beside it, and a field for a new chord. Type a chord and press Enter: the row says `Not saved` and names the chord Save will write. An empty field, or the chord it has, takes the staging back. Save renames every staged chord with the same validation and backup as an option, and Undo puts the old bytes back.

Only the chord's token in the file changes. The action, its properties (`allow-when-locked`, `repeat`, `cooldown-ms`), the comments around it, the indentation and its one-line form stay byte for byte. A chord another bind already takes, once `Mod` is read through `input { mod-key }` (Super when the file sets none), is refused, and so is one KDL cannot write as a node name, an include's bind and a `/-` one; the footer says why and nothing is written. `Mod+Q` and `Super+Q` collide, `Mod+Q` and `Shift+Q` do not, and `Enter` and `Return` are different chords.

An include's bind and a `/-` one are not listed at all. A Save that writes one rename and then refuses another keeps the refused one staged for a fix, and Undo takes the written one back. Adding or removing a bind, or editing its action or properties, is still a hand edit (see [Limitations](Limitations.md)). The same rename is `kanade niri-config rebind` ([CLI](CLI.md), [ADR 0161](../adr/0161-niri-bind-rename.md)).

Not on the page: window rules, `spawn-at-startup` and `environment` (see [Limitations](Limitations.md)).

### Outputs

The first section of the niri page has a card for each monitor niri lists. It asks through the same IPC as the rest of the shell, again every 2 s while the page shows. So a monitor plugged in or out appears. A card names the monitor by make and model and its connector, and has:

- **Enabled**, a switch. The last monitor that is on cannot be turned off. The switch is disabled and says why. That is the only rule on what may be turned off.
- **Mode**, a list of the modes niri reports as `1920 × 1080 @ 144.001 Hz`, the current and the preferred marked. A mode is written as `WxH@R` with three decimals, as `niri msg output <name> mode` takes it. A monitor on a custom mode (a modeline) is refused a mode change with the reason: it is not in the list, so a revert could not name it; change it in the config.
- **Scale**, from 0.5 to 4 in steps of 0.25, by `-`/`+` or typed (Enter). A value outside the range is refused. niri rounds a scale to one the monitor can show. The card shows what niri applied.
- **Transform**: Normal, 90°, 180°, 270° and the four flipped ones.
- **Variable refresh rate**: Off, On or On demand, only for a monitor niri says supports it. niri reports on-demand as off until a window wants it. So the card takes it from the config.
- **Position**, x and y as whole numbers (negative allowed), written together by Apply or Enter. The layout canvas also lets you drag a monitor, snapping its edges to nearby monitors. Releasing starts a position trial. Text you type in a field is yours while a trial runs: the countdown and niri's poll do not replace it, only an Apply, Keep or Revert does.

A change is tried at once in niri (`niri msg output <name> ...`), which lasts only until niri reloads its config. Nothing is written to the file. About 300 ms later the card asks niri again and shows what it applied. A bar in the footer then asks **Keep** or **Revert** with 15 seconds on it. Each further change starts the 15 seconds over, and while it runs only that monitor can be changed.

- **Keep** writes every change of the trial to the monitor's `output` block. That is one validated change with one backup (the footer's Undo takes it back), then it waits for niri to reload the config. It watches for `ConfigLoaded` on the event stream, up to about 2 s.
- If niri's reload fails, the monitor is put back and the footer says so. If niri shows something other than what was written (a scale it rounded, say) the footer says so, and the pending file is kept as a record.
- If niri agrees, the pending file is deleted. If no reload is seen within the window (the event stream may be down), the keep is not undone, and a comparison is run instead. Since the config was written, the file is marked saved. A later start clears it without putting the monitor back.
- **Revert** is the button, Esc, the 15 seconds running out, turning to another page or searching, or closing the window or exiting Kanade. The monitor goes back to what niri reported before the first change. It never goes back to what the file says, as the file may differ from what was on.
- A revert niri does not accept, or that cannot ask niri which outputs there are, keeps the trial, and the footer says so. It is tried again every 5 s and by Revert.
- A revert never turns off the last monitor that is on. If the trial turned a monitor on and every other one has since gone, it stays on, and the footer says it was left on.
- Before the first change of a trial, the state to go back to is written to `$XDG_STATE_HOME/kanade/outputs-pending.json` (default `~/.local/state/kanade/`). It is deleted when the trial ends.
- SIGTERM and SIGINT (`systemctl --user restart kanade`, `scripts/dev`, Ctrl+C) put the monitor back before Kanade exits, for up to 3 s.
- If Kanade is killed in a trial (SIGKILL, a crash, the power), the next start puts the monitor back from that file. It forces the fields the trial changed, so a variable-refresh-rate mode niri reports as off (on demand) goes back too. A niri that lists no outputs yet is waited for, and the file stays.
- A file that records a saved keep is cleared instead, with the monitor left as the config now has it. For a file older than 5 minutes, Kanade restores Previous only if the monitor still matches the trial's applied edits. Otherwise it clears the record without changing the monitor, preserving later changes.
- A rejected change shows niri's message and writes nothing. A monitor whose block an include sets, or that two blocks match, is read-only with the file named.
- Reset on a card removes the keys Kanade manages from its block (and the block, if nothing else is in it). The block is named `make model serial` when niri knows all three (a built-in panel `eDP-*` and anything else by its connector).

A monitor that is off cannot be changed but for being turned on.

## The Umbriel page

Under Umbriel, Compositor shows the Umbriel page instead of niri. It edits `$UMBRIEL_CONFIG`, else `$XDG_CONFIG_HOME/umbriel/config.toml` (default `~/.config/umbriel/config.toml`). It preserves TOML comments and key order through `kanade umbriel-config`'s editor.

The Layout, Appearance and Input sections cover layout mode, gaps and extent presets; borders, corners, blur and shadows; keyboard repeat and layouts, touchpad, mouse and cursor options. A missing config can be created from Kanade's supplied default on request.

Changes are staged until **Save**. **Discard** drops them, and closing the window discards unsaved edits. Save validates the original and the result with `umbriel config validate -c <file>` before replacing the file. It then requests `umbriel msg config-reload`. If the Umbriel executable is missing, validation checks TOML syntax only.

A symlink is accepted when both it and its resolved regular-file target belong to you. Neither path may be under `/etc`, and the target and its directory must be writable. Kanade atomically replaces the target and preserves the symlink. Hard-linked files and files changed elsewhere are refused.

Backups live under `$XDG_STATE_HOME/kanade/umbriel-config/`. The footer's **Undo** takes back the last change from this window while the file still matches it. A failed reload is reported separately: the file was written and Undo stays available, but the running compositor may still use its old config. Layer-rule switches write immediately rather than waiting for Save.

### Outputs under Umbriel

The page shares the Outputs canvas and monitor cards described above. Monitor snapshots come from Umbriel. Trials use `output-set` and change only compositor memory.

**Keep** writes the connector's `[output."<connector>"]` properties (`enabled`, `mode`, `position`, `scale`, `transform`, `vrr`) through the Umbriel config editor and requests a reload. **Revert** restores the snapshot from before the trial without writing the config. The last enabled monitor cannot be disabled. Umbriel versions without `output-set` can show monitors but explicitly refuse trials.

Recovery uses `$XDG_STATE_HOME/kanade/umbriel-outputs-pending.json`, separate from niri's record, with the same saved-record and stale-trial rules above. See [ADR 0147](../adr/0147-umbriel-compositor-support-and-config-editor.md).
