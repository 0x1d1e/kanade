//! XDG Settings window chrome hit testing. The compositor performs the move
//! or resize; Kanade only chooses the edge from the actual pointer position.

use wayland_protocols::xdg::shell::client::xdg_toplevel::ResizeEdge;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChromeAction {
    Move,
    Resize(ResizeEdge),
    Content,
}

/// Border hit areas precede the title strip; the center remains content.
/// The hit boxes never overlap a valid Settings content button.
pub fn hit_test(width: u32, height: u32, x: f64, y: f64) -> ChromeAction {
    const BORDER: f64 = 9.0;
    const TITLE: f64 = 40.0;
    let width = width as f64;
    let height = height as f64;
    if x < 0.0 || y < 0.0 || x >= width || y >= height || width < 36.0 || height < 36.0 {
        return ChromeAction::Content;
    }
    let left = x < BORDER;
    let right = x >= width - BORDER;
    let top = y < BORDER;
    let bottom = y >= height - BORDER;
    if top && left {
        ChromeAction::Resize(ResizeEdge::TopLeft)
    } else if top && right {
        ChromeAction::Resize(ResizeEdge::TopRight)
    } else if bottom && left {
        ChromeAction::Resize(ResizeEdge::BottomLeft)
    } else if bottom && right {
        ChromeAction::Resize(ResizeEdge::BottomRight)
    } else if top {
        ChromeAction::Resize(ResizeEdge::Top)
    } else if bottom {
        ChromeAction::Resize(ResizeEdge::Bottom)
    } else if left {
        ChromeAction::Resize(ResizeEdge::Left)
    } else if right {
        ChromeAction::Resize(ResizeEdge::Right)
    } else if y < TITLE {
        ChromeAction::Move
    } else {
        ChromeAction::Content
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_chrome_has_eight_resize_edges() {
        let w = 640;
        let h = 480;
        assert_eq!(
            hit_test(w, h, 1.0, 1.0),
            ChromeAction::Resize(ResizeEdge::TopLeft)
        );
        assert_eq!(
            hit_test(w, h, 638.0, 1.0),
            ChromeAction::Resize(ResizeEdge::TopRight)
        );
        assert_eq!(
            hit_test(w, h, 638.0, 478.0),
            ChromeAction::Resize(ResizeEdge::BottomRight)
        );
        assert_eq!(
            hit_test(w, h, 1.0, 478.0),
            ChromeAction::Resize(ResizeEdge::BottomLeft)
        );
        assert_eq!(
            hit_test(w, h, 10.0, 1.0),
            ChromeAction::Resize(ResizeEdge::Top)
        );
        assert_eq!(
            hit_test(w, h, 10.0, 478.0),
            ChromeAction::Resize(ResizeEdge::Bottom)
        );
        assert_eq!(
            hit_test(w, h, 1.0, 60.0),
            ChromeAction::Resize(ResizeEdge::Left)
        );
        assert_eq!(
            hit_test(w, h, 638.0, 60.0),
            ChromeAction::Resize(ResizeEdge::Right)
        );
    }

    #[test]
    fn header_drags_but_content_never_does() {
        assert_eq!(hit_test(640, 480, 120.0, 22.0), ChromeAction::Move);
        assert_eq!(hit_test(640, 480, 120.0, 100.0), ChromeAction::Content);
        assert_eq!(hit_test(640, 480, -2.0, 10.0), ChromeAction::Content);
        assert_eq!(hit_test(640, 480, 645.0, 10.0), ChromeAction::Content);
        assert_eq!(hit_test(0, 0, 0.0, 0.0), ChromeAction::Content);
    }
}
