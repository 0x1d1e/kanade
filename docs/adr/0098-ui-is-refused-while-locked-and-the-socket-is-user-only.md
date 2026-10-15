# 98. `kanade ui` is refused while locked, and the socket is user-only

Status: accepted. Item #21.

## Context

Acting on a shell by name is more reach than reading it. While the session is locked nothing may act, and the lock screen is the one thing whose contents never leave it.

## Decision

- `crates/lock` is not described. The lock window draws no targets of the runtime's tree, and Kanade's own `ui` module refuses every request while `lock::holds()` (`the session is locked`), described or not.
- The IPC socket is `chmod 0600` right after bind; it lives in `$XDG_RUNTIME_DIR` (0700) anyway, so the window between bind and chmod is not reachable by another user.
- `ui.describe = off` refuses at the entry and builds nothing ([ADR 0096](0096-ui-describe-privacy-modes.md)).

## Consequences

- Any process of the user can read the tree and act on the shell, as it could already run `kanade controls open` or `kanade wifi off`. `structure` keeps what a user or the world wrote out of the tree.
- Not a defense against a hostile process of the same user: such a process can already read the screen.
