use std::io::Read;
use std::os::unix::io::{AsFd, BorrowedFd};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use wayland_client::backend::ObjectId;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, event_created_child};
use wayland_protocols::ext::data_control::v1::client::{
    ext_data_control_device_v1 as ext_device, ext_data_control_manager_v1 as ext_manager,
    ext_data_control_offer_v1 as ext_offer,
};
use wayland_protocols_wlr::data_control::v1::client::{
    zwlr_data_control_device_v1 as wlr_device, zwlr_data_control_manager_v1 as wlr_manager,
    zwlr_data_control_offer_v1 as wlr_offer,
};

use super::WaylandState;

pub const LIMIT: usize = 16 << 20;
pub const TIMEOUT: Duration = Duration::from_secs(2);

static SELECTION_SEQ: AtomicU64 = AtomicU64::new(0);

pub(super) enum DataControlManager {
    Ext(ext_manager::ExtDataControlManagerV1),
    Wlr(wlr_manager::ZwlrDataControlManagerV1),
}

pub(super) enum DataControlDevice {
    Ext(ext_device::ExtDataControlDeviceV1),
    Wlr(wlr_device::ZwlrDataControlDeviceV1),
}

pub(super) enum ActiveOffer {
    Ext(ext_offer::ExtDataControlOfferV1),
    Wlr(wlr_offer::ZwlrDataControlOfferV1),
}

impl ActiveOffer {
    pub(super) fn id(&self) -> ObjectId {
        match self {
            Self::Ext(o) => o.id(),
            Self::Wlr(o) => o.id(),
        }
    }

    pub(super) fn destroy(self) {
        match self {
            Self::Ext(o) => o.destroy(),
            Self::Wlr(o) => o.destroy(),
        }
    }

    pub(super) fn receive(&self, mime: String, fd: BorrowedFd<'_>) {
        match self {
            Self::Ext(o) => o.receive(mime, fd),
            Self::Wlr(o) => o.receive(mime, fd),
        }
    }
}

pub(super) fn bind(
    globals: &wayland_client::globals::GlobalList,
    qh: &QueueHandle<WaylandState>,
) -> Option<DataControlManager> {
    if let Ok(manager) = globals.bind::<ext_manager::ExtDataControlManagerV1, _, _>(qh, 1..=1, ()) {
        return Some(DataControlManager::Ext(manager));
    }
    if let Ok(manager) = globals.bind::<wlr_manager::ZwlrDataControlManagerV1, _, _>(qh, 1..=2, ())
    {
        return Some(DataControlManager::Wlr(manager));
    }
    None
}

impl WaylandState {
    pub(super) fn follow_data_control(&mut self, qh: &QueueHandle<Self>, seat: &WlSeat) {
        if !self.data_control_enabled || self.data_control_device.is_some() {
            return;
        }

        if let Some(manager) = &self.data_control_manager {
            let device = match manager {
                DataControlManager::Ext(m) => {
                    DataControlDevice::Ext(m.get_data_device(seat, qh, ()))
                }
                DataControlManager::Wlr(m) => {
                    DataControlDevice::Wlr(m.get_data_device(seat, qh, ()))
                }
            };
            self.data_control_device = Some(device);
        }
    }

    pub(super) fn drop_data_control(&mut self) {
        SELECTION_SEQ.fetch_add(1, Ordering::SeqCst);
        if let Some(device) = self.data_control_device.take() {
            match device {
                DataControlDevice::Ext(d) => d.destroy(),
                DataControlDevice::Wlr(d) => d.destroy(),
            }
        }
        if let Some(offer) = self.data_control_active_offer.take() {
            offer.destroy();
        }
        self.data_control_offers.clear();
    }
}

impl Dispatch<ext_manager::ExtDataControlManagerV1, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &ext_manager::ExtDataControlManagerV1,
        _: ext_manager::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wlr_manager::ZwlrDataControlManagerV1, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &wlr_manager::ZwlrDataControlManagerV1,
        _: wlr_manager::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ext_device::ExtDataControlDeviceV1, ()> for WaylandState {
    fn event(
        state: &mut Self,
        _: &ext_device::ExtDataControlDeviceV1,
        event: ext_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_device::Event::DataOffer { id } => {
                state.data_control_offers.insert(id.id(), Vec::new());
            }
            ext_device::Event::Selection { id } => {
                handle_selection(state, id.map(ActiveOffer::Ext));
            }
            ext_device::Event::PrimarySelection { id: Some(offer) } => {
                state.data_control_offers.remove(&offer.id());
                offer.destroy();
            }
            ext_device::Event::Finished => {
                state.drop_data_control();
            }
            _ => {}
        }
    }

    event_created_child!(WaylandState, ext_device::ExtDataControlDeviceV1, [
        ext_device::EVT_DATA_OFFER_OPCODE => (ext_offer::ExtDataControlOfferV1, ()),
    ]);
}

impl Dispatch<wlr_device::ZwlrDataControlDeviceV1, ()> for WaylandState {
    fn event(
        state: &mut Self,
        _: &wlr_device::ZwlrDataControlDeviceV1,
        event: wlr_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wlr_device::Event::DataOffer { id } => {
                state.data_control_offers.insert(id.id(), Vec::new());
            }
            wlr_device::Event::Selection { id } => {
                handle_selection(state, id.map(ActiveOffer::Wlr));
            }
            wlr_device::Event::PrimarySelection { id: Some(offer) } => {
                state.data_control_offers.remove(&offer.id());
                offer.destroy();
            }
            wlr_device::Event::Finished => {
                state.drop_data_control();
            }
            _ => {}
        }
    }

    event_created_child!(WaylandState, wlr_device::ZwlrDataControlDeviceV1, [
        wlr_device::EVT_DATA_OFFER_OPCODE => (wlr_offer::ZwlrDataControlOfferV1, ()),
    ]);
}

impl Dispatch<ext_offer::ExtDataControlOfferV1, ()> for WaylandState {
    fn event(
        state: &mut Self,
        offer: &ext_offer::ExtDataControlOfferV1,
        event: ext_offer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ext_offer::Event::Offer { mime_type } = event {
            state
                .data_control_offers
                .entry(offer.id())
                .or_default()
                .push(mime_type);
        }
    }
}

impl Dispatch<wlr_offer::ZwlrDataControlOfferV1, ()> for WaylandState {
    fn event(
        state: &mut Self,
        offer: &wlr_offer::ZwlrDataControlOfferV1,
        event: wlr_offer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wlr_offer::Event::Offer { mime_type } = event {
            state
                .data_control_offers
                .entry(offer.id())
                .or_default()
                .push(mime_type);
        }
    }
}

fn handle_selection(state: &mut WaylandState, offer: Option<ActiveOffer>) {
    let generation = SELECTION_SEQ.fetch_add(1, Ordering::SeqCst) + 1;

    if let Some(old) = state.data_control_active_offer.take() {
        old.destroy();
    }

    let Some(offer) = offer else {
        return;
    };

    let types = state
        .data_control_offers
        .remove(&offer.id())
        .unwrap_or_default();

    if types.iter().any(|m| m == "x-kde-passwordManagerHint") {
        offer.destroy();
        return;
    }

    let Some(mime) = pick_mime(&types) else {
        offer.destroy();
        return;
    };

    let (read_end, write_end) = match rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC) {
        Ok(pipe) => pipe,
        Err(_) => {
            offer.destroy();
            return;
        }
    };

    offer.receive(mime.clone(), write_end.as_fd());
    drop(write_end);
    let _ = state.connection.flush();

    state.data_control_active_offer = Some(offer);

    let file = std::fs::File::from(read_end);
    spawn_bounded_read(file, mime, generation);
}

fn spawn_bounded_read(file: std::fs::File, mime: String, generation: u64) {
    let _ = std::thread::Builder::new()
        .name("clipboard-transfer".into())
        .spawn(move || {
            if let Some(bytes) = read_pipe_bounded(file, LIMIT, TIMEOUT, generation) {
                send_selection(Some(mime), bytes, generation);
            }
        });
}

pub(crate) fn read_pipe_bounded(
    mut file: std::fs::File,
    limit: usize,
    timeout: Duration,
    generation: u64,
) -> Option<Vec<u8>> {
    let deadline = Instant::now() + timeout;
    let mut data = Vec::new();
    let mut chunk = [0u8; 64 * 1024];

    loop {
        if SELECTION_SEQ.load(Ordering::SeqCst) != generation {
            return None;
        }

        let now = Instant::now();
        if now >= deadline {
            return None;
        }
        let remaining = deadline.saturating_duration_since(now);
        let slice = remaining.min(Duration::from_millis(50));
        let timeout_ts: rustix::event::Timespec = slice.try_into().ok()?;

        let mut poll_fds = [rustix::event::PollFd::new(
            &file,
            rustix::event::PollFlags::IN,
        )];
        match rustix::event::poll(&mut poll_fds, Some(&timeout_ts)) {
            Ok(0) => {
                if Instant::now() >= deadline {
                    return None;
                }
                continue;
            }
            Ok(_) => {
                if SELECTION_SEQ.load(Ordering::SeqCst) != generation {
                    return None;
                }

                let revents = poll_fds[0].revents();
                if revents.contains(rustix::event::PollFlags::ERR) {
                    return None;
                }
                match file.read(&mut chunk) {
                    Ok(0) => {
                        if SELECTION_SEQ.load(Ordering::SeqCst) != generation {
                            return None;
                        }
                        return Some(data);
                    }
                    Ok(n) => {
                        if data.len() + n > limit {
                            return None;
                        }
                        data.extend_from_slice(&chunk[..n]);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => return None,
                }
            }
            Err(rustix::io::Errno::INTR) => continue,
            Err(_) => return None,
        }
    }
}

pub(crate) fn pick_mime(types: &[String]) -> Option<String> {
    if types.iter().any(|m| m == "text/plain;charset=utf-8") {
        return Some(String::from("text/plain;charset=utf-8"));
    }
    if types.iter().any(|m| m == "text/plain") {
        return Some(String::from("text/plain"));
    }
    if types.iter().any(|m| m == "UTF8_STRING") {
        return Some(String::from("UTF8_STRING"));
    }
    if let Some(mime) = types.iter().find(|m| textual(m)) {
        return Some(mime.clone());
    }
    if types.iter().any(|m| m == "image/png") {
        return Some(String::from("image/png"));
    }
    if let Some(mime) = types.iter().find(|m| m.starts_with("image/")) {
        return Some(mime.clone());
    }
    None
}

fn textual(mime: &str) -> bool {
    mime.starts_with("text/")
        || matches!(mime, "TEXT" | "STRING" | "UTF8_STRING")
        || mime.contains("json")
        || ["script", "xml", "yaml", "csv", "ini", "pgp-keys"]
            .iter()
            .any(|suffix| mime.ends_with(suffix))
        || mime.contains("application/vnd.ms-publisher")
}

pub type Handler = fn(Option<String>, Vec<u8>);

static HANDLER: Mutex<Option<Handler>> = Mutex::new(None);

pub fn set_handler(handler: Handler) {
    if let Ok(mut lock) = HANDLER.lock() {
        *lock = Some(handler);
    }
}

pub fn on_selection(handler: Handler) {
    set_handler(handler);
}

fn send_selection(mime: Option<String>, bytes: Vec<u8>, generation: u64) {
    if SELECTION_SEQ.load(Ordering::SeqCst) != generation {
        return;
    }
    let handler = HANDLER.lock().ok().and_then(|h| *h);
    if let Some(handler) = handler {
        handler(mime, bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_pick_mime_preference() {
        assert_eq!(
            pick_mime(&["text/plain".into(), "text/plain;charset=utf-8".into()]),
            Some("text/plain;charset=utf-8".into())
        );
        assert_eq!(
            pick_mime(&["image/png".into(), "text/plain".into()]),
            Some("text/plain".into())
        );
        assert_eq!(
            pick_mime(&["image/png".into(), "image/jpeg".into()]),
            Some("image/png".into())
        );
        assert_eq!(pick_mime(&["application/x-unknown".into()]), None);
    }

    #[test]
    fn test_read_pipe_bounded_success() {
        let _lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (read_end, write_end) =
            rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC).unwrap();
        let file = std::fs::File::from(read_end);

        let writer = std::thread::spawn(move || {
            let mut write_file = std::fs::File::from(write_end);
            write_file.write_all(b"hello clipboard").unwrap();
        });

        let generation = SELECTION_SEQ.fetch_add(1, Ordering::SeqCst) + 1;
        let data = read_pipe_bounded(file, 1024, Duration::from_secs(1), generation).unwrap();
        assert_eq!(data, b"hello clipboard");
        writer.join().unwrap();
    }

    #[test]
    fn test_read_pipe_bounded_overflow() {
        let _lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (read_end, write_end) =
            rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC).unwrap();
        let file = std::fs::File::from(read_end);

        let writer = std::thread::spawn(move || {
            let mut write_file = std::fs::File::from(write_end);
            write_file.write_all(b"123456789012345").unwrap();
        });

        // Limit to 5 bytes
        let generation = SELECTION_SEQ.fetch_add(1, Ordering::SeqCst) + 1;
        let data = read_pipe_bounded(file, 5, Duration::from_secs(1), generation);
        assert!(data.is_none());
        writer.join().unwrap();
    }

    #[test]
    fn test_read_pipe_bounded_timeout() {
        let _lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (read_end, write_end) =
            rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC).unwrap();
        let file = std::fs::File::from(read_end);

        // Don't write anything, but keep write_end open
        let _write_end = write_end;
        let start = Instant::now();
        let generation = SELECTION_SEQ.fetch_add(1, Ordering::SeqCst) + 1;
        let data = read_pipe_bounded(file, 1024, Duration::from_millis(50), generation);
        assert!(data.is_none());
        assert!(start.elapsed() >= Duration::from_millis(45));
    }

    #[test]
    fn test_read_pipe_bounded_generation_mismatch() {
        let _lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (read_end, write_end) =
            rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC).unwrap();
        let file = std::fs::File::from(read_end);

        let writer = std::thread::spawn(move || {
            let mut write_file = std::fs::File::from(write_end);
            let _ = write_file.write_all(b"stale clipboard");
        });

        let generation = SELECTION_SEQ.fetch_add(1, Ordering::SeqCst) + 1;
        // Bump generation so generation is stale
        SELECTION_SEQ.fetch_add(1, Ordering::SeqCst);

        let data = read_pipe_bounded(file, 1024, Duration::from_secs(1), generation);
        assert!(data.is_none());
        writer.join().unwrap();
    }

    #[test]
    fn test_read_pipe_bounded_superseded_mid_read() {
        let _lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (read_end, write_end) =
            rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC).unwrap();
        let file = std::fs::File::from(read_end);

        let generation = SELECTION_SEQ.fetch_add(1, Ordering::SeqCst) + 1;

        let writer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            // Newer selection event arrives mid-read
            SELECTION_SEQ.fetch_add(1, Ordering::SeqCst);
            let mut write_file = std::fs::File::from(write_end);
            let _ = write_file.write_all(b"stale data");
        });

        let data = read_pipe_bounded(file, 1024, Duration::from_secs(1), generation);
        assert!(data.is_none());
        writer.join().unwrap();
    }

    #[test]
    fn test_send_selection_generation_check() {
        let _lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        static RECEIVED: Mutex<Option<Vec<u8>>> = Mutex::new(None);
        fn test_handler(_mime: Option<String>, bytes: Vec<u8>) {
            *RECEIVED.lock().unwrap() = Some(bytes);
        }
        set_handler(test_handler);

        // Matching generation delivers
        *RECEIVED.lock().unwrap() = None;
        let gen1 = SELECTION_SEQ.fetch_add(1, Ordering::SeqCst) + 1;
        send_selection(Some("text/plain".into()), b"fresh".to_vec(), gen1);
        assert_eq!(
            RECEIVED.lock().unwrap().as_deref(),
            Some(b"fresh".as_slice())
        );

        // Mismatched generation is dropped
        *RECEIVED.lock().unwrap() = None;
        let gen2 = SELECTION_SEQ.fetch_add(1, Ordering::SeqCst) + 1;
        SELECTION_SEQ.fetch_add(1, Ordering::SeqCst);
        send_selection(Some("text/plain".into()), b"stale".to_vec(), gen2);
        assert!(RECEIVED.lock().unwrap().is_none());
    }
}
