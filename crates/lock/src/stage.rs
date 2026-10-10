//! Where the lock stands with niri, without the lock itself: what the lock's thread does on each
//! ask of the shell and each event of niri's. Pure, so the shell's tests run it with its requests.

use crate::News;

// a lock asked for or held serves the newest request asked of it
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Stage {
    #[default]
    Unlocked,

    // asked, not yet held; an unlock asked meanwhile is `owed` until niri holds it
    Locking {
        owed: bool,
        request: u64,
    },

    Locked {
        request: u64,
    },
}

// what the lock's thread does after a step
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Nothing,

    // asks niri for a lock, with a surface per output; one niri never got is `refused`
    Ask,

    // tells the shell
    Tell(News),

    // ends the lock (`unlock`, a no-op unless niri holds it), drops it and its surfaces, and tells
    End(News),
}

impl Stage {
    // the shell asks for a lock for `request`
    pub fn lock(&mut self, request: u64) -> Step {
        match *self {
            Stage::Locking { .. } => {
                *self = Stage::Locking {
                    owed: false,
                    request,
                };
                Step::Nothing
            }
            Stage::Locked { .. } => {
                *self = Stage::Locked { request };
                Step::Tell(News::Locked(request))
            }
            Stage::Unlocked => {
                *self = Stage::Locking {
                    owed: false,
                    request,
                };
                Step::Ask
            }
        }
    }

    // the lock `Ask` asked for could not be, so niri never sees it
    pub fn refused(&mut self) -> Step {
        match std::mem::take(self) {
            Stage::Locking { request, .. } => Step::Tell(News::Finished(request)),
            stage => {
                *self = stage;
                Step::Nothing
            }
        }
    }

    // the shell asks the lock to end, once niri holds it
    pub fn unlock(&mut self) -> Step {
        match *self {
            Stage::Locking { request, .. } => {
                *self = Stage::Locking {
                    owed: true,
                    request,
                };
                Step::Nothing
            }
            Stage::Locked { request } => {
                *self = Stage::Unlocked;
                Step::End(News::Unlocked(request))
            }
            Stage::Unlocked => Step::Nothing,
        }
    }

    // niri holds the lock; an unlock asked meanwhile ends it at once, so the shell hears only that
    pub fn locked(&mut self) -> Step {
        match *self {
            Stage::Locking {
                owed: true,
                request,
            } => {
                *self = Stage::Unlocked;
                Step::End(News::Unlocked(request))
            }
            Stage::Locking {
                owed: false,
                request,
            } => {
                *self = Stage::Locked { request };
                Step::Tell(News::Locked(request))
            }
            Stage::Locked { .. } | Stage::Unlocked => Step::Nothing,
        }
    }

    // niri refused the lock, or ended it
    pub fn finished(&mut self) -> Step {
        match std::mem::take(self) {
            Stage::Locking { request, .. } | Stage::Locked { request } => {
                Step::End(News::Finished(request))
            }
            Stage::Unlocked => Step::Nothing,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unlock_asked_while_locking_ends_the_lock_as_niri_holds_it() {
        let mut stage = Stage::default();
        assert_eq!(stage.lock(1), Step::Ask);
        assert_eq!(stage.unlock(), Step::Nothing);
        assert_eq!(stage.locked(), Step::End(News::Unlocked(1)));
        assert_eq!(stage, Stage::Unlocked);
    }

    #[test]
    fn a_lock_asked_while_locking_drops_the_unlock_owed() {
        let mut stage = Stage::default();
        stage.lock(1);
        stage.unlock();
        assert_eq!(stage.lock(2), Step::Nothing);
        assert_eq!(stage.locked(), Step::Tell(News::Locked(2)));
        assert_eq!(stage.lock(3), Step::Tell(News::Locked(3)));
        assert_eq!(stage.finished(), Step::End(News::Finished(3)));
        assert_eq!(stage.locked(), Step::Nothing);
    }

    #[test]
    fn a_lock_that_could_not_be_asked_is_finished() {
        let mut stage = Stage::default();
        stage.lock(4);
        assert_eq!(stage.refused(), Step::Tell(News::Finished(4)));
        assert_eq!(stage, Stage::Unlocked);
    }
}
