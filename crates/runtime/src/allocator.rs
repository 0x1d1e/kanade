use std::ffi::c_int;
use std::sync::Once;
use std::thread;
use std::time::Duration;

// glibc's mallopt settings: the size served by its own mapping, and how many pools threads share
const M_MMAP_THRESHOLD: c_int = -3;
const M_ARENA_MAX: c_int = -8;

const ONE_MEGABYTE: c_int = 1024 * 1024;

const POOLS: c_int = 2;

// how often freed memory inside the pools is handed back
const TRIM: Duration = Duration::from_secs(10);

static LIMIT: Once = Once::new();

unsafe extern "C" {
    fn mallopt(param: c_int, value: c_int) -> c_int;

    fn malloc_trim(pad: usize) -> c_int;
}

/*
 * glibc raises its mmap threshold after the first big block is freed, so later decode buffers
 * would stay in a pool; pinning it keeps them on their own mappings. Two pools stop every decode
 * thread leaving freed memory behind in a pool of its own.
 */
pub fn limit() {
    LIMIT.call_once(|| {
        let mapped = unsafe { mallopt(M_MMAP_THRESHOLD, ONE_MEGABYTE) };
        let pooled = unsafe { mallopt(M_ARENA_MAX, POOLS) };

        assert!(
            mapped == 1 && pooled == 1,
            "failed to set the allocator's limits"
        );

        thread::spawn(trim);
    });
}

/*
 * a pool only shrinks from its end, so the gaps decoding, d-bus and niri's
 * events leave in the middle stay with the process; trimming gives them back.
 * the timer is the backstop: a collapse to Rest and a normal window close trim
 * once between frames, or a sample two seconds later still shows the freed heap
 */
fn trim() {
    loop {
        thread::sleep(TRIM);

        trim_heap();
    }
}

pub(crate) fn trim_heap() {
    unsafe { malloc_trim(0) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limit_is_idempotent() {
        limit();
        limit();
    }
}
