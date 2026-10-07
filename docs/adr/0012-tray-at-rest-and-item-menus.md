# 12. Tray at Rest and item menus

Status: accepted (roadmap 9 Shell essentials, #135). Changed the `CONTEXT.md` Presentation, Surface, Sub-surface and Pin entries.

## Context

`docs/design.md` says "Tray appears on interaction" at Rest, and lists what a StatusNotifierItem needs: primary and secondary activation, scroll, menus, submenus and actions. #134 hosts the items but shows none. Two things are undecided: where the items show without growing the resting island, and what a right click on one does, since ADR 0010 gives right click one meaning (raise and pin) and rules out context menus.

## Decision

### The strip

- **A Presentation, `Tray(slots)`.** The hover delay at Rest raises it while there are items: the time, then one slot per item, up to `TRAY_SLOTS`; past that the last slot is "+N" and opens the Tray Surface. It rests again when the pointer leaves and the grace runs out, or when the last item goes. It is Rest's own small form, as Peek is Compact's, so it takes the same hover, delay and grace and remembers nothing.
- **Hover only on entering.** As for a Peek, the delay starts when the pointer enters. Items arriving under a resting pointer do not raise it.
- **A slot is the item's.** Left activates it, or opens its menu when the item says it is only a menu. Middle is secondary activation. The wheel scrolls it, a notch 120. Right opens its menu.
- **Elsewhere on the strip it is Rest.** A click on the time opens Controls, a right click opens Controls pinned (ADR 0010).

### The Tray Surface

- **A Surface, `Tray`.** It lists every item: the row activates it, its chevron opens its menu. `kanade tray open|close|toggle` opens it, so the keyboard reaches every item.
- **A menu is a sub-surface.** An item's `com.canonical.dbusmenu` layout shows as rows: a separator is a rule above the next row, a checked or selected entry a check, a submenu a chevron, a disabled entry dimmed and inert, a hidden one absent. Entering a submenu is a further sub-surface; back or Escape returns to the entry that entered it. An action runs and collapses the island.
- **A menu opened from a slot is pinned.** An app's menu stays until dismissed, and the pointer leaving to reach the Surface's rows must not close it. It ends as any pin does.
- **Keyboard-complete.** Arrows move the ring, Enter or Space presses, Right enters a submenu, Left or Escape goes back a level, and Escape at the items collapses (`CONTEXT.md` Sub-surface).

## Alternatives

**Items always beside the time.** The resting island would grow and redraw for every icon change, against a minimal Rest.

**Right click on a slot raises and pins the strip, as elsewhere.** Consistent with ADR 0010, but the item's menu, the one thing a right click on a tray icon means in every other shell, would then be a chevron away in the Surface. The slot is the item's own target, not the island's, so right click is the item's. ADR 0010's reasons against a context menu (a hidden second route, actions that run unseen) do not hold: the menu is the app's, and it opens as a Surface level where every entry is seen before it runs.

**Menus as a separate layer window anchored to the slot.** A second window, its own focus and dismissal rules and input region, beside an island that can already show a list with a ring, back and pin.

**`ContextMenu(x, y)` for items with a menu.** Hands the app a position on a layer surface it cannot place a window against; most draw nothing or draw it wrongly on niri. Used only for an item that has no dbusmenu.

## Consequences

- `Presentation` gains `Tray(u8)` and `Surface` gains `Tray`. A Hover due at Rest applies only while there are items.
- `CONTEXT.md` Pin: a right click on a tray slot opens that item's menu pinned. No other target gains a context action.
- A menu reads its layout when it opens and on each `LayoutUpdated`, and calls `AboutToShow` before showing a level, so an app that fills its menu lazily shows it.
