//! Settings' Login screen page: what `kanade sddm status` finds, read inside the shell off the
//! draw thread and shown read-only beside the page's rows. Nothing here runs a command as root;
//! the commands root needs are shown to copy.

use std::thread;

use kanade_runtime::service::Service;

use crate::config;
use crate::sddm::{self, Status};

use super::Settings;

// the page's name
pub const PAGE: &str = "login";

/*
 * reads the status on a worker, if none runs, and puts it in the Service. Called when the page
 * shows, by its refresh link, and after a changed row is written
 */
pub fn load() {
    {
        let mut settings = Settings::write();
        if settings.login_loading {
            return;
        }
        settings.login_loading = true;
    }

    let worker = thread::Builder::new()
        .name(String::from("kanade-login"))
        .spawn(|| finish(sddm::overview(&config::get())));

    if let Err(error) = worker {
        finish(Err(format!("could not start the check: {error}")));
    }
}

fn finish(status: Result<Status, String>) {
    let mut settings = Settings::write();
    settings.login = Some(status);
    settings.login_loading = false;
}
