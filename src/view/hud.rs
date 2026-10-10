//! The OSD over a Peek or an open Surface.

use std::time::{Duration, Instant};

use kanade_runtime::{Center, End, Monitor, Padding, Rectangle, Shadow, Start, request_frame};

use crate::island::geometry;
use crate::island::presentation::{Content, Presentation};
use crate::island::service::IslandService;
use crate::theme;

use super::forms::small_form;
use super::shape::shape;

// how the OSD over a Peek or an open Surface comes in and goes
const HUD_IN: Duration = Duration::from_millis(180);
const HUD_OUT: Duration = Duration::from_millis(260);

// how far in it starts, popping up to its size as it comes
const HUD_POP: f32 = 0.86;

/*
 * the OSD over a Peek or an open Surface, which it would otherwise end: the Compact form floats as
 * its own capsule over the near edge, pops in as the key is pressed, and fades out as it expires,
 * the Peek or Surface staying open under it
 */
pub(super) fn hud(
    island: &IslandService,
    monitor: &Monitor,
    body: geometry::Rect,
    hang: geometry::Hang,
    now: Instant,
) -> Option<Rectangle> {
    let frame = island.frame(&monitor.name, now);

    if !island.floats(&monitor.name, &frame) {
        return None;
    }

    let primary = frame.primary?;

    let (id, since, expiry) = island.shown(&monitor.name)?;

    if id != primary.id() {
        return None;
    }

    let shown = Content {
        presentation: Presentation::Compact,
        activity: Some(primary),
        satellite: None,
    };

    let form = small_form(&shown, None, now)?;

    let coming = now.saturating_duration_since(since).as_secs_f32() / HUD_IN.as_secs_f32();
    let going = expiry.map_or(1.0, |expiry| {
        expiry.saturating_duration_since(now).as_secs_f32() / HUD_OUT.as_secs_f32()
    });

    // eased out, as the Island's own fades
    let come = 1.0 - (1.0 - coming.clamp(0.0, 1.0)).powi(3);
    let go = going.clamp(0.0, 1.0).powi(2);

    // it moves until it is gone; the expiry itself redraws the Island
    request_frame();

    let roles = theme::island();
    let edge = 10.0;
    let compact = shape(Presentation::Compact);

    let capsule = Rectangle::new()
        .width(compact.width)
        .height(compact.height)
        .radius(compact.radius)
        .fill(roles.surface_container_high)
        .border(1.0, roles.outline)
        .shadow(Shadow::drop(theme::SHADOW_HUD).blur(18.0).offset(0.0, 4.0))
        .scale(HUD_POP + (1.0 - HUD_POP) * come)
        .opacity(come * go)
        .child(form);

    let over = Rectangle::new().width(body.width).height(body.height);

    Some(
        match hang.edge {
            geometry::Edge::Top => over
                .padding(Padding {
                    top: edge,
                    ..Padding::default()
                })
                .align_child(Center, Start),
            geometry::Edge::Bottom => over
                .padding(Padding {
                    bottom: edge,
                    ..Padding::default()
                })
                .align_child(Center, End),
            geometry::Edge::Left => over
                .padding(Padding {
                    left: edge,
                    ..Padding::default()
                })
                .align_child(Start, Center),
            geometry::Edge::Right => over
                .padding(Padding {
                    right: edge,
                    ..Padding::default()
                })
                .align_child(End, Center),
        }
        .child(capsule),
    )
}
