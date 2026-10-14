# 53. Settings: keyboard ring, previews and contrast

Status: accepted. Amends ADR 0029 (previews now exist; rows show a short hint) and ADR 0050 to 0052 only in that Settings draws a sample of each finish.

## Context

An audit of the Settings window found: control edges below 3:1 against their ground (switch, segmented, fields), no keyboard operation or visible focus, help copy that was the schema's prose, a title twice and a heading over every single row, a Modules page of 40 identical rows under raw ids, no way to see what Tone, Finish or a spring would do before choosing it, and a footer that said nothing true about the file.

## Decision

- **Edge and ring tokens** (`ThemeRoles::edge`, `card`, `ring`, `grounds`): `edge` draws every control boundary and holds 3:1 against each ground, `ring` is the primary color when that holds, else the text color. A unit test checks both in each tone.
- **Keyboard ring** (`src/settings/ring.rs`): each control the view builds registers a stop, in build order, which is Tab order: the side, the header, the page, the footer. Tab and Shift+Tab travel (wrapping), Enter and Space press, arrows step a switch-less control (a segmented choice, a stepper, the page list), Escape leaves. Up and Down pass through a focused text input, so a stepper's number field keeps stepping. The ring survives a redraw by stop id and scrolls into view once on arrival (`Reveal` in the runtime). It draws as a border where the control has one, else a halo.
- **Hint**: a `Setting.hint`, one short line for Settings; `help` stays the long text for the wiki and CLI, shown in Settings only under Inspect.
- **Previews** in the row's own control: Tone and Finish as cards drawn with the shell's own finish and a light or dark sample, spring character as the step response of each damping ratio. This replaces ADR 0029's "no live preview": a sample of the choice, drawn from the same tokens, not a preview of the shell.
- **Less chrome**: the page title shows once, a section heading only over more than one row, a search result under its page and section.
- **Modules** open on a filter, names humanized, counts per group, a dependency in words ("needs windows"), no banner.
- **Footer** says what is true now: a refusal, a problem, a restart waiting, the last change saved, or where changes are saved. Undo shows only with something to undo.

## Consequences

- A new control registers a stop, or the keyboard cannot reach it.
- Previews are drawn code, not screenshots: a new finish or spring needs its sample drawn.
- A stepper's step sizes are still fixed by kind; a `-` or `+` button is pointer-only, the arrow keys cover the keyboard.
