# 123. The first start is a tour

Status: accepted and implemented.

## Decision

- One pass after the first start of the shell with a focused output. The Welcome Surface explains how to open the Island and Surfaces, names the Launcher and Keys keybinds, and points to the Keys Surface for the user's niri binds.
- Enter, Space or Right advances; Left goes back; Escape dismisses. The final advance and dismissal both complete the tour.
- The tour takes no keyboard. It opens over a start that already has a window focused, so typing has to reach that window: it is an island that waits (`IslandService::wait`, which `IslandService::open` calls for the Welcome Surface), not one that holds, and OnDemand takes the keyboard only once it is pressed.
- It stays until the user dismisses it. Nothing closes it on its own: no hold, no grace, no self-close. Dismissing is Escape, the final advance, a press on the tour, another Surface opening over it, or `kanade island collapse`.
- The press is the tour's own content's, not the island's: a press on the island beneath it claims the Surface already open rather than ending it, which would take the keyboard and leave it there with no deadline. A typed character dismisses it too, as one into any held Surface that does not type, so the tour never swallows typing meant for the window beneath.
- The one close that is not the user's is niri's overview, which ends every open Surface (`set_niri` to `Presentations::set_overview`); the tour is recorded as over then too.
- `$XDG_STATE_HOME/kanade/welcome-tour` gates later starts, following the durability pattern of `$XDG_STATE_HOME/kanade/health-notice` in ADR 0088. It is written when the tour is over, however it closed: completion follows the Welcome Surface leaving the island (`surfaces::welcome::follow`, a watcher of `IslandService`), not the Surface's own code, since the island collapses an open Surface with no call into it. If the write fails, the tour still closes for this run, the error is logged, and a later shell start may offer it again.
- The tour is opened by the always-on Island Module and is not withheld when `keys-surface` is off. It does not choose settings, write niri config, or replace the health notice. Settings stays the place for choices.

Files: `src/surfaces/welcome.rs` owns the tour, its dismissal and its completion marker; `src/island/service.rs` opens it waiting (`wait`) instead of holding the keyboard; `src/island/presentation.rs`, `src/island/geometry.rs`, `src/view/forms.rs`, and `src/view/input.rs` register and dispatch its Surface; `src/modules/catalog.rs` starts it after the core Island setup.
