use std::time::{Duration, Instant};

use smithay_client_toolkit::reexports::protocols::ext::idle_notify::v1::client::{
    ext_idle_notification_v1::{self, ExtIdleNotificationV1},
    ext_idle_notifier_v1::{self, ExtIdleNotifierV1},
};
use wayland_client::{Connection, Dispatch, QueueHandle};

use crate::idle;

use super::WaylandState;

impl WaylandState {
    /*
     * a notification for each timeout `idle::watch` asks for, on the first seat: the ones no
     * longer asked for destroyed, the new ones made. Through `get_idle_notification`, so the
     * Wayland idle inhibitors apps hold keep it from going idle
     */
    pub(super) fn follow_idle(&mut self) {
        let Some(notifier) = &self.idle_notifier else {
            return;
        };

        let wanted = idle::wanted();

        self.idle_notifications.retain(|(timeout, notification)| {
            let keep = wanted.contains(timeout);

            if !keep {
                notification.destroy();
                idle::resumed(*timeout);
            }

            keep
        });

        let Some(seat) = self.seat.seats().next() else {
            return;
        };

        for timeout in wanted {
            if self
                .idle_notifications
                .iter()
                .any(|(armed, _)| *armed == timeout)
            {
                continue;
            }

            let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
            let notification = notifier.get_idle_notification(ms, &seat, &self.qh, timeout);

            self.idle_notifications.push((timeout, notification));
        }
    }

    // a seat gone takes its notifications with it; the next seat gets new ones
    pub(super) fn forget_idle(&mut self) {
        for (timeout, notification) in self.idle_notifications.drain(..) {
            notification.destroy();
            idle::resumed(timeout);
        }
    }
}

impl Dispatch<ExtIdleNotifierV1, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &ExtIdleNotifierV1,
        _: ext_idle_notifier_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtIdleNotificationV1, Duration> for WaylandState {
    fn event(
        _: &mut Self,
        _: &ExtIdleNotificationV1,
        event: ext_idle_notification_v1::Event,
        timeout: &Duration,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_idle_notification_v1::Event::Idled => idle::idled(*timeout, Instant::now()),
            ext_idle_notification_v1::Event::Resumed => idle::resumed(*timeout),
            _ => {}
        }
    }
}

// the idle notifier, when the compositor has one
pub fn bind(
    globals: &wayland_client::globals::GlobalList,
    qh: &QueueHandle<WaylandState>,
) -> Option<ExtIdleNotifierV1> {
    match globals.bind::<ExtIdleNotifierV1, _, _>(qh, 1..=1, ()) {
        Ok(notifier) => Some(notifier),
        Err(_) => {
            idle::unavailable("the compositor has no ext-idle-notify-v1");

            None
        }
    }
}
