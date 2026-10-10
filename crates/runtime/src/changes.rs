use std::any::TypeId;
use std::cell::RefCell;
use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock, PoisonError};

use crate::timing;

// the backend sets this once, so services never name the event loop's types
static WAKE: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

/*
 * which services changed since the windows last looked, and whether
 * something changed that no service names, which every window may show
 */
static CHANGED: Mutex<Changes> = Mutex::new(Changes {
    changed: None,
    everything: false,
});

struct Changes {
    changed: Option<Changed>,
    everything: bool,
}

/*
 * a part of a service some windows show and others do not, like one pane's copy of the screen:
 * a hash of whatever names it, so two names that collide only draw a window needlessly
 */
pub type Scope = (TypeId, u64);

// the part of a service a key names
pub fn scope<S: 'static>(key: &impl Hash) -> Scope {
    let mut hasher = DefaultHasher::new();

    key.hash(&mut hasher);

    (TypeId::of::<S>(), hasher.finish())
}

/// What a window's view and widgets read: whole services, and parts of them.
#[derive(Debug, Default, Clone)]
pub struct Reads {
    services: HashSet<TypeId>,
    scopes: HashSet<Scope>,
}

impl Reads {
    pub fn extend(&mut self, more: Reads) {
        self.services.extend(more.services);
        self.scopes.extend(more.scopes);
    }
}

/// What changed since the windows last looked.
#[derive(Debug, Default)]
pub struct Changed {
    services: HashSet<TypeId>,
    scopes: HashSet<Scope>,
}

impl Changed {
    // whether a window that read `reads` shows something that changed
    pub fn touches(&self, reads: &Reads) -> bool {
        // a whole service changed, or a part of one that a whole read includes
        if !self.services.is_disjoint(&reads.services)
            || self
                .scopes
                .iter()
                .any(|(service, _)| reads.services.contains(service))
        {
            return true;
        }

        // a part read changed, or its whole service did
        !self.scopes.is_disjoint(&reads.scopes)
            || reads
                .scopes
                .iter()
                .any(|(service, _)| self.services.contains(service))
    }
}

// what the window being drawn right now has read
thread_local! {
    static READ: RefCell<Reads> = RefCell::new(Reads::default());
}

pub fn set_waker(wake: impl Fn() + Send + Sync + 'static) {
    if WAKE.set(Box::new(wake)).is_err() {
        panic!("failed to set wake: already set");
    }
}

// asks the event loop to draw every window again, from any thread
pub fn mark_all() {
    if timing::enabled() {
        eprintln!("change everything");
    }

    CHANGED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .everything = true;

    ping();
}

// only the windows that read this service are drawn again
pub fn mark(service: TypeId) {
    let mut changes = CHANGED.lock().unwrap_or_else(PoisonError::into_inner);

    changes
        .changed
        .get_or_insert_default()
        .services
        .insert(service);

    drop(changes);

    ping();
}

// only the windows that read these parts, or the whole service, are drawn again
pub fn mark_scopes(scopes: impl IntoIterator<Item = Scope>) {
    let mut changes = CHANGED.lock().unwrap_or_else(PoisonError::into_inner);

    changes
        .changed
        .get_or_insert_default()
        .scopes
        .extend(scopes);

    drop(changes);

    ping();
}

// none means every window should draw
pub fn take() -> Option<Changed> {
    let mut changes = CHANGED.lock().unwrap_or_else(PoisonError::into_inner);

    let changed = changes.changed.take().unwrap_or_default();

    if std::mem::take(&mut changes.everything) {
        return None;
    }

    Some(changed)
}

pub fn note_read(service: TypeId) {
    READ.with_borrow_mut(|read| read.services.insert(service));
}

// a read of one part of a service: only a change to it, or to the whole service, draws again
pub fn note_read_scope(scope: Scope) {
    READ.with_borrow_mut(|read| read.scopes.insert(scope));
}

// what was read since the last call, which starts a fresh list
pub fn take_read() -> Reads {
    READ.take()
}

fn ping() {
    // a write before the loop exists is picked up by the first frame anyway
    let Some(wake) = WAKE.get() else {
        return;
    };

    wake();
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Panes;
    struct Other;

    fn reads(services: &[TypeId], scopes: &[Scope]) -> Reads {
        Reads {
            services: services.iter().copied().collect(),
            scopes: scopes.iter().copied().collect(),
        }
    }

    fn changed(services: &[TypeId], scopes: &[Scope]) -> Changed {
        Changed {
            services: services.iter().copied().collect(),
            scopes: scopes.iter().copied().collect(),
        }
    }

    // a key names the same part whether it is held as a String or a str, as a monitor's name is
    #[test]
    fn a_part_is_the_same_for_a_string_and_a_str() {
        assert_eq!(
            scope::<Panes>(&String::from("eDP-1")),
            scope::<Panes>(&"eDP-1")
        );
    }

    // a part's change draws the windows that read that part or the whole, and no other
    #[test]
    fn a_part_changed_draws_only_who_read_it() {
        let (island, dock) = (scope::<Panes>(&"island"), scope::<Panes>(&"dock"));
        let change = changed(&[], &[island]);

        assert!(change.touches(&reads(&[], &[island])));
        assert!(change.touches(&reads(&[TypeId::of::<Panes>()], &[])));
        assert!(!change.touches(&reads(&[], &[dock])));
        assert!(!change.touches(&reads(
            &[TypeId::of::<Other>()],
            &[scope::<Other>(&"island")]
        )));
    }

    #[test]
    fn a_whole_service_changed_draws_who_read_a_part() {
        let change = changed(&[TypeId::of::<Panes>()], &[]);

        assert!(change.touches(&reads(&[], &[scope::<Panes>(&"dock")])));
        assert!(!change.touches(&reads(&[], &[scope::<Other>(&"dock")])));
    }
}
