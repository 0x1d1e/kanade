# 29. Settings pages by task

Status: accepted. Amends the Settings window of #148, which had a page per Module.

## Context

Settings had a page for each Module that owns settings, each key on a card under its config name. Most keys belong to the core Module, so one page held the look, the Island's placement, motion, timings, privacy and the Module switches, while others held a single key. Users look for a setting by what it does, not by which Module owns it.

## Decision

- **Pages by task** (`src/settings/layout.rs`): Appearance, Island, Dock, Motion & timing, Notifications & privacy, Modules, Data and Lock screen, under Shell, Experience and System, each in sections. The layout only arranges the schema: every key shows on exactly one page and every Module but the core in one group of the Modules page, both held by tests.
- **Rows by label**: each `Setting` has a `label` and each `Module` an `about` line, shown with its help; Inspect shows the config keys. The material shows as tiles of each material, an edge and side as one grid of the eight places (six until ADR 0030 added the side edges).
- **Search** across every page, by label, help, key and Module.
- **Undo** writes back what each change since the window opened replaced in the settings file, a change of several keys taken back whole. A change of several keys is one write of the file (`file::set_all`), validated as a whole. Undo refuses, leaving the file as it is, if a key no longer holds what the window wrote, so an edit made elsewhere since is never overwritten.
- `kanade settings open <page>` takes the new names; a Module's name opens the page with its settings.

## Consequences

- A new key needs a place in the layout, or the tests fail.
- No live preview or export, as in the mockup: the shell itself is the preview, and the settings file is the export.
