//! The $1 unistroke recognizer (Wobbrock, Wilson and Li, 2007), pure: a stroke is resampled to
//! `SAMPLES` points, turned so its first point lies on the x axis through the centroid, scaled
//! to a unit square and moved to the origin; a candidate is compared with each template at the
//! best angle within `ANGLE` (golden-section search), and matches only when its score is high
//! enough and clearly above every other gesture's (ADR 0105).
//!
//! Two departures from the paper, both for strokes a user draws by hand: the scale is uniform, so
//! a straight line does not blow its noise up to a square, and a stroke too short to mean anything
//! is no stroke.

use std::f32::consts::PI;

// the points a stroke is resampled to, and the length of a normalized stroke
pub const SAMPLES: usize = 64;

// a stroke shorter than this, in pixels, or with a smaller reach, is an accident of the hand
const MIN_LENGTH: f32 = 60.0;
const MIN_REACH: f32 = 24.0;

// the search turns a candidate this far either way, to within `PRECISION`
const ANGLE: f32 = 45.0 * PI / 180.0;
const PRECISION: f32 = 2.0 * PI / 180.0;

// the score a match needs, 1 being the same stroke, and how far it must lead the next gesture
pub const THRESHOLD: f32 = 0.85;
pub const MARGIN: f32 = 0.04;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    fn distance(self, other: Point) -> f32 {
        (self.x - other.x).hypot(self.y - other.y)
    }
}

// a gesture's recorded strokes, normalized
#[derive(Debug, Clone, PartialEq)]
pub struct Template {
    pub name: String,
    pub strokes: Vec<Vec<Point>>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Outcome<'a> {
    Match { name: &'a str, score: f32 },

    // nothing came near enough
    Unknown,

    // two gestures are about as near as each other
    Ambiguous,
}

// a drawn stroke as a normalized one, or none for one too short or flat to mean anything
pub fn normalize(drawn: &[Point]) -> Option<Vec<Point>> {
    if drawn
        .iter()
        .any(|point| !point.x.is_finite() || !point.y.is_finite())
    {
        return None;
    }

    let length = length(drawn);
    let (width, height) = reach(drawn);

    if length < MIN_LENGTH || width.max(height) < MIN_REACH {
        return None;
    }

    let mut points = resample(drawn, length);
    let center = centroid(&points);
    let turn = (center.y - points[0].y).atan2(center.x - points[0].x);

    rotate(&mut points, -turn, center);

    let (width, height) = reach(&points);
    let side = width.max(height);

    if side <= f32::EPSILON {
        return None;
    }

    let center = centroid(&points);

    for point in &mut points {
        point.x = (point.x - center.x) / side;
        point.y = (point.y - center.y) / side;
    }

    Some(points)
}

/*
 * the gesture a normalized candidate is, if one is: each gesture scores as its best stroke, and
 * the best must reach `THRESHOLD` and lead the second best by `MARGIN`
 */
pub fn recognize<'a>(candidate: &[Point], templates: &'a [Template]) -> Outcome<'a> {
    let mut scores: Vec<(&str, f32)> = templates
        .iter()
        .filter_map(|template| {
            template
                .strokes
                .iter()
                .map(|stroke| score(candidate, stroke))
                .max_by(f32::total_cmp)
                .map(|best| (template.name.as_str(), best))
        })
        .collect();

    scores.sort_by(|a, b| b.1.total_cmp(&a.1));

    let Some(&(name, best)) = scores.first() else {
        return Outcome::Unknown;
    };

    if best < THRESHOLD {
        return Outcome::Unknown;
    }

    if scores.get(1).is_some_and(|&(_, next)| best - next < MARGIN) {
        return Outcome::Ambiguous;
    }

    Outcome::Match { name, score: best }
}

// 1 for the same stroke, near 0 for as far apart as two unit strokes get
fn score(candidate: &[Point], template: &[Point]) -> f32 {
    if candidate.len() != template.len() || candidate.is_empty() {
        return 0.0;
    }

    // the corner to corner of the unit square, halved
    let half_diagonal = 0.5 * 2f32.sqrt();

    1.0 - distance_at_best_angle(candidate, template) / half_diagonal
}

// golden-section search for the turn of the candidate that brings it nearest the template
fn distance_at_best_angle(candidate: &[Point], template: &[Point]) -> f32 {
    let phi = 0.5 * (-1.0 + 5f32.sqrt());
    let (mut low, mut high) = (-ANGLE, ANGLE);
    let mut first = phi * low + (1.0 - phi) * high;
    let mut second = (1.0 - phi) * low + phi * high;
    let mut at_first = distance_at(candidate, template, first);
    let mut at_second = distance_at(candidate, template, second);

    while (high - low).abs() > PRECISION {
        if at_first < at_second {
            high = second;
            second = first;
            at_second = at_first;
            first = phi * low + (1.0 - phi) * high;
            at_first = distance_at(candidate, template, first);
        } else {
            low = first;
            first = second;
            at_first = at_second;
            second = (1.0 - phi) * low + phi * high;
            at_second = distance_at(candidate, template, second);
        }
    }

    at_first.min(at_second)
}

// the mean distance between the template and the candidate turned by `angle` about the origin
fn distance_at(candidate: &[Point], template: &[Point], angle: f32) -> f32 {
    let (sin, cos) = angle.sin_cos();
    let total: f32 = candidate
        .iter()
        .zip(template)
        .map(|(point, other)| {
            let turned = Point::new(point.x * cos - point.y * sin, point.x * sin + point.y * cos);

            turned.distance(*other)
        })
        .sum();

    total / candidate.len() as f32
}

fn length(points: &[Point]) -> f32 {
    points
        .windows(2)
        .map(|pair| pair[0].distance(pair[1]))
        .sum()
}

// the width and height of the box around the points
fn reach(points: &[Point]) -> (f32, f32) {
    let first = points.first().copied().unwrap_or_default();
    let (mut low, mut high) = (first, first);

    for point in points {
        low.x = low.x.min(point.x);
        low.y = low.y.min(point.y);
        high.x = high.x.max(point.x);
        high.y = high.y.max(point.y);
    }

    (high.x - low.x, high.y - low.y)
}

fn centroid(points: &[Point]) -> Point {
    let count = points.len().max(1) as f32;
    let (x, y) = points
        .iter()
        .fold((0.0, 0.0), |(x, y), point| (x + point.x, y + point.y));

    Point::new(x / count, y / count)
}

fn rotate(points: &mut [Point], angle: f32, about: Point) {
    let (sin, cos) = angle.sin_cos();

    for point in points {
        let (x, y) = (point.x - about.x, point.y - about.y);

        point.x = x * cos - y * sin + about.x;
        point.y = x * sin + y * cos + about.y;
    }
}

// `SAMPLES` points an equal way apart along the stroke, its first and last among them
fn resample(points: &[Point], length: f32) -> Vec<Point> {
    let interval = length / (SAMPLES - 1) as f32;
    let mut points = points.to_vec();
    let mut resampled = vec![points[0]];
    let mut covered = 0.0;
    let mut at = 1;

    while at < points.len() {
        let (before, after) = (points[at - 1], points[at]);
        let step = before.distance(after);

        if step > 0.0 && covered + step >= interval {
            let share = (interval - covered) / step;
            let point = Point::new(
                before.x + share * (after.x - before.x),
                before.y + share * (after.y - before.y),
            );

            resampled.push(point);

            // the rest of this step is measured from the new point
            points.insert(at, point);
            covered = 0.0;
        } else {
            covered += step;
        }

        at += 1;
    }

    // rounding may leave the last point out
    while resampled.len() < SAMPLES {
        resampled.push(points[points.len() - 1]);
    }

    resampled.truncate(SAMPLES);
    resampled
}

#[cfg(test)]
mod tests {
    use super::*;

    // a circle of `radius` pixels about `center`, from `start` degrees, clockwise on the screen
    fn circle(center: Point, radius: f32, start: f32, count: usize) -> Vec<Point> {
        (0..=count)
            .map(|step| {
                let angle = start.to_radians() + 2.0 * PI * step as f32 / count as f32;

                Point::new(
                    center.x + radius * angle.cos(),
                    center.y + radius * angle.sin(),
                )
            })
            .collect()
    }

    // a check mark: down to the left, then up and out to the right
    fn check() -> Vec<Point> {
        line(&[(0.0, 40.0), (30.0, 80.0), (110.0, 0.0)])
    }

    // a lightning bolt of three strokes
    fn zigzag() -> Vec<Point> {
        line(&[(0.0, 0.0), (80.0, 40.0), (0.0, 80.0), (80.0, 120.0)])
    }

    // the corners joined by many small steps, as a pointer reports them
    fn line(corners: &[(f32, f32)]) -> Vec<Point> {
        let mut points = vec![Point::new(corners[0].0, corners[0].1)];

        for pair in corners.windows(2) {
            let (from, to) = (pair[0], pair[1]);

            for step in 1..=20 {
                let share = step as f32 / 20.0;

                points.push(Point::new(
                    from.0 + share * (to.0 - from.0),
                    from.1 + share * (to.1 - from.1),
                ));
            }
        }

        points
    }

    fn turned(points: &[Point], degrees: f32, scale: f32, by: Point) -> Vec<Point> {
        let (sin, cos) = degrees.to_radians().sin_cos();

        points
            .iter()
            .map(|point| {
                Point::new(
                    scale * (point.x * cos - point.y * sin) + by.x,
                    scale * (point.x * sin + point.y * cos) + by.y,
                )
            })
            .collect()
    }

    // a tiny deterministic wobble, as a hand has
    fn shaky(points: &[Point], amount: f32) -> Vec<Point> {
        points
            .iter()
            .enumerate()
            .map(|(at, point)| {
                let wobble = ((at * 37 % 11) as f32 - 5.0) / 5.0 * amount;

                Point::new(point.x + wobble, point.y - wobble)
            })
            .collect()
    }

    fn template(name: &str, strokes: &[Vec<Point>]) -> Template {
        Template {
            name: name.to_owned(),
            strokes: strokes
                .iter()
                .map(|stroke| normalize(stroke).expect("a stroke long enough"))
                .collect(),
        }
    }

    fn matched(candidate: &[Point], templates: &[Template]) -> Option<String> {
        match recognize(&normalize(candidate)?, templates) {
            Outcome::Match { name, .. } => Some(name.to_owned()),
            _ => None,
        }
    }

    fn all() -> Vec<Template> {
        vec![
            template("circle", &[circle(Point::new(300.0, 300.0), 90.0, 0.0, 60)]),
            template("check", &[check()]),
            template("zigzag", &[zigzag()]),
        ]
    }

    #[test]
    fn a_normalized_stroke_has_the_samples_and_the_unit_reach() {
        let points = normalize(&check()).expect("long enough");
        let (width, height) = reach(&points);

        assert_eq!(points.len(), SAMPLES);
        assert!((width.max(height) - 1.0).abs() < 1e-3);
        assert!(centroid(&points).distance(Point::default()) < 1e-3);
    }

    #[test]
    fn a_drawn_circle_matches_the_circle() {
        let drawn = shaky(&circle(Point::new(800.0, 500.0), 120.0, 0.0, 90), 3.0);

        assert_eq!(matched(&drawn, &all()).as_deref(), Some("circle"));
    }

    #[test]
    fn a_stroke_is_the_same_turned_scaled_and_moved() {
        let templates = all();

        for degrees in [-40.0, 25.0, 90.0, 180.0] {
            for scale in [0.5, 1.0, 2.5] {
                let moved = turned(&check(), degrees, scale, Point::new(700.0, 200.0));

                assert_eq!(
                    matched(&moved, &templates).as_deref(),
                    Some("check"),
                    "turned {degrees}, scaled {scale}"
                );
            }
        }

        let moved = turned(
            &circle(Point::new(0.0, 0.0), 90.0, 0.0, 60),
            70.0,
            1.7,
            Point::new(50.0, 900.0),
        );
        assert_eq!(matched(&moved, &templates).as_deref(), Some("circle"));
    }

    #[test]
    fn another_shape_is_none_of_them() {
        let only_circle = vec![template(
            "circle",
            &[circle(Point::new(300.0, 300.0), 90.0, 0.0, 60)],
        )];

        assert_eq!(matched(&zigzag(), &only_circle), None);
        assert_eq!(matched(&check(), &only_circle), None);

        // a straight line is not a check mark either
        let only_check = vec![template("check", &[check()])];
        assert_eq!(
            matched(&line(&[(0.0, 0.0), (200.0, 0.0)]), &only_check),
            None
        );
    }

    #[test]
    fn two_alike_gestures_are_ambiguous_for_a_stroke_between_them() {
        let templates = vec![
            template("one", &[circle(Point::new(0.0, 0.0), 90.0, 0.0, 60)]),
            template("two", &[circle(Point::new(500.0, 0.0), 90.0, 0.0, 60)]),
        ];
        let candidate = normalize(&circle(Point::new(0.0, 0.0), 90.0, 0.0, 60)).expect("long");

        assert_eq!(recognize(&candidate, &templates), Outcome::Ambiguous);

        // one gesture alone is not ambiguous with itself
        assert!(matches!(
            recognize(&candidate, &templates[..1]),
            Outcome::Match { name: "one", .. }
        ));
    }

    #[test]
    fn a_gesture_scores_as_its_best_stroke() {
        let wide = turned(&check(), 0.0, 1.0, Point::default());
        let templates = vec![template("check", &[zigzag(), wide])];

        assert_eq!(matched(&check(), &templates).as_deref(), Some("check"));
    }

    #[test]
    fn no_templates_recognize_nothing() {
        let candidate = normalize(&check()).expect("long");

        assert_eq!(recognize(&candidate, &[]), Outcome::Unknown);
    }

    #[test]
    fn an_accident_of_the_hand_is_no_stroke() {
        assert_eq!(normalize(&[]), None);
        assert_eq!(normalize(&[Point::new(5.0, 5.0)]), None);

        // a click that slid a few pixels
        assert_eq!(normalize(&line(&[(10.0, 10.0), (20.0, 14.0)])), None);

        // not a number
        assert_eq!(
            normalize(&[Point::new(f32::NAN, 0.0), Point::new(100.0, 100.0)]),
            None
        );
    }
}
