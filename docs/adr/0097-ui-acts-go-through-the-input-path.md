# 97. `kanade ui` acts through the runtime's own input path

Status: accepted. Item #21.

## Context

An act by name must do what a real press does, hover and drag and focus included, and must not move the user's pointer or steal their keys in the compositor.

## Decision

- The IPC handler is a stateless function on the event-loop thread, with no window state. It queues a request on a thread-local (`ui::ask`) and the socket closure serves the queue with `&mut WaylandState` right after answering (`serve_ui`), then asks for frames.
- `press NAME`: finds the node in the window's last frame, refuses a disabled one, takes its middle and checks that the topmost click- or drag-taking target there is the node or inside it (else it is covered or takes no click). It then runs the same `Pointer` calls a real left click makes: move, press, drag start/stop, release. The pointer's position and hover are saved before and restored after, so the user's pointer state is unchanged. No cursor shape is set.
- `focus NAME` is a press on a text input. `type NAME TEXT` focuses it, then delivers each letter as a key to the node's window. `key NAME KEY` delivers one key (`enter`, `ctrl+a`, ...), `scroll NAME N` sends wheel lines over the node.
- Keys use the keyboard path's `deliver_to(surface, key, modifiers, to_input)`, to the node's window and not the compositor's focus. The real keyboard path calls it with the same arguments as before.
- `wait NAME`: the client asks again every 100 ms (`Find`) until the node is there or `--timeout` (5 s default, 600 s at most) passes. The shell never holds a call.
- An act is addressed by name, never by coordinates. A name that fits several nodes is refused with their ids.

## Consequences

- A press is the real handler, so everything it triggers (hover, ring hide, Surface open) happens as for a user. It does not move niri's focus.
- A slider is pressed at its midpoint, so it sets 50%.
