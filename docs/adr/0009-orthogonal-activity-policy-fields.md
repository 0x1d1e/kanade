# 9. Orthogonal Activity policy fields

Status: accepted (design, roadmap 7A-8). Changes the `CONTEXT.md` Activity invariant "Scope and Interrupt follow from Lifetime and Priority" when implemented.

## Context

An Activity has four policy fields that answer different questions:

| Field | Question |
|---|---|
| `Lifetime` | how long it exists |
| `Priority` | how it ranks |
| `Scope` | where it appears |
| `Interrupt` | whether and how it interrupts |

Today only Lifetime and Priority are set. `Activity::scope()` and `Activity::interrupt()` in `src/island/activity.rs` derive the other two: Transient means FocusedOutput and `Interrupt::Transient` (shown over the primary), Persistent means Global and `Never`, Critical means `Preempt`. So the rules are accidental. Being Transient should not by itself mean interrupting the primary, and being Critical should not by itself decide where an Activity shows. `docs/design.md` adds values the derivation cannot express: `until-dismissed` Lifetime and `auto-expand(ms)` Interrupt.

`Interrupt::Transient` and the Frame's `transient` and `queued` slots exist for in-Island toasts and level changes. ADR 0007 moves those to Banners and the OSD, and ADR 0005 moves privacy to its own Overlay. (ADR 0021 later returns the OSD to the Island, as a Transient of its own Priority, not through `Interrupt::Transient`.)

## Decision

`Lifetime`, `Priority`, `Scope` and `Interrupt` are independent policy dimensions. No field implies another. Sources set each one explicitly, and the Arbiter interprets each one on its own.

- `Lifetime::Transient(duration)` means only that the Activity expires after `duration`. It implies no presentation and no interruption.
- `Interrupt` is `None | Preempt | AutoExpand(duration)`. `AutoExpand` opens the Activity's Surface for `duration`, then restores the prior Presentation and Surface. `Interrupt::Transient` is removed.
  - An explicit user action while it is open (click, open or toggle, collapse, pin, a consumed key) claims the current state and cancels the restore.
  - The pointer entering or leaving alone claims nothing. The pointer still on the Surface at the deadline keeps it open like Hold; leaving then starts the grace.
  - A `Preempt` arriving is policy, not user intent: the prior state is restored first, then it preempts as it would have.
- `Frame` is `primary`, `satellites`, `overflow`. The `transient` and `queued` slots are removed; Banners, the OSD and the privacy cluster have their own presentation.

### Validation

Validation stays minimal, so it does not bring the derivation back as rules. `Activity::new(...) -> Result<Activity, InvalidActivity>` rejects only what cannot be carried out:

- `AutoExpand` without a Surface to expand into.
- `AutoExpand` or `Preempt` with a Scope that targets no Island, if such a Scope is ever added.
- `Transient(0)`.

Unusual but explicit combinations are valid, and source policy decides whether to create them: `Persistent` + `FocusedOutput`, `Transient` + `Global`, a low Priority + `Preempt`, `Critical` + `None`.

A workspace switch shows the fields at work: `Transient`, `Osd` (now `Glance`, ADR 0021), `FocusedOutput`, `None` (ADR 0007). It is short-lived and may outrank the primary, yet interrupts nothing; the previous primary returns when it expires.

The Arbiter never panics on an Activity: anything it receives was valid at construction.

Arbiter rules may still read several fields at once, such as Satellite eligibility = `Persistent` and (`Ongoing` or `Critical`). That is policy over explicit fields, not one field implying another.

## Alternatives

**Keep deriving.** Fewer arguments per source, but every new behavior needs a new Lifetime or Priority value, or an exception to the derivation.

**Derive by default and allow overrides.** Hides the policy in defaults that a reader of a source cannot see, and the implied rules come back.

**Validate the usual combinations.** Rejecting `Transient` + `Global` or `Critical` + `None` turns today's derivation into validation rules, the same coupling under another name.

## Consequences

- Activity construction takes all four fields and can fail; sources state their policy where they post and handle `InvalidActivity`.
- Arbiter rules key on the field they concern: ranking on Priority, expiry on Lifetime, routing on Scope, displacement on Interrupt.
- Every source's mapping is rewritten. The tests that assert a derivation (`transients_are_for_the_focused_output`, `only_critical_preempts`) become tests of explicit policy, plus one test per rejected combination.
- Removing `Interrupt::Transient` and the `transient` and `queued` slots lands with or after ADR 0007's Banner and OSD, so no Transient loses its presentation in between.
- `CONTEXT.md` keeps the current invariant until the implementation PR, which changes code, tests and `CONTEXT.md` (Activity, Frame, Surface's dropped-Transient invariant) together.
