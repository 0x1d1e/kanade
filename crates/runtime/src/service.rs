//! Kanade's shared state: one value per type, read by views, written by
//! Sources and input handlers.
//!
//! Each Service lives until the program exits and listens on its own thread,
//! started on first use and started again 5 s after a panic. A view's read is
//! noted, and a finished write draws again the windows whose last view read it; a
//! Service read and written by part (`read_part`, `Write::part`) draws only the windows that read
//! the part written.
//! What `watch`es a Service is derived from it by the runtime, in order, before
//! the next draw (`derive`), not by the writer.
use std::any::{self, Any, TypeId};
use std::collections::HashMap;
use std::hash::Hash;
use std::ops::{Deref, DerefMut};
use std::panic;
use std::sync::{LazyLock, Mutex, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::thread;
use std::time::Duration;

use crate::{changes, timing};

pub trait Service: Send + Sync + Sized + 'static {
    fn new() -> Self;

    fn interval() -> Duration {
        Duration::from_secs(1)
    }

    /// Reads the source again and says whether anything a window shows changed.
    fn update(&mut self) -> bool {
        false
    }

    /// Runs on the Service's own thread, so waiting here never stalls drawing.
    /// Event-driven Services replace it with their own loop, Services changed
    /// only by input with an empty one.
    fn listen() {
        loop {
            thread::sleep(Self::interval());

            let mut service = Self::write();

            // a poll that found nothing new redraws nothing
            if !service.update() {
                service.quiet();
            }
        }
    }

    fn read() -> RwLockReadGuard<'static, Self> {
        changes::note_read(TypeId::of::<Self>());

        find::<Self>()
            .read()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Reads one part of the Service that `key` names. A write of that part
    /// (`Write::part`) draws the window again, and so does one that names no
    /// part, as it may have changed any; a write of another part does not.
    fn read_part(key: &impl Hash) -> RwLockReadGuard<'static, Self> {
        changes::note_read_scope(changes::scope::<Self>(key));

        find::<Self>()
            .read()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// For Sources and input handlers. Inside a view a read of the same
    /// Service is still held, so this would wait forever.
    fn write() -> Write<Self> {
        let guard = find::<Self>()
            .write()
            .unwrap_or_else(PoisonError::into_inner);

        Write {
            guard: Some(guard),
            quiet: false,
            parts: Vec::new(),
        }
    }
}

pub struct Write<S: 'static> {
    // let go of before what watches the Service runs, so that may read it
    guard: Option<RwLockWriteGuard<'static, S>>,

    // the write changed nothing a window shows
    quiet: bool,

    // the parts it changed, as far as it says: it draws only the windows that read those
    parts: Vec<changes::Scope>,
}

impl<S: 'static> Write<S> {
    /// This write changed what `key` names, which windows read with
    /// `read_part`. A write that names parts, and only those, draws the
    /// windows that read them and the ones that read the whole Service.
    pub fn part(&mut self, key: &impl Hash) {
        self.parts.push(changes::scope::<S>(key));
    }
}

impl<S> Write<S> {
    /// This write changed nothing a window shows, so it redraws nothing.
    pub fn quiet(&mut self) {
        self.quiet = true;
    }
}

impl<S> Deref for Write<S> {
    type Target = S;

    fn deref(&self) -> &S {
        self.guard.as_ref().expect("written after the write ended")
    }
}

impl<S> DerefMut for Write<S> {
    fn deref_mut(&mut self) -> &mut S {
        self.guard.as_mut().expect("written after the write ended")
    }
}

impl<S: 'static> Drop for Write<S> {
    fn drop(&mut self) {
        drop(self.guard.take());

        if self.quiet {
            return;
        }

        // with KANADE_FRAMES, the frame log also says which Service woke the windows
        if timing::enabled() {
            eprintln!("change {}", any::type_name::<S>());
        }

        // a write that panicked halfway leaves the rest to the restart
        if !thread::panicking() {
            let mut pending = PENDING.lock().unwrap_or_else(PoisonError::into_inner);

            if !pending.contains(&TypeId::of::<S>()) {
                pending.push(TypeId::of::<S>());
            }
        }

        // after the derive is pending, so the wake that runs it finds it
        if self.parts.is_empty() {
            changes::mark(TypeId::of::<S>());
        } else {
            changes::mark_scopes(std::mem::take(&mut self.parts));
        }
    }
}

// what `watch` runs, in the order it was asked
static WATCHERS: Mutex<Vec<Watcher>> = Mutex::new(Vec::new());

type Watcher = (TypeId, fn());

// the Services written since `derive` last ran
static PENDING: Mutex<Vec<TypeId>> = Mutex::new(Vec::new());

// how many rounds of derived writes `derive` follows before it leaves the rest to the next wake
const PASSES: usize = 16;

/// Runs `then` before the next draw after any write of `S` that changed
/// something a window shows, on the runtime's thread, so what follows from a
/// Service's change is worked out once, wherever it was written, by code the
/// writer does not know. Derived updates run in the order they were watched,
/// so one that reads what another derives asks to come after it. `then` may
/// write Services, which derives again, and must not block.
pub fn watch<S: Service>(then: fn()) {
    WATCHERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push((TypeId::of::<S>(), then));
}

/// Runs what `watch`es each Service written since it last ran, until the
/// derived writes settle. The runtime calls it before it looks at what
/// changed and before a window's view reads, so a draw shows every derived
/// value. A derived update that panics is reported and the shell goes on.
pub fn derive() {
    for _ in 0..PASSES {
        let written = std::mem::take(&mut *PENDING.lock().unwrap_or_else(PoisonError::into_inner));

        if written.is_empty() {
            return;
        }

        let then: Vec<fn()> = WATCHERS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|(service, _)| written.contains(service))
            .map(|&(_, then)| then)
            .collect();

        for then in then {
            if panic::catch_unwind(then).is_err() {
                eprintln!("kanade: a derived update panicked, going on without it");
            }
        }
    }

    // a cycle of watchers: what is left runs after the next write, the shell stays responsive
    eprintln!("kanade: derived updates did not settle in {PASSES} rounds");
}

type Store = HashMap<TypeId, &'static (dyn Any + Send + Sync)>;

static SERVICES: LazyLock<Mutex<Store>> = LazyLock::new(|| Mutex::new(HashMap::new()));

// how long a Service that panicked waits before it listens again
const RESTART_DELAY: Duration = Duration::from_secs(5);

fn find<S: Service>() -> &'static RwLock<S> {
    let id = TypeId::of::<S>();

    if let Some(service) = SERVICES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&id)
    {
        return service.downcast_ref().expect("failed to find service");
    }

    // made outside the lock, as a Service's new may read another Service
    let made = RwLock::new(S::new());

    let mut services = SERVICES.lock().unwrap_or_else(PoisonError::into_inner);

    // another thread made it meanwhile: theirs is the one, listening already
    if let Some(service) = services.get(&id) {
        return service.downcast_ref().expect("failed to find service");
    }

    // Services stay until the program exits, so leaking gives a reference valid forever
    let service: &'static RwLock<S> = Box::leak(Box::new(made));

    services.insert(id, service);

    drop(services);

    // stored first, so the thread's own reads and writes find this same Service
    thread::spawn(keep_listening::<S>);

    service
}

// a listen that panics, like on a bus that went away, starts again after a
// pause instead of leaving the Service frozen; one that returns is done
fn keep_listening<S: Service>() {
    while panic::catch_unwind(S::listen).is_err() {
        eprintln!(
            "kanade: {} stopped, starting it again",
            any::type_name::<S>()
        );

        thread::sleep(RESTART_DELAY);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    #[derive(Default)]
    struct Counter(u32);

    impl Service for Counter {
        fn new() -> Self {
            Self::default()
        }

        fn listen() {}
    }

    #[derive(Default)]
    struct Watched(u32);

    impl Service for Watched {
        fn new() -> Self {
            Self::default()
        }

        fn listen() {}
    }

    // `derive` takes every pending write, so the tests that call it take turns
    static DERIVING: Mutex<()> = Mutex::new(());

    static SEEN: Mutex<Vec<u32>> = Mutex::new(Vec::new());

    // reads what was written, once the write let go of it
    fn seen() {
        SEEN.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Watched::read().0);
    }

    #[test]
    fn a_watcher_runs_on_derive_after_a_write_but_not_a_quiet_one() {
        let _turn = DERIVING.lock().unwrap_or_else(PoisonError::into_inner);

        watch::<Watched>(seen);

        Watched::write().0 = 1;

        // nothing runs in the writer
        assert!(
            SEEN.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .is_empty()
        );

        // the writes since the last derive run it once, seeing the latest
        Watched::write().0 = 2;
        derive();
        derive();

        assert_eq!(*SEEN.lock().unwrap_or_else(PoisonError::into_inner), [2]);

        let mut quiet = Watched::write();
        quiet.0 = 3;
        quiet.quiet();
        drop(quiet);
        derive();

        assert_eq!(*SEEN.lock().unwrap_or_else(PoisonError::into_inner), [2]);
    }

    #[derive(Default)]
    struct Source(u32);

    impl Service for Source {
        fn new() -> Self {
            Self::default()
        }

        fn listen() {}
    }

    #[derive(Default)]
    struct Middle(u32);

    impl Service for Middle {
        fn new() -> Self {
            Self::default()
        }

        fn listen() {}
    }

    #[derive(Default)]
    struct Last(u32);

    impl Service for Last {
        fn new() -> Self {
            Self::default()
        }

        fn listen() {}
    }

    // each derived value comes from the one before it, in the order watched, however it is written
    #[test]
    fn derived_writes_settle_in_order() {
        let _turn = DERIVING.lock().unwrap_or_else(PoisonError::into_inner);

        watch::<Source>(|| Middle::write().0 = Source::read().0 * 2);
        watch::<Middle>(|| Last::write().0 = Middle::read().0 + 1);

        Source::write().0 = 5;
        derive();

        assert_eq!(Middle::read().0, 10);
        assert_eq!(Last::read().0, 11);
    }

    #[derive(Default)]
    struct Broken(u32);

    impl Service for Broken {
        fn new() -> Self {
            Self::default()
        }

        fn listen() {}
    }

    #[derive(Default)]
    struct After(u32);

    impl Service for After {
        fn new() -> Self {
            Self::default()
        }

        fn listen() {}
    }

    // a derived update that panics must not take the runtime's thread down with it
    #[test]
    fn a_derived_update_that_panics_leaves_the_rest() {
        let _turn = DERIVING.lock().unwrap_or_else(PoisonError::into_inner);

        watch::<Broken>(|| panic!("a derived update that fails"));
        watch::<Broken>(|| After::write().0 = 7);

        Broken::write().0 = 1;
        derive();

        assert_eq!(After::read().0, 7);
    }

    #[test]
    fn reads_after_a_write_panicked() {
        let failed = thread::spawn(|| {
            let _counter = Counter::write();

            panic!("a write that fails halfway");
        });

        assert!(failed.join().is_err());

        assert_eq!(Counter::read().0, 0);
    }

    struct Flaky;

    static LISTENS: Mutex<u32> = Mutex::new(0);

    impl Service for Flaky {
        fn new() -> Self {
            Self
        }

        fn listen() {
            let mut listens = LISTENS.lock().unwrap_or_else(PoisonError::into_inner);
            *listens += 1;

            if *listens == 1 {
                drop(listens);
                panic!("a bus that went away");
            }
        }
    }

    #[test]
    fn a_listen_that_panicked_starts_again() {
        let (done, wait) = mpsc::channel();

        thread::spawn(move || {
            keep_listening::<Flaky>();
            done.send(()).expect("failed to report");
        });

        wait.recv_timeout(RESTART_DELAY * 2)
            .expect("listen did not start again");

        assert_eq!(*LISTENS.lock().unwrap_or_else(PoisonError::into_inner), 2);
    }
}
