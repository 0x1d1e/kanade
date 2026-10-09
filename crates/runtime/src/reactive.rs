//! Native, event-driven Service invalidation.
//!
//! A view records which types of state it reads. Publishing a new value
//! schedules only the windows that consumed that type. There is no polling,
//! view I/O, thread wakeup while idle, or retained widget state to synchronize.
//! This replaces Amane's Service read/write subscription bookkeeping, not
//! Kanade's domain objects.
use std::{
    any::TypeId,
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
};

use crate::window::WindowId;

#[derive(Default)]
pub struct Reactive {
    observed: BTreeMap<WindowId, HashSet<TypeId>>,
    readers: HashMap<TypeId, BTreeSet<WindowId>>,
    drawing: Option<(WindowId, HashSet<TypeId>)>,
    dirty: BTreeSet<WindowId>,
}

impl Reactive {
    /// Called immediately before constructing a view, once per window.
    /// Nested views share this transaction; invoking begin twice is a bug.
    pub fn begin(&mut self, window: WindowId) -> Result<(), &'static str> {
        if self.drawing.is_some() {
            return Err("a view already owns the dependency transaction");
        }
        self.drawing = Some((window, HashSet::new()));
        Ok(())
    }

    /// Register only types actually read during this draw.
    pub fn observe<T: 'static>(&mut self) -> Result<(), &'static str> {
        let Some((_, observed)) = &mut self.drawing else {
            return Err("Service read outside a view transaction");
        };
        observed.insert(TypeId::of::<T>());
        Ok(())
    }

    /// Finalize dependencies; subscriptions omitted from this view stop
    /// invalidating its window. Hidden windows may unsubscribe entirely.
    pub fn end(&mut self) -> Result<(), &'static str> {
        let Some((window, new)) = self.drawing.take() else {
            return Err("no view is recording dependencies");
        };
        let old = self.observed.insert(window, new.clone()).unwrap_or_default();
        for removed in old.difference(&new) {
            if let Some(readers) = self.readers.get_mut(removed) {
                readers.remove(&window);
                if readers.is_empty() {
                    self.readers.remove(removed);
                }
            }
        }
        for added in new.difference(&old) {
            self.readers.entry(*added).or_default().insert(window);
        }
        Ok(())
    }

    /// A Source posts after its adapter has observed a real state change.
    /// Writing from a view is refused to prevent self-triggered redraw loops.
    pub fn publish<T: 'static>(&mut self) -> Result<(), &'static str> {
        if self.drawing.is_some() {
            return Err("Service writes are forbidden inside views");
        }
        if let Some(readers) = self.readers.get(&TypeId::of::<T>()) {
            self.dirty.extend(readers);
        }
        Ok(())
    }

    /// An input event/animation can invalidate exactly one target without
    /// pretending that a Service changed.
    pub fn invalidate(&mut self, window: WindowId) {
        self.dirty.insert(window);
    }

    /// Unsubscribe all types on window close, monitor unplug and module off.
    pub fn remove(&mut self, window: WindowId) {
        self.dirty.remove(&window);
        if let Some(old) = self.observed.remove(&window) {
            for ty in old {
                if let Some(readers) = self.readers.get_mut(&ty) {
                    readers.remove(&window);
                    if readers.is_empty() {
                        self.readers.remove(&ty);
                    }
                }
            }
        }
    }

    /// Deduplicated IDs passed to LayerRuntime::invalidate in the same event
    /// loop turn. Nothing to drain when everything is at rest.
    pub fn drain(&mut self) -> Vec<WindowId> {
        std::mem::take(&mut self.dirty).into_iter().collect()
    }

    pub fn has_work(&self) -> bool {
        !self.dirty.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Clock;
    struct Media;

    #[test]
    fn only_subscribers_receive_updates() {
        let mut rt = Reactive::default();
        let a = WindowId(1);
        let b = WindowId(2);
        rt.begin(a).unwrap();
        rt.observe::<Clock>().unwrap();
        rt.end().unwrap();
        rt.begin(b).unwrap();
        rt.observe::<Media>().unwrap();
        rt.end().unwrap();
        rt.publish::<Clock>().unwrap();
        rt.publish::<Clock>().unwrap();
        assert_eq!(rt.drain(), vec![a]);
        rt.publish::<Media>().unwrap();
        assert_eq!(rt.drain(), vec![b]);
    }

    #[test]
    fn dependencies_are_removed_when_view_no_longer_uses_them() {
        let mut rt = Reactive::default();
        let id = WindowId(7);
        rt.begin(id).unwrap();
        rt.observe::<Media>().unwrap();
        rt.end().unwrap();
        rt.begin(id).unwrap();
        rt.observe::<Clock>().unwrap();
        rt.end().unwrap();
        rt.publish::<Media>().unwrap();
        assert!(!rt.has_work());
        rt.publish::<Clock>().unwrap();
        assert_eq!(rt.drain(), vec![id]);
        rt.remove(id);
        rt.publish::<Clock>().unwrap();
        assert!(!rt.has_work());
    }

    #[test]
    fn view_cannot_mutate_services_or_nest_transactions() {
        let mut rt = Reactive::default();
        assert!(rt.observe::<Clock>().is_err());
        rt.begin(WindowId(1)).unwrap();
        assert!(rt.begin(WindowId(2)).is_err());
        assert!(rt.publish::<Clock>().is_err());
        rt.end().unwrap();
        assert!(rt.end().is_err());
    }

    #[test]
    fn explicit_invalidations_coalesce_and_close_discards_work() {
        let mut rt = Reactive::default();
        rt.invalidate(WindowId(1));
        rt.invalidate(WindowId(1));
        rt.invalidate(WindowId(2));
        rt.remove(WindowId(2));
        assert_eq!(rt.drain(), vec![WindowId(1)]);
        assert!(!rt.has_work());
    }
}
