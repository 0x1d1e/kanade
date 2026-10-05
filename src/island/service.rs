use amane::Service;

// the one Amane-facing piece of island/: owns the Arbiter and per-monitor Presentation
pub struct IslandService;

impl Service for IslandService {
    fn new() -> Self {
        Self
    }

    // changes only through input and sources, nothing to poll
    fn listen() {}
}
