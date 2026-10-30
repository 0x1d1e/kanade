# 146. Polkit authentication on the Island

Status: accepted. Supersedes the refusal of polkit's prompts that ADR 0011 assumed and `docs/archived/design.md` stated ("Session ... not interactive, so polkit never asks for a password"; "polkit credentials stay external"). ADR 0120 stands: no privileged helper, no polkit action.

## Context

polkit asks an authentication agent for a password when a program requests an action it does not hold: mount, a package, a firewall rule, a network change from a program running unprivileged. ADR 0011 and the archived design assumed the shell never answers, so such a request went nowhere.

A prompt is outside text and a password at once. What polkit and PAM say is not Kanade's writing, and must not be trusted with the screen; what the user types must not be held anywhere after it is used.

## Decision

- **Kanade registers a user-session `org.freedesktop.PolicyKit1.AuthenticationAgent` on the system bus**, at `/org/kanade/polkit/agent`, for this logind session, registers once at startup, and registers again whenever `org.freedesktop.PolicyKit1` gets a new owner.
- **Only the current owner of `org.freedesktop.PolicyKit1` (polkitd) may call Begin/CancelAuthentication on it.** Anything else is dropped silently: anyone on the bus can name the agent's path.
- **Only this session's own logind user is accepted as the authenticatee.** polkit's `(sa{sv})` identity is read as a `unix-user` uid and compared with logind's `Session.User` for this session; a session, a group, or another user's uid is none.
- **The password goes to polkit's stock helper**, socket-activated at `/run/polkit/agent-helper.socket`: `<user>`, then `<cookie>`, a line each, then a line per PAM message (`PAM_PROMPT_*`, `PAM_ERROR_MSG`, `PAM_TEXT_INFO`, `g_strescape`d) until `SUCCESS` or `FAILURE`. PAM runs as root in the helper, so Kanade holds no privilege, registers no polkit action, and never calls `AuthenticationAgentResponse`.
- **Refusals are quiet void replies**, which polkit reads as not authorized: nothing shows and nothing queues. That is a locked session, a missing helper socket, no unix-user identity matching this session, or four requests already waiting. A malformed call, with no cookie, is instead a D-Bus `InvalidArgs` error.
- **Three tries per request**, then it is answered as not authorized. A failed try empties the field and says how many tries are left. A request is given up on when the session locks, when the user closes the prompt, or after 30 minutes with nothing typed.
- **`Surface::Auth` holds the keyboard while it shows**, and nothing but its own answer, its own Cancel or the session lock closes it. Escape cancels and answers polkit nothing.
- **The password is typed only on the Island, only while unlocked, in a field held as `Zeroizing`/`Secret`.** It is zeroized when let go, never logged, never sent over Kanade's own socket, never persisted. It is held to 256 characters, and a control character can never be typed.
- **The Surface shows polkit's message cut to one line and elided, `Password for <user>`, a dot per character, and Cancel/Authenticate.** polkit's message and PAM's words are outside text: cut, elided, never trusted with the screen.
- **`Bus::try_call` tells a failed call from its answer**; registration needs the reason it was refused.
- **The field component was taken out of the Wi-Fi prompt and shared** (`surfaces/password.rs`).

## Consequences

- The `polkit` Module is on unless turned off, needs only the core, and registers on start.
- Registration is refused while another polkit agent owns the name, and stderr says so. polkit-gnome is spawned by `dist/niri/config.kdl` and installed by `scripts/install`, so a session already running it keeps it, and Kanade's prompt never shows.
- The `BeginAuthentication` argument layout has not been checked against a live polkitd: with the helper unverified, a request fails closed rather than hanging.
- Nothing waits on the Island forever: three tries, the session lock, a closed prompt, or 30 minutes with nothing typed each end a request as not authorized.
