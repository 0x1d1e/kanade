//! State a Surface keeps for one visit (`IslandService::visit`), so every opening starts afresh.

// a Service whose state belongs to one visit
pub trait Visited: Clone {
    fn visit(&self) -> u64;

    // the state of a new `visit`
    fn begin(visit: u64) -> Self;

    // this visit's state; one kept from an earlier visit is over
    fn of(&self, visit: u64) -> Self {
        if self.visit() == visit {
            self.clone()
        } else {
            Self::begin(visit)
        }
    }
}
