//! Kanade's StatusNotifierWatcher, served while Kanade holds its name: items register here, and the
//! tray thread hears of each. It lists what the tray thread keeps, so a registration it refused, or
//! an item that is gone, is not listed. Kanade is its own host, so a host is always registered.

use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, PoisonError};

use zbus::message::Header;
use zbus::object_server::SignalEmitter;
use zbus::{fdo, interface};

use super::Event;
use super::item::Address;

pub const NAME: &str = "org.kde.StatusNotifierWatcher";
pub const PATH: &str = "/StatusNotifierWatcher";
pub const INTERFACE: &str = "org.kde.StatusNotifierWatcher";

pub struct Watcher {
    pub events: Sender<Event>,

    // as listed, written by the tray thread
    pub registered: Arc<Mutex<Vec<String>>>,
}

#[interface(name = "org.kde.StatusNotifierWatcher")]
impl Watcher {
    async fn register_status_notifier_item(
        &self,
        service: &str,
        #[zbus(header)] header: Header<'_>,
    ) -> fdo::Result<()> {
        let caller = header.sender().map(|caller| caller.as_str());

        let address = Address::registered(service, caller)
            .ok_or_else(|| fdo::Error::InvalidArgs(format!("no item at {service:?}")))?;

        let _ = self.events.send(Event::Registered {
            address,
            watcher: None,
        });

        Ok(())
    }

    // any host may register; Kanade already is one
    async fn register_status_notifier_host(
        &self,
        _service: &str,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> fdo::Result<()> {
        let _ = Self::status_notifier_host_registered(&emitter).await;

        Ok(())
    }

    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> Vec<String> {
        self.registered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn protocol_version(&self) -> i32 {
        0
    }

    #[zbus(signal)]
    async fn status_notifier_item_registered(
        emitter: &SignalEmitter<'_>,
        service: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_item_unregistered(
        emitter: &SignalEmitter<'_>,
        service: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_host_registered(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}
