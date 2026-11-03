//! Umbriel starts bare `kanade`; this parent owns one shell and its ephemeral lock intent.
//! No Wayland connection, Modules, passwords or persisted lock state live here (ADR 0149).

use std::env;
use std::io::{self, Read, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Child, Command, ExitCode, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::{clock, sources};

pub const CHILD: &str = "--umbriel-shell";
const ACK: u8 = b'A';
const LOCK: u8 = b'L';
const CLEAR: u8 = b'U';
const TIMEOUT: Duration = Duration::from_secs(2);
const STOP: Duration = Duration::from_secs(6);
const MAX_BACKOFF: Duration = Duration::from_secs(8);

struct Inherited {
    channel: Mutex<UnixStream>,
    relock: bool,
    parent: u32,
}

static INHERITED: OnceLock<Inherited> = OnceLock::new();

// Only the private child enters the runtime. stdin is a private socket pair installed by its
// parent, not an environment variable or a public IPC verb; all other commands keep their stdin.
pub fn enter_child() -> io::Result<()> {
    let (mut channel, parent) = clock::recovery_child_channel()?;
    channel.set_read_timeout(Some(TIMEOUT))?;
    channel.set_write_timeout(Some(TIMEOUT))?;
    let relock = match read_byte(&mut channel)? {
        LOCK => true,
        CLEAR => false,
        _ => return Err(io::Error::other("invalid recovery greeting")),
    };
    channel.write_all(&[ACK])?;
    INHERITED
        .set(Inherited {
            channel: Mutex::new(channel),
            relock,
            parent,
        })
        .map_err(|_| io::Error::other("recovery child already initialized"))
}

pub fn relock() -> bool {
    INHERITED.get().is_some_and(|child| child.relock)
}

pub fn status() -> Option<String> {
    INHERITED
        .get()
        .map(|child| format!("recovery: Umbriel parent pid {}", child.parent))
}

// Called under the lock requests mutex, before asking the compositor or after a confirmed end.
// Losing the checkpoint is fatal: an untracked new lock must never reach the compositor.
pub fn checkpoint(locked: bool) {
    let Some(child) = INHERITED.get() else {
        return;
    };
    let mut channel = child.channel.lock().unwrap_or_else(|e| e.into_inner());
    if let Err(error) = exchange(&mut channel, locked) {
        eprintln!("kanade: recovery lock checkpoint failed: {error}");
        std::process::exit(1);
    }
}

fn exchange(channel: &mut UnixStream, locked: bool) -> io::Result<()> {
    channel.write_all(&[if locked { LOCK } else { CLEAR }])?;
    if read_byte(channel)? != ACK {
        return Err(io::Error::other("invalid recovery acknowledgement"));
    }
    Ok(())
}

// None preserves the existing niri/service path, before allocator/runtime/Modules initialization.
pub fn start() -> Option<ExitCode> {
    if sources::compositor::active() != sources::compositor::Compositor::Umbriel {
        return None;
    }
    let result = (|| {
        let runtime = env::var_os("XDG_RUNTIME_DIR")
            .ok_or_else(|| io::Error::other("XDG_RUNTIME_DIR is not set"))?;
        let owner = Path::new(&runtime).join("kanade-recovery.owner");
        let socket = sources::umbriel::socket_path()
            .ok_or_else(|| io::Error::other("Umbriel IPC socket disappeared"))?;
        let executable = env::current_exe()?;
        supervise(&socket, &owner, || {
            let mut command = Command::new(&executable);
            command.arg(CHILD);
            command
        })
    })();
    Some(match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("kanade: Umbriel recovery: {error}");
            ExitCode::FAILURE
        }
    })
}

fn supervise(socket: &Path, owner: &Path, mut command: impl FnMut() -> Command) -> io::Result<()> {
    let _owner = clock::recovery_owner(owner)?;
    let compositor = clock::ipc_process(socket)?;
    let termination = clock::catch_termination()?;
    clock::name_recovery_parent()?;
    let mut locked = false;
    let mut backoff = Duration::from_millis(250);

    loop {
        // Includes the first launch and every backoff boundary: a stop never launches a new shell.
        if stopping(&termination, &compositor, Duration::ZERO)? {
            return Ok(());
        }
        // An older/direct shell owns the runtime's singleton; do not create a restart loop around it.
        if UnixStream::connect(kanade_runtime::ipc_socket()).is_ok() {
            return Err(io::Error::other("kanade is already running"));
        }
        let (mut parent, child_channel) = UnixStream::pair()?;
        parent.set_read_timeout(Some(TIMEOUT))?;
        parent.set_write_timeout(Some(TIMEOUT))?;
        let mut launch = command();
        launch.stdin(Stdio::from(OwnedFd::from(child_channel)));
        clock::die_with_parent(&mut launch);
        let began = Instant::now();
        let mut child = launch.spawn()?;
        let result = watch(
            &mut child,
            &mut parent,
            &termination,
            &compositor,
            &mut locked,
        );
        // All error/stop paths reap the child before releasing singleton ownership.
        if !matches!(result, Ok(false)) {
            stop(&mut child);
        }
        if result? {
            return Ok(());
        }
        if began.elapsed() >= Duration::from_secs(30) {
            backoff = Duration::from_millis(250);
        }
        let status = child.wait()?;
        eprintln!("kanade: shell exited with {status}; restarting in {backoff:?}");
        if stopping(&termination, &compositor, backoff)? {
            return Ok(());
        }
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

// true means stop; false means the child exited and may restart. The parent never waits on a
// blocking handshake: termination/compositor death can interrupt startup and every restart delay.
fn watch(
    child: &mut Child,
    channel: &mut UnixStream,
    termination: &UnixStream,
    compositor: &OwnedFd,
    locked: &mut bool,
) -> io::Result<bool> {
    let child_fd = clock::process_fd(child.id())?;
    let mut connected = channel
        .write_all(&[if *locked { LOCK } else { CLEAR }])
        .is_ok();
    let mut greeting = true;
    loop {
        let events = clock::wait_readable(
            &[
                termination.as_fd(),
                compositor.as_fd(),
                child_fd.as_fd(),
                if connected {
                    channel.as_fd()
                } else {
                    child_fd.as_fd()
                },
            ],
            None,
        )?;
        if events[0] || events[1] {
            return Ok(true);
        }
        // Drain channel bytes before reaping, including checkpoints sent just before child death.
        if connected && events[3] {
            match read_byte(channel) {
                Ok(ACK) if greeting => greeting = false,
                Ok(byte @ (LOCK | CLEAR)) if !greeting => {
                    *locked = byte == LOCK;
                    connected = channel.write_all(&[ACK]).is_ok();
                }
                Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => connected = false,
                Err(error) => return Err(error),
                _ => return Err(io::Error::other("invalid child lock checkpoint")),
            }
        }
        if events[2] {
            // Child is dead; buffered channel updates were acknowledged before any lock could be
            // requested. An unacknowledged clear can only cause a conservative extra relock.
            child.wait()?;
            return Ok(false);
        }
    }
}

fn stopping(termination: &UnixStream, compositor: &OwnedFd, wait: Duration) -> io::Result<bool> {
    Ok(
        clock::wait_readable(&[termination.as_fd(), compositor.as_fd()], Some(wait))?
            .into_iter()
            .any(|ready| ready),
    )
}

fn read_byte(channel: &mut UnixStream) -> io::Result<u8> {
    let mut byte = [0];
    channel.read_exact(&mut byte)?;
    Ok(byte[0])
}

fn stop(child: &mut Child) {
    if child.try_wait().ok().flatten().is_some() {
        return;
    }
    let _ = clock::terminate_pid(child.id());
    let deadline = Instant::now() + STOP;
    while child.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    if child.try_wait().ok().flatten().is_none() {
        let _ = child.kill();
    }
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File, OpenOptions};
    use std::os::unix::net::UnixListener;
    use std::path::PathBuf;
    use std::sync::atomic::Ordering;

    const FIXTURE: &str = "KANADE_RECOVERY_FIXTURE";
    const TEST: &str = "recovery::tests::fixture";

    fn fixture_command(kind: &str, dir: &Path) -> Command {
        let mut command = Command::new(env::current_exe().unwrap());
        command.args(["--exact", TEST, "--nocapture"]);
        command.env(FIXTURE, kind).env("KANADE_RECOVERY_DIR", dir);
        command.env("XDG_RUNTIME_DIR", dir);
        command.stdout(Stdio::null());
        command
    }

    // Runs only in disposable subprocesses. No real compositor, renderer, lock, bus or user state.
    #[test]
    fn fixture() {
        let Ok(kind) = env::var(FIXTURE) else { return };
        let dir = PathBuf::from(env::var_os("KANADE_RECOVERY_DIR").unwrap());
        match kind.as_str() {
            "compositor" => {
                let listener = UnixListener::bind(dir.join("umbriel.sock")).unwrap();
                for stream in listener.incoming() {
                    drop(stream.unwrap());
                }
            }
            "parent" => {
                let result = supervise(&dir.join("umbriel.sock"), &dir.join("owner"), || {
                    fixture_command("child", &dir)
                });
                if let Err(error) = result {
                    eprintln!("{error}");
                    std::process::exit(1);
                }
            }
            "child" => {
                let behavior = fs::read_to_string(dir.join("behavior")).unwrap();
                if behavior == "before-handshake" {
                    fs::write(
                        dir.join("children"),
                        format!("{} false\n", std::process::id()),
                    )
                    .unwrap();
                    loop {
                        std::thread::park();
                    }
                }
                enter_child().unwrap();
                let log = dir.join("children");
                let first = !log.exists();
                let mut log = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(log)
                    .unwrap();
                writeln!(log, "{} {}", std::process::id(), relock()).unwrap();
                if behavior == "ignore-term" {
                    use std::os::unix::process::CommandExt;
                    let error = Command::new("sh")
                        .args(["-c", "trap '' TERM; : > \"$1\"; exec sleep 60", "fixture"])
                        .arg(dir.join("ignoring-term"))
                        .exec();
                    panic!("fixture exec failed: {error}");
                }
                if first && behavior.contains("lock") {
                    checkpoint(true);
                    fs::write(dir.join("checkpoint"), "acknowledged").unwrap();
                    if behavior.contains("clear") {
                        checkpoint(false);
                    }
                }
                if first && (behavior.contains("crash") || behavior == "clean-exit") {
                    std::process::exit(if behavior == "clean-exit" { 0 } else { 17 });
                }
                loop {
                    std::thread::park();
                }
            }
            _ => panic!("unknown fixture"),
        }
    }

    struct World {
        dir: PathBuf,
        compositor: Child,
        parent: Option<Child>,
    }

    impl World {
        fn new(behavior: &str) -> Self {
            static SERIAL: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let dir = env::temp_dir().join(format!(
                "kanade-recovery-{}-{}",
                std::process::id(),
                SERIAL.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&dir).unwrap();
            fs::write(dir.join("behavior"), behavior).unwrap();
            let compositor = fixture_command("compositor", &dir).spawn().unwrap();
            until(|| dir.join("umbriel.sock").exists());
            Self {
                dir,
                compositor,
                parent: None,
            }
        }

        fn start(&mut self) {
            let log = File::create(self.dir.join("parent-log")).unwrap();
            self.parent = Some(
                fixture_command("parent", &self.dir)
                    .stderr(log)
                    .spawn()
                    .unwrap(),
            );
        }

        fn children(&self) -> Vec<(u32, bool)> {
            fs::read_to_string(self.dir.join("children"))
                .unwrap_or_default()
                .lines()
                .filter_map(|line| {
                    let (pid, locked) = line.split_once(' ')?;
                    Some((pid.parse().ok()?, locked.parse().ok()?))
                })
                .collect()
        }

        fn launched(&self, count: usize) -> Vec<(u32, bool)> {
            until(|| self.children().len() >= count);
            self.children()
        }

        fn delayed(&self) {
            until(|| {
                fs::read_to_string(self.dir.join("parent-log"))
                    .unwrap_or_default()
                    .contains("restarting in")
            });
        }

        fn stop_parent(&mut self) {
            let parent = self.parent.as_mut().unwrap();
            clock::terminate_pid(parent.id()).unwrap();
            until(|| parent.try_wait().unwrap().is_some());
            assert!(parent.wait().unwrap().success());
        }

        fn no_children_alive(&self) {
            for (pid, _) in self.children() {
                assert!(
                    !Path::new(&format!("/proc/{pid}")).exists(),
                    "child {pid} was not reaped"
                );
            }
        }
    }

    impl Drop for World {
        fn drop(&mut self) {
            if let Some(parent) = &mut self.parent
                && parent.try_wait().ok().flatten().is_none()
            {
                let _ = clock::terminate_pid(parent.id());
                let deadline = Instant::now() + Duration::from_secs(8);
                while parent.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(10));
                }
                let _ = parent.kill();
                let _ = parent.wait();
            }
            let _ = self.compositor.kill();
            let _ = self.compositor.wait();
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    fn until(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !condition() {
            assert!(Instant::now() < deadline, "fixture timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn shell_exit_restarts_once_and_duplicate_parent_cannot_launch() {
        let mut world = World::new("crash");
        world.start();
        let children = world.launched(2);
        assert_eq!(children.len(), 2);
        assert_ne!(children[0].0, children[1].0);
        assert!(!Path::new(&format!("/proc/{}", children[0].0)).exists());
        let result = fixture_command("parent", &world.dir).output().unwrap();
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("owner already running"));
        assert_eq!(world.children().len(), 2);
        let parent = world.parent.as_ref().unwrap().id();
        assert_eq!(
            fs::read_to_string(format!("/proc/{parent}/comm"))
                .unwrap()
                .trim(),
            "kanade-recover"
        );
        world.stop_parent();
        world.no_children_alive();
    }

    #[test]
    fn a_clean_child_exit_also_restarts() {
        let mut world = World::new("clean-exit");
        world.start();
        assert_eq!(world.launched(2).len(), 2);
        world.stop_parent();
        world.no_children_alive();
    }

    #[test]
    fn an_existing_direct_shell_prevents_a_restart_loop() {
        let world = World::new("wait");
        let _old_shell = UnixListener::bind(world.dir.join("kanade.sock")).unwrap();
        let result = fixture_command("parent", &world.dir).output().unwrap();
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("kanade is already running"));
        assert!(world.children().is_empty());
    }

    #[test]
    fn compositor_death_and_parent_termination_reap_the_only_child() {
        for (compositor_dies, behavior) in [
            (true, "wait"),
            (false, "wait"),
            (true, "before-handshake"),
            (false, "before-handshake"),
        ] {
            let mut world = World::new(behavior);
            world.start();
            world.launched(1);
            if compositor_dies {
                world.compositor.kill().unwrap();
                world.compositor.wait().unwrap();
                let parent = world.parent.as_mut().unwrap();
                until(|| parent.try_wait().unwrap().is_some());
                assert!(parent.wait().unwrap().success());
            } else {
                world.stop_parent();
            }
            world.no_children_alive();
            assert_eq!(world.children().len(), 1);
        }
    }

    #[test]
    fn termination_escalates_and_reaps_a_child_ignoring_sigterm() {
        let mut world = World::new("ignore-term");
        world.start();
        world.launched(1);
        until(|| world.dir.join("ignoring-term").exists());
        world.stop_parent();
        world.no_children_alive();
        assert_eq!(world.children().len(), 1);
    }

    #[test]
    fn abrupt_parent_death_kills_its_child_without_starting_another() {
        let mut world = World::new("wait");
        world.start();
        let child = world.launched(1)[0].0;
        let child_fd = clock::process_fd(child).unwrap();
        let parent = world.parent.as_mut().unwrap();
        parent.kill().unwrap();
        parent.wait().unwrap();
        assert!(clock::wait_readable(&[child_fd.as_fd()], Some(TIMEOUT)).unwrap()[0]);
        assert_eq!(world.children().len(), 1);
    }

    #[test]
    fn stop_and_compositor_death_interrupt_restart_delay() {
        for compositor_dies in [true, false] {
            let mut world = World::new("crash");
            world.start();
            world.delayed();
            if compositor_dies {
                world.compositor.kill().unwrap();
                world.compositor.wait().unwrap();
                let parent = world.parent.as_mut().unwrap();
                until(|| parent.try_wait().unwrap().is_some());
                assert!(parent.wait().unwrap().success());
            } else {
                world.stop_parent();
            }
            assert_eq!(world.children().len(), 1);
            world.no_children_alive();
        }
    }

    #[test]
    fn acknowledged_intent_survives_crash_and_confirmed_clear_ends_it() {
        for (behavior, expected) in [("lock-crash", true), ("lock-clear-crash", false)] {
            let mut world = World::new(behavior);
            world.start();
            let children = world.launched(2);
            assert!(world.dir.join("checkpoint").exists());
            assert!(!children[0].1);
            assert_eq!(
                children[1].1, expected,
                "replacement relock decision for {behavior}"
            );
            world.stop_parent();
            world.no_children_alive();
        }
    }

    #[test]
    fn lock_checkpoint_waits_for_acknowledgement_and_rejects_eof() {
        let (mut parent, mut child) = UnixStream::pair().unwrap();
        let (done, finished) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            done.send(exchange(&mut child, true)).unwrap();
        });
        assert_eq!(read_byte(&mut parent).unwrap(), LOCK);
        assert!(
            finished.try_recv().is_err(),
            "lock request must wait until parent records intent"
        );
        parent.write_all(&[ACK]).unwrap();
        assert!(finished.recv_timeout(TIMEOUT).unwrap().is_ok());
        worker.join().unwrap();
        let (parent, mut child) = UnixStream::pair().unwrap();
        drop(parent);
        assert!(exchange(&mut child, true).is_err());
    }
}
