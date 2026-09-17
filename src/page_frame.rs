//! One computational writing frame for a uniformly written page.
//!
//! Geometry establishes an axis, not upright polarity. The largest valid raw
//! text rectangle owns the axis; resolve its polarity once and use that same
//! direction for words, labels, layout and later repair reads. The resolver
//! prefers known photo-up and only reads the anchor when that is inconclusive.
//! It never calls a detector or alters canonical pixels.

use crate::detect::Quad;
use crate::split::{Frame, frame_of, turn};

mod polarity;
pub use polarity::{PageDirection, PhotoUp, PolarityReason};

/// Explicit geometry context. Local frames remain only for the registered-field
/// and historical geometry APIs; generic scans supply one shared direction.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) enum WritingFrame {
    #[default]
    Local,
    Page(Direction),
}

impl WritingFrame {
    pub(crate) fn shared(self) -> bool {
        matches!(self, Self::Page(_))
    }

    pub(crate) fn frame_of(self, quad: &Quad) -> Frame {
        match self {
            Self::Local => frame_of(quad),
            Self::Page(d) => d.frame(quad, 0.).unwrap_or(Frame {
                // Invalid input stays invalid for split/scale/read preflights;
                // never fall back to choosing a different per-box direction.
                cx: f32::NAN,
                cy: f32::NAN,
                w: f32::NAN,
                h: f32::NAN,
                angle: d.angle_degrees(),
            }),
        }
    }

    pub(crate) fn crop_frame(self, quad: &Quad, margin: f32) -> Frame {
        if let Self::Local = self {
            return frame_of(&crate::split::with_margin(quad, margin));
        }
        let mut f = self.frame_of(quad);
        let pad = margin * f.h;
        f.w += 2. * pad;
        f.h += 2. * pad;
        f
    }

    pub(crate) fn with_margin(self, quad: &Quad, margin: f32) -> Quad {
        if let Self::Local = self {
            return crate::split::with_margin(quad, margin);
        }
        let f = self.crop_frame(quad, margin);
        let corners = [
            (-f.w / 2., -f.h / 2.),
            (f.w / 2., -f.h / 2.),
            (f.w / 2., f.h / 2.),
            (-f.w / 2., f.h / 2.),
        ]
        .map(|(x, y)| {
            let (x, y) = turn(x, y, f.angle);
            (x + f.cx, y + f.cy)
        });
        Quad(crate::detect::clockwise(corners))
    }

    pub(crate) fn project(self, quad: &Quad) -> Quad {
        match self {
            Self::Local => quad.clone(),
            Self::Page(d) => d.quad_to_page(quad),
        }
    }

    pub(crate) fn unproject(self, quad: &Quad) -> Quad {
        match self {
            Self::Local => quad.clone(),
            Self::Page(d) => d.quad_to_photo(quad),
        }
    }

    pub(crate) fn angle_or(self, local: f32) -> f32 {
        match self {
            Self::Local => local,
            Self::Page(d) => d.angle_degrees(),
        }
    }
}

/// The original raw detection which supplies the page axis.
#[derive(Clone, Debug, PartialEq)]
pub struct Anchor {
    /// Index in the original accepted detector output, before any filtering.
    pub source_id: usize,
    /// Original canonical-photo corners, not an annotation or split word.
    pub quad: Quad,
    /// The valid minimum-area rectangle, with its longer side first.
    pub rectangle: Frame,
}

/// Evidence establishing the axis, including explicit geometric fallbacks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AxisEvidence {
    /// The largest valid rectangle supplies its long-side axis.
    LargestRectangle,
    /// The anchor is numerically square, so canonical horizontal is the fallback.
    SquareAnchor,
    /// No valid raw detections; canonical horizontal at the origin is the fallback.
    NoValidDetections,
}

impl AxisEvidence {
    /// Stable diagnostic reason; neither fallback claims a measured direction.
    pub fn reason(self) -> &'static str {
        match self {
            Self::LargestRectangle => "largest_rectangle",
            Self::SquareAnchor => "square_anchor_canonical_horizontal",
            Self::NoValidDetections => "no_valid_detections_canonical_horizontal",
        }
    }
}

/// An undirected page axis. No OCR result or per-region longest-side vote enters it.
#[derive(Clone, Debug, PartialEq)]
pub struct PageAxis {
    /// The largest valid raw rectangle, or none on empty/degenerate input.
    pub anchor: Option<Anchor>,
    /// Whether the axis was measured or fell back to canonical horizontal.
    pub evidence: AxisEvidence,
    angle: f32,
    origin: (f32, f32),
}

impl PageAxis {
    /// Select greatest minimum-rectangle area, breaking exact ties by lowest
    /// original detector index. Invalid entries do not renumber later sources.
    pub fn from_detections(quads: &[Quad]) -> Self {
        let mut anchor: Option<Anchor> = None;
        for (source_id, quad) in quads.iter().enumerate() {
            if !valid_quad(quad) {
                continue;
            }
            let rectangle = frame_of(quad);
            if !valid_frame(rectangle) {
                continue;
            }
            let area = |f: Frame| f64::from(f.w) * f64::from(f.h);
            if anchor
                .as_ref()
                .is_none_or(|a| area(rectangle) > area(a.rectangle))
            {
                anchor = Some(Anchor {
                    source_id,
                    quad: quad.clone(),
                    rectangle,
                });
            }
        }
        let Some(a) = &anchor else {
            return Self {
                anchor,
                evidence: AxisEvidence::NoValidDetections,
                angle: 0.,
                origin: (0., 0.),
            };
        };
        // Only floating-point indistinguishability, not an aspect-ratio heuristic
        // or a reason to hunt for a different anchor. Keep the requested largest.
        let square = (a.rectangle.w - a.rectangle.h).abs() <= 8. * f32::EPSILON * a.rectangle.w;
        let angle = if square { 0. } else { a.rectangle.angle };
        let origin = (a.rectangle.cx, a.rectangle.cy);
        Self {
            anchor,
            evidence: if square {
                AxisEvidence::SquareAnchor
            } else {
                AxisEvidence::LargestRectangle
            },
            angle,
            origin,
        }
    }

    /// Undirected angle, anticlockwise in the canonical photo, in (-90, 90].
    pub fn angle_degrees(&self) -> f32 {
        self.angle
    }

    /// Apply the caller's one page-wide polarity choice. A rectangle cannot
    /// decide `reversed`; false is the explicitly unresolved/base orientation.
    pub fn direction(&self, reversed: bool) -> Direction {
        Direction {
            origin: self.origin,
            angle: self.angle + if reversed { 180. } else { 0. },
        }
    }
}

/// A resolved computational direction. It maps geometry, not the displayed photo.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Direction {
    origin: (f32, f32),
    angle: f32,
}

impl Direction {
    /// Signed photo-frame angle; leveling applies its negative. May include 180°.
    pub fn angle_degrees(self) -> f32 {
        self.angle
    }

    /// Canonical-photo point to computational page point (x along writing).
    pub fn to_page(self, (x, y): (f32, f32)) -> (f32, f32) {
        turn(x - self.origin.0, y - self.origin.1, -self.angle)
    }

    /// Computational page point back to the original canonical-photo frame.
    pub fn to_photo(self, (x, y): (f32, f32)) -> (f32, f32) {
        let (x, y) = turn(x, y, self.angle);
        (x + self.origin.0, y + self.origin.1)
    }

    /// Project corners without reordering their identities or choosing a local axis.
    pub fn quad_to_page(self, quad: &Quad) -> Quad {
        Quad(quad.0.map(|p| self.to_page(p)))
    }

    /// Inverse projection, retaining corner identity for canonical output geometry.
    pub fn quad_to_photo(self, quad: &Quad) -> Quad {
        Quad(quad.0.map(|p| self.to_photo(p)))
    }

    /// Bounding rectangle in this direction, even when it is taller than wide.
    /// Padding uses this frame's height. No region can swap axes or polarity here.
    pub fn frame(self, quad: &Quad, margin: f32) -> Option<Frame> {
        if !valid_quad(quad) || !margin.is_finite() || margin < 0. {
            return None;
        }
        let points = self.quad_to_page(quad).0;
        if !points.iter().all(|(x, y)| x.is_finite() && y.is_finite()) {
            return None;
        }
        let (mut l, mut t, mut r, mut b) = (
            f32::INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
        );
        for (x, y) in points {
            l = l.min(x);
            r = r.max(x);
            t = t.min(y);
            b = b.max(y);
        }
        let (cx, cy) = self.to_photo(((l + r) / 2., (t + b) / 2.));
        let pad = margin * (b - t);
        let frame = Frame {
            cx,
            cy,
            w: r - l + 2. * pad,
            h: b - t + 2. * pad,
            angle: self.angle,
        };
        valid_frame(frame).then_some(frame)
    }
}

fn valid_frame(f: Frame) -> bool {
    [f.cx, f.cy, f.w, f.h, f.angle]
        .iter()
        .all(|v| v.is_finite())
        && f.w > 0.
        && f.h > 0.
}

fn valid_quad(q: &Quad) -> bool {
    if !q.0.iter().all(|(x, y)| x.is_finite() && y.is_finite()) {
        return false;
    }
    let area: f64 = (0..4)
        .map(|i| {
            let (x, y) = q.0[i];
            let (u, v) = q.0[(i + 1) % 4];
            f64::from(x) * f64::from(v) - f64::from(y) * f64::from(u)
        })
        .sum();
    area.is_finite() && area != 0.
}

#[cfg(test)]
pub(crate) fn test_direction(angle: f32) -> Direction {
    let raw = Quad(
        [(-120., -20.), (120., -20.), (120., 20.), (-120., 20.)].map(|(x, y)| {
            let (x, y) = turn(x, y, angle);
            (x + 256., y + 256.)
        }),
    );
    let axis = PageAxis::from_detections(&[raw]);
    axis.direction((angle - axis.angle_degrees()).to_radians().cos() < 0.)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::split::level_crop_in;
    use image::{Rgb, RgbImage};

    fn quad(x: f32, y: f32, w: f32, h: f32) -> Quad {
        Quad([(x, y), (x + w, y), (x + w, y + h), (x, y + h)])
    }
    fn close(a: f32, b: f32) {
        assert!((a - b).abs() < 0.002, "{a} != {b}");
    }

    #[test]
    fn largest_area_not_longest_width_and_ties_keep_original_source_id() {
        let invalid = Quad([(f32::NAN, 0.); 4]);
        let quads = [
            invalid,
            quad(0., 0., 100., 10.),
            quad(200., 100., 60., 30.),
            quad(400., 100., 60., 30.),
        ];
        let axis = PageAxis::from_detections(&quads);
        let anchor = axis.anchor.as_ref().unwrap();
        assert_eq!(anchor.source_id, 2);
        assert_eq!(anchor.quad, quads[2]);
        assert_eq!(axis.evidence, AxisEvidence::LargestRectangle);
    }

    #[test]
    fn tall_label_and_wide_word_share_direction_through_skew_and_quarter_half_turns() {
        let word = quad(40., 60., 180., 30.);
        let label = quad(15., 60., 10., 40.);
        for degrees in [0., 17., 90., 107., 180., 197., 270.] {
            let rotate = |q: &Quad| {
                Quad(q.0.map(|(x, y)| {
                    let (x, y) = turn(x, y, degrees);
                    (x + 500., y + 500.)
                }))
            };
            let quads = [rotate(&label), rotate(&word)];
            let axis = PageAxis::from_detections(&quads);
            assert!(axis.angle_degrees() > -90. && axis.angle_degrees() <= 90.);
            assert_eq!(axis.anchor.as_ref().unwrap().source_id, 1);
            for reversed in [false, true] {
                let d = axis.direction(reversed);
                let wf = d.frame(&quads[1], 0.).unwrap();
                let lf = d.frame(&quads[0], 0.).unwrap();
                close(wf.w, 180.);
                close(wf.h, 30.);
                close(lf.w, 10.);
                close(lf.h, 40.);
                assert_eq!(wf.angle, lf.angle);
                let grown = d.frame(&quads[0], 0.15).unwrap();
                close(grown.w, 22.);
                close(grown.h, 52.);
                for q in &quads {
                    let back = d.quad_to_photo(&d.quad_to_page(q));
                    for (a, b) in back.0.iter().zip(q.0) {
                        close(a.0, b.0);
                        close(a.1, b.1);
                    }
                }
            }
        }
    }

    #[test]
    fn canonical_pixels_keep_their_direction_and_polarity_is_only_page_wide() {
        let mut photo = RgbImage::from_pixel(100, 70, Rgb([255; 3]));
        // An asymmetric upright mark, not a learned/template-reader fixture.
        for y in 30..50 {
            for x in 10..18 {
                if y < 33 || x >= 15 {
                    photo.put_pixel(x, y, Rgb([0; 3]));
                }
            }
        }
        let label = quad(10., 30., 8., 20.);
        let axis = PageAxis::from_detections(&[quad(30., 10., 60., 12.), label.clone()]);
        let original = photo.clone();
        let upright = level_crop_in(&photo, axis.direction(false).frame(&label, 0.).unwrap());
        assert_eq!(upright.dimensions(), (8, 20));
        assert_eq!(upright.get_pixel(0, 0).0, [0]);
        assert_eq!(upright.get_pixel(0, 19).0, [255]);
        let flipped = level_crop_in(&photo, axis.direction(true).frame(&label, 0.).unwrap());
        assert_eq!(flipped, image::imageops::rotate180(&upright));
        assert_eq!(photo, original);
    }

    #[test]
    fn invalid_inputs_and_square_anchor_have_explicit_deterministic_fallbacks() {
        let line = Quad([(0., 0.), (1., 1.), (2., 2.), (3., 3.)]);
        for quads in [
            vec![],
            vec![line.clone()],
            vec![Quad([(f32::INFINITY, 0.); 4])],
        ] {
            let axis = PageAxis::from_detections(&quads);
            assert!(axis.anchor.is_none());
            assert_eq!(axis.evidence, AxisEvidence::NoValidDetections);
            assert_eq!(axis.angle_degrees(), 0.);
            assert_eq!(axis.direction(false).to_photo((4., 9.)), (4., 9.));
            assert!(axis.direction(false).frame(&line, 0.).is_none());
        }
        let square = quad(20., 20., 30., 30.);
        let axis = PageAxis::from_detections(&[square.clone(), quad(0., 0., 50., 10.)]);
        assert_eq!(axis.anchor.as_ref().unwrap().source_id, 0);
        assert_eq!(axis.evidence, AxisEvidence::SquareAnchor);
        for margin in [f32::NAN, f32::INFINITY, -0.1] {
            assert!(axis.direction(false).frame(&square, margin).is_none());
        }
    }
}
