# 24. Glass on every surface

Status: accepted. Amended by ADR 0037: a pane's copy is a GPU texture the runtime captures, not a rim picture. Amended: the materials are `liquid-glass` and `monochrome` (one flat tone, nothing seen through it, little rim light) until more are added; `transparent`, `clear`, `obsidian` and `opaque` are gone. Amends ADR 0020, whose material was the Island's only, and ADR 0023, whose twin saw nothing behind it. Amended by ADR 0025: the lock screen is drawn by `kanade-lock`, its glass bent from its own backdrop, and nothing captures under it.

## Context

ADR 0020 made liquid glass for the Island alone; the Dock kept a flat card, Banners a filled one, and the lock screen a solid color. On macOS one material runs through the menu bar, Dock, notifications and lock screen. `appearance.material` should read as the whole shell's.

## Decision

- **Panes.** Each piece of glass is a pane, named by a `Spot`: its output and a name there, `island`, `dock` or `banner-<i>`. the runtime captures each pane apart (ADR 0037), and everything `glass/` keeps (rims, timing, lead) is kept per `Spot`. Each window places its panes with `glass::place`, folding its margins into the canvas and body.
- **Variants**, as Apple's: `Clear` lets the backdrop through, for the Island and the Dock, whose few glyphs read over anything; `Regular` tints it enough for lines of text, for the Banners: dark at least 0.58 opaque, light 0.72, a little more or less as the backdrop is light. The middle is not frosted as Apple's: the capture holds the card itself, so only the rim, sampled outside it, can show the backdrop bent; the tint lets the backdrop through as glass but hides it enough that the text over it still reads.
- **Constant layer count.** `glass::layers` always gives the rim, tint and shader, the rim empty when there is none, and decoded rims no longer shown (`glass::unseen`) are drawn last in their window. Amane tracks hover and press by a target's index in preorder, so a count that changes from frame to frame before clickable content would move the targets under the pointer.
- **Dock.** The strip is a Clear pane, its border shown only when not liquid. In every material it takes the Island's tint, so an opaque or obsidian Dock is no longer a filled card but as the Island is. While it is out of sight, empty, slid past its edge or stepped aside, it captures nothing.
- **Banners.** Each card is a Regular pane of its own while it shows, so one leaving never hands its rims to another: a pane let go is forgotten, its newest rim and how long its rims took, and one no Banner draws in stops capturing while its rims linger out unseen, stepped only until they are freed, the Banners' window kept up for them. Each is captured where it is heading, ahead by how long its own pane's rims take, as the Island's body is. The twin is now the card's own glass, so it refracts from its first frame; under liquid glass it draws no drop shadow, which would darken the backdrop it bends. The Banners follow the Island's autohide slide, as `autohide::Wakes` is also written as a slide starts, and while any shows the Island stays out, so none slides from under the pointer.
- **Lock screen.** As macOS's: the date over a large time near the top; the user's picture (the first png, jpeg, webp, gif or svg of `~/.face`, `~/.face.icon` and AccountsService's, an svg known by the name a link to it has, once Amane decoded it) or initial on a glass disc, their full name as NSS gives it (`getent passwd`, looked up off the view as a lock is asked), and a glass pill to type the password in near the bottom. Behind, the wallpaper awww shows on that output, read from `awww query` (`wallpaper::Shown`) at start, as Kanade sets one and as the lock is asked, and asked again a few times while a daemon still starts, as at login. `lock.backdrop` blurs it (a small thumbnail blurred), dims it, shows it sharp, or shows a solid color; with no wallpaper known, solid. Over a wallpaper the text is light on dark glass, whatever `appearance.tone`, under a shade as dark as the wallpaper's mean luma needs (any image the lock screen decodes, decoded for it once per file). The pill and disc frost what is behind them with Amane's backdrop blur, as the lock screen is one window and everything behind them is its own; the lock screen captures nothing itself.

## Alternatives

**One capture per window, cut per pane.** Fewer captures, but a window's canvas is mostly empty and a Banners window is tall; panes keep each capture to its body.

**Compositor blur (`ext-background-effect-v1`) behind the Dock and Banners.** Rejected in ADR 0020 for the Island for the same reasons: a surface of its own under each pane and niri's xray blurring only the wallpaper.

**Refract the wallpaper into the lock screen's pill and disc.** No capture is needed, as the wallpaper is known, but Kanade would decode and blur it itself, so the rim matches what is drawn, and pin where the pill and disc lie. Over the default blurred backdrop a bent blur looks as the blur does, and macOS's own lock field is frosted.

**Reuse the Island's capture for the lock screen.** Screencopy of a locked session shows the lock itself, and frosting the wallpaper needs no capture.

## Consequences

- Each pane shown on liquid glass is one more capture and rim stream; three Banners, the Dock and the Island are five per output. Visual effects first; their cost is for later.
- The Settings window, a window of its own, keeps its flat look.
- The lock screen reads png, jpeg, webp, gif and svg (ADR 0025). An image over an 8K screen's pixels, or one that fails to decode, is not read for its lightness, so its shade takes it as fairly light. The mean luma is the whole wallpaper's, not where the text sits.
- The Island's, Dock's and Banners' panes keep the placement they last drew while locked, as their windows stop drawing under the lock screen, so a change on the lock screen beside them may still be captured.
- Every pane's rims come over one `Backdrops` subscription per window, so a window redraws for any of its panes.
- `theme.palette` no longer reaches the Dock or Banners: as the Island, they take their colors from `appearance.tone`, which tints their glass; only the Settings window follows the palette.
- The face is decoded as a lock is asked, before who is signed in is looked up again, so the lock shows it from its first frame; one decoded for a lock niri refused, or since changed, is never drawn, so Amane keeps it for the whole run; the wallpaper only as the first lock screen draws it, so there it may come a moment after the clock. Amane keeps the blurred small copy for the whole run, a few hundred kilobytes, and frees the full size one, sharp or dimmed, on unlock.
- Amane knows a picture by its path, so a wallpaper or face rewritten in place keeps its old picture until Kanade restarts, while its shade, kept by the file's modification time, follows.
- The lock screen was checked on a preview window, not a real lock, as only the user's password unlocks their session.
