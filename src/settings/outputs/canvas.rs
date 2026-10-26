//! Layout canvas geometry and snapping for output arrangements.

/// A rectangle in logical display coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn right(&self) -> i32 {
        self.x + self.width as i32
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.height as i32
    }

    #[allow(dead_code)]
    pub fn center_x(&self) -> i32 {
        self.x + (self.width as i32) / 2
    }

    #[allow(dead_code)]
    pub fn center_y(&self) -> i32 {
        self.y + (self.height as i32) / 2
    }
}

/// The bounding box of a collection of rectangles in logical coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoundingBox {
    pub min_x: i32,
    pub min_y: i32,
    pub max_x: i32,
    pub max_y: i32,
    pub width: u32,
    pub height: u32,
}

impl BoundingBox {
    /// Computes the bounding box of the given rectangles.
    /// If empty, returns a zero bounding box at origin.
    pub fn of(rects: &[Rect]) -> Self {
        if rects.is_empty() {
            return Self {
                min_x: 0,
                min_y: 0,
                max_x: 0,
                max_y: 0,
                width: 0,
                height: 0,
            };
        }

        let min_x = rects.iter().map(|r| r.x).min().unwrap();
        let min_y = rects.iter().map(|r| r.y).min().unwrap();
        let max_x = rects.iter().map(|r| r.right()).max().unwrap();
        let max_y = rects.iter().map(|r| r.bottom()).max().unwrap();
        let width = (max_x - min_x).max(0) as u32;
        let height = (max_y - min_y).max(0) as u32;

        Self {
            min_x,
            min_y,
            max_x,
            max_y,
            width,
            height,
        }
    }
}

/// Fits a bounding box into a canvas with padding, computing scale and centering offsets.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CanvasFit {
    pub scale: f32,
    pub offset_x: f32,
    pub offset_y: f32,
    pub bbox: BoundingBox,
}

impl CanvasFit {
    pub fn new(bbox: BoundingBox, canvas_width: f32, canvas_height: f32, padding: f32) -> Self {
        if bbox.width == 0 || bbox.height == 0 {
            return Self {
                scale: 1.0,
                offset_x: canvas_width / 2.0,
                offset_y: canvas_height / 2.0,
                bbox,
            };
        }

        let usable_w = (canvas_width - 2.0 * padding).max(1.0);
        let usable_h = (canvas_height - 2.0 * padding).max(1.0);
        let scale = (usable_w / bbox.width as f32).min(usable_h / bbox.height as f32);

        let bbox_canvas_w = bbox.width as f32 * scale;
        let bbox_canvas_h = bbox.height as f32 * scale;

        let bbox_canvas_x = (canvas_width - bbox_canvas_w) / 2.0;
        let bbox_canvas_y = (canvas_height - bbox_canvas_h) / 2.0;

        let offset_x = bbox_canvas_x - bbox.min_x as f32 * scale;
        let offset_y = bbox_canvas_y - bbox.min_y as f32 * scale;

        Self {
            scale,
            offset_x,
            offset_y,
            bbox,
        }
    }

    /// Maps a logical rectangle to canvas coordinates (x, y, width, height).
    pub fn to_canvas(self, x: i32, y: i32, width: u32, height: u32) -> (f32, f32, f32, f32) {
        let cx = self.offset_x + x as f32 * self.scale;
        let cy = self.offset_y + y as f32 * self.scale;
        let cw = width as f32 * self.scale;
        let ch = height as f32 * self.scale;
        (cx, cy, cw, ch)
    }

    /// Maps canvas point (cx, cy) to logical coordinates (x, y).
    #[allow(dead_code)]
    pub fn to_logical(self, cx: f32, cy: f32) -> (i32, i32) {
        let lx = ((cx - self.offset_x) / self.scale).round() as i32;
        let ly = ((cy - self.offset_y) / self.scale).round() as i32;
        (lx, ly)
    }
}

/// Threshold in logical pixels for snapping candidate dragged monitor edges.
pub const SNAP_THRESHOLD: i32 = 10;

/// Snaps a candidate position (x, y) for a rectangle of (width, height) against other rectangles.
///
/// Snaps to:
/// - Adjacent edges:
///   - Left-to-Right: candidate left snaps to other right (other.x + other.width)
///   - Right-to-Left: candidate right snaps to other left (other.x - width)
///   - Top-to-Bottom: candidate top snaps to other bottom (other.y + other.height)
///   - Bottom-to-Top: candidate bottom snaps to other top (other.y - height)
/// - Alignments:
///   - Lefts: candidate left snaps to other left (other.x)
///   - Rights: candidate right snaps to other right (other.x + other.width - width)
///   - Centers X: candidate center snaps to other center (other.x + (other.width - width) / 2)
///   - Tops: candidate top snaps to other top (other.y)
///   - Bottoms: candidate bottom snaps to other bottom (other.y + other.height - height)
///   - Centers Y: candidate center snaps to other center (other.y + (other.height - height) / 2)
pub fn snap(
    candidate_x: i32,
    candidate_y: i32,
    width: u32,
    height: u32,
    others: &[Rect],
    threshold: i32,
) -> (i32, i32) {
    let snapped_x = snap_1d(candidate_x, width, others, threshold, |r| r.x, |r| r.width);
    let snapped_y = snap_1d(
        candidate_y,
        height,
        others,
        threshold,
        |r| r.y,
        |r| r.height,
    );
    (snapped_x, snapped_y)
}

fn snap_1d(
    candidate: i32,
    size: u32,
    others: &[Rect],
    threshold: i32,
    pos: impl Fn(&Rect) -> i32,
    dim: impl Fn(&Rect) -> u32,
) -> i32 {
    let mut best_target = candidate;
    let mut best_diff = threshold + 1;

    for other in others {
        let o_pos = pos(other);
        let o_dim = dim(other);

        let targets = [
            o_pos + o_dim as i32,
            o_pos - size as i32,
            o_pos,
            o_pos + o_dim as i32 - size as i32,
            o_pos + (o_dim as i32 - size as i32) / 2,
        ];

        for target in targets {
            let diff = (candidate - target).abs();
            if diff <= threshold && diff < best_diff {
                best_diff = diff;
                best_target = target;
            }
        }
    }

    best_target
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounding_box_of_empty_is_zero() {
        let bbox = BoundingBox::of(&[]);
        assert_eq!(
            bbox,
            BoundingBox {
                min_x: 0,
                min_y: 0,
                max_x: 0,
                max_y: 0,
                width: 0,
                height: 0,
            }
        );
    }

    #[test]
    fn bounding_box_single_rect() {
        let rect = Rect::new(100, 200, 1920, 1080);
        let bbox = BoundingBox::of(&[rect]);
        assert_eq!(
            bbox,
            BoundingBox {
                min_x: 100,
                min_y: 200,
                max_x: 2020,
                max_y: 1280,
                width: 1920,
                height: 1080,
            }
        );
    }

    #[test]
    fn bounding_box_handles_negative_coordinates() {
        let r1 = Rect::new(-1920, 0, 1920, 1080);
        let r2 = Rect::new(0, 0, 1920, 1080);
        let bbox = BoundingBox::of(&[r1, r2]);
        assert_eq!(
            bbox,
            BoundingBox {
                min_x: -1920,
                min_y: 0,
                max_x: 1920,
                max_y: 1080,
                width: 3840,
                height: 1080,
            }
        );
    }

    #[test]
    fn canvas_fit_scale_and_centering() {
        let bbox = BoundingBox {
            min_x: 0,
            min_y: 0,
            max_x: 3840,
            max_y: 1080,
            width: 3840,
            height: 1080,
        };
        let canvas_w = 680.0;
        let canvas_h = 220.0;
        let padding = 20.0;

        let fit = CanvasFit::new(bbox, canvas_w, canvas_h, padding);

        let usable_w = canvas_w - 2.0 * padding; // 640.0
        let usable_h = canvas_h - 2.0 * padding; // 180.0
        let expected_scale = (usable_w / 3840.0).min(usable_h / 1080.0); // 640/3840 = 0.16666667, 180/1080 = 0.16666667
        assert!((fit.scale - expected_scale).abs() < 1e-5);

        // Check centering
        let (cx, cy, cw, ch) = fit.to_canvas(0, 0, 3840, 1080);
        assert!((cx - padding).abs() < 1e-4);
        assert!((cy - padding).abs() < 1e-4);
        assert!((cw - usable_w).abs() < 1e-4);
        assert!((ch - usable_h).abs() < 1e-4);

        // Logical roundtrip
        let (lx, ly) = fit.to_logical(cx, cy);
        assert_eq!((lx, ly), (0, 0));
    }

    #[test]
    fn canvas_fit_with_negative_offset() {
        let bbox = BoundingBox {
            min_x: -1920,
            min_y: -500,
            max_x: 1920,
            max_y: 580,
            width: 3840,
            height: 1080,
        };
        let fit = CanvasFit::new(bbox, 680.0, 220.0, 20.0);
        let (lx, ly) = fit.to_logical(
            fit.to_canvas(-1920, -500, 100, 100).0,
            fit.to_canvas(-1920, -500, 100, 100).1,
        );
        assert_eq!((lx, ly), (-1920, -500));
    }

    #[test]
    fn snap_adjacent_edges() {
        let other = Rect::new(0, 0, 1920, 1080);
        let others = [other];
        let threshold = 10;

        // Left-to-right: candidate at x = 1925 (within 10 of other.right = 1920)
        let (sx, _) = snap(1925, 500, 1920, 1080, &others, threshold);
        assert_eq!(sx, 1920);

        // Right-to-left: candidate at x = -1928 (within 10 of other.left - width = -1920)
        let (sx, _) = snap(-1928, 500, 1920, 1080, &others, threshold);
        assert_eq!(sx, -1920);

        // Top-to-bottom: candidate at y = 1085 (within 10 of other.bottom = 1080)
        let (_, sy) = snap(500, 1085, 1920, 1080, &others, threshold);
        assert_eq!(sy, 1080);

        // Bottom-to-top: candidate at y = -1086 (within 10 of other.top - height = -1080)
        let (_, sy) = snap(500, -1086, 1920, 1080, &others, threshold);
        assert_eq!(sy, -1080);
    }

    #[test]
    fn snap_alignments() {
        let other = Rect::new(0, 0, 1920, 1080);
        let others = [other];
        let threshold = 10;

        // Lefts alignment: candidate at x = 8 (within 10 of other.left = 0)
        let (sx, _) = snap(8, 2000, 1920, 1080, &others, threshold);
        assert_eq!(sx, 0);

        // Rights alignment: candidate width 1280 at x = 645 (within 10 of 1920 - 1280 = 640)
        let (sx, _) = snap(645, 2000, 1280, 1024, &others, threshold);
        assert_eq!(sx, 640);

        // Centers alignment X: candidate width 1280 at x = 325 (within 10 of (1920 - 1280)/2 = 320)
        let (sx, _) = snap(325, 2000, 1280, 1024, &others, threshold);
        assert_eq!(sx, 320);

        // Tops alignment: candidate at y = -6 (within 10 of other.top = 0)
        let (_, sy) = snap(2000, -6, 1920, 1080, &others, threshold);
        assert_eq!(sy, 0);

        // Bottoms alignment: candidate height 720 at y = 363 (within 10 of 1080 - 720 = 360)
        let (_, sy) = snap(2000, 363, 1280, 720, &others, threshold);
        assert_eq!(sy, 360);

        // Centers alignment Y: candidate height 720 at y = 178 (within 10 of (1080 - 720)/2 = 180)
        let (_, sy) = snap(2000, 178, 1280, 720, &others, threshold);
        assert_eq!(sy, 180);
    }

    #[test]
    fn snap_beyond_threshold_keeps_candidate() {
        let other = Rect::new(0, 0, 1920, 1080);
        let others = [other];
        let threshold = 10;

        // Diff 15 is > threshold 10
        let (sx, sy) = snap(1935, 1095, 1920, 1080, &others, threshold);
        assert_eq!((sx, sy), (1935, 1095));
    }
}
