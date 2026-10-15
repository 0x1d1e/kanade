use std::fs::{self, Permissions};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::time::Duration;

use smithay_client_toolkit::reexports::calloop::{
    Interest, LoopHandle, Mode, PostAction, generic::Generic,
};

use crate::ipc::{IpcCall, IpcHandlers, IpcReply, ipc_socket};

use super::WaylandState;

pub fn insert(handle: &LoopHandle<'static, WaylandState>, handlers: IpcHandlers) {
    let source = Generic::new(listen(), Interest::READ, Mode::Level);

    handle
        .insert_source(source, move |_, listener, state| {
            // a failed call only loses that call, the shell keeps running
            if let Ok((stream, _)) = listener.accept() {
                answer(stream, &handlers);

                // a `kanade ui` call was queued for the windows, which this has
                state.serve_ui();

                state.request_frames();
            }

            Ok(PostAction::Continue)
        })
        .expect("failed to insert ipc socket");
}

fn listen() -> UnixListener {
    let path = ipc_socket();

    // a socket that still answers belongs to another running shell
    if UnixStream::connect(&path).is_ok() {
        panic!("failed to listen: kanade is already running");
    }

    // what is left is a leftover file from a shell that did not exit cleanly
    let _ = fs::remove_file(&path);

    let listener = UnixListener::bind(&path).expect("failed to create ipc socket");

    // whoever can connect can act on the shell (`kanade ui` presses and types), so only its user can
    fs::set_permissions(&path, Permissions::from_mode(0o600))
        .expect("failed to make ipc socket private");

    // the event loop only wakes this up when a client is waiting, so accept must never block
    listener
        .set_nonblocking(true)
        .expect("failed to make ipc socket non-blocking");

    listener
}

fn answer(stream: UnixStream, handlers: &IpcHandlers) {
    // a client that never finishes sending must not freeze the shell
    let timeout = Some(Duration::from_secs(1));

    if stream.set_read_timeout(timeout).is_err() {
        return;
    }

    let Ok(call) = IpcCall::read(&stream) else {
        return;
    };

    handlers.run(&call, IpcReply::new(stream));
}
