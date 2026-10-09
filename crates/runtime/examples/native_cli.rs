//! Standalone control client for the native protocol probe.
//! cargo run -p kanade-runtime --example native_cli -- status
//!
//! The production CLI is unchanged until all Kanade modules have migrated.

use std::env;
use std::path::Path;
use std::process::ExitCode;

use kanade_runtime::ipc;

fn main() -> ExitCode {
    let Some(runtime_dir) = env::var_os("XDG_RUNTIME_DIR") else {
        eprintln!("kanade native CLI: XDG_RUNTIME_DIR is unset");
        return ExitCode::FAILURE;
    };
    let path = Path::new(&runtime_dir).join("kanade/native.sock");
    let arguments: Vec<String> = env::args().skip(1).collect();
    match ipc::call(&path, &arguments) {
        Ok(reply) if reply.starts_with("ok\n") => {
            let message = reply.trim_start_matches("ok\n");
            if !message.is_empty() {
                println!("{message}");
            }
            ExitCode::SUCCESS
        }
        Ok(reply) if reply.starts_with("refused\n") => {
            eprintln!(
                "kanade native CLI: {}",
                reply.trim_start_matches("refused\n")
            );
            ExitCode::FAILURE
        }
        Ok(reply) => {
            eprintln!("kanade native CLI: {reply}");
            ExitCode::from(3)
        }
        Err(error) => {
            eprintln!("kanade native CLI: {error}");
            ExitCode::FAILURE
        }
    }
}
