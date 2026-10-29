# 128. The Launcher lists sections for an empty query

Status: accepted.

## Context

An empty query listed one flat ranking: the six highest frecency scores as `Fit::Recent`, then every app by name. Commands, windows and destinations returned nothing, and apps mixed into a list with no shape. A person who opens the Launcher with a habit in mind has to rank by what they type, or scroll a flat list. The plan (`docs/launcher-work.md`, step 3) asks for sections instead.

## Decision

With nothing typed, the Launcher lists five sections in this order, each under a heading: the Recents, the Quicklinks, the Commands, the Windows and the Apps. (The Quicklinks joined them with ADR 0139.)

- The Recents are the six highest frecency scores (`Usage::recent`, ADR 0127), resolved back to the answer each key stands for. An answer the Recents show is taken out of the section it would otherwise list in, so nothing lists twice; a recent key that no longer resolves is left out rather than shown as a dead row.
- Quicklinks, Commands, Windows and Apps answer an empty query with everything they offer now, each as `Fit::Prefix`, so a section that would list nothing shows no heading. Destinations and Settings pages answer too but get no section, reaching only the Recents: they are many a page each, and listing them all would bury the windows and apps.
- The section order is fixed, not a ranking. Within a section, the provider's own order stands: Quicklinks in config order (ADR 0139), Commands by name and the higher score first, Windows by title, Apps by the higher score then by name.
- A heading is the list's own row, not a second widget: the list lays out a heading before each section's first answer, and a heading takes its own height, so a section's answers keep their place whatever it holds. Headings are drawn but never selected: Up and Down, the wheel and a click move over answers only.
- Typing any character returns to the ranked search of ADR 0126 and 0127 with the same query, unchanged.

## Consequences

An empty query now says what the Launcher holds rather than only what it led to lately. The sections cost a provider pass each on an empty query, which the memoed niri titles already absorb, and the list's scroll geometry is per-answer rather than a fixed step. The Flat list stays for a typed query, so the ranking remains the one place answers interleave across providers.
