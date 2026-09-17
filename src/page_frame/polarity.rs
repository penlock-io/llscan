//! One page-level polarity decision. On unknown/steep pages, an exact word
//! witness in only one anchor read can justify polarity; confidence alone cannot.
//! This uses the known vocabulary only for direction, never box/word selection.

use super::{AxisEvidence, Direction, PageAxis};
use crate::phrase::DecisionTrace;
use crate::recogniser::{LineRead, Recogniser};
use crate::split::{level_crop_in, upside_down};
use crate::vocabulary::Word;
use image::{GrayImage, RgbImage};
use serde_json::json;

/// Provenance of the canonical photo's top, separate from its pixel transform.
/// In particular, an identity transform does not imply known photo-up. This is
/// a caller hint, not proof that the paper itself was photographed right way up.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PhotoUp {
    /// Legacy caller, stored raster or missing/invalid metadata.
    #[default]
    Unknown,
    /// Capture-time camera metadata, including rotation zero.
    Camera,
    /// Valid file EXIF orientation, including orientation 1.
    Exif,
    /// Caller explicitly declares these canonical pixels upright.
    Explicit,
}

impl PhotoUp {
    /// Stable source name for diagnostics; no numeric transform is inferred.
    pub fn source(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Camera => "camera",
            Self::Exif => "exif",
            Self::Explicit => "explicit",
        }
    }

    /// Whether the caller supplied evidence of photo-up.
    pub fn known(self) -> bool {
        self != Self::Unknown
    }
}

/// Why the page uses its base direction or the opposite polarity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolarityReason {
    /// Trust known photo-up when the measured axis is within 45° of horizontal.
    PhotoUp,
    /// Empty/invalid/square geometry; use canonical horizontal without a read.
    NoMeasuredAxis,
    /// A fallback read was needed, but no recogniser was supplied.
    NoRecogniser,
    /// Only the base anchor read contains a usable exact BIP39 word witness.
    BaseRead,
    /// Only the reversed anchor read contains a usable exact BIP39 word witness.
    ReversedRead,
    /// Both or neither orientation has a word witness; keep base unresolved.
    InconclusiveReads,
    /// Neither read has nonblank text and finite confidence in [0, 1]; keep base.
    UnusableReads,
}

impl PolarityReason {
    fn label(self) -> &'static str {
        match self {
            Self::PhotoUp => "known_photo_up_near_horizontal",
            Self::NoMeasuredAxis => "no_measured_axis_canonical_horizontal",
            Self::NoRecogniser => "no_recogniser_base_unresolved",
            Self::BaseRead => "base_anchor_only_word_witness",
            Self::ReversedRead => "reversed_anchor_only_word_witness",
            Self::InconclusiveReads => "inconclusive_anchor_reads_keep_base",
            Self::UnusableReads => "unusable_anchor_reads_keep_base",
        }
    }
}

/// The immutable page choice to reuse for all generic-page geometry and reads.
/// Ordinary scans keep no OCR strings/history here after choosing polarity.
#[derive(Clone, Debug)]
pub struct PageDirection {
    direction: Direction,
    /// The signal/fallback which actually chose the direction.
    pub reason: PolarityReason,
    /// Zero on skipped paths; exactly two on a successful OCR fallback.
    pub orientation_reads: usize,
    /// Opt-in evidence for the page decision, separate from individual word reads.
    pub decisions: Option<DecisionTrace>,
}

impl PageDirection {
    /// All subsequent regions must use this direction, even if taller than wide.
    pub fn direction(&self) -> Direction {
        self.direction
    }
}

impl PageAxis {
    /// Resolve polarity once on already-canonical pixels. No EXIF parsing or
    /// canonical-photo transform occurs here. `margin` is the word-crop margin.
    /// Fallback performs at most two OCR calls, no classifier/detector calls;
    /// errors abort instead of becoming a preference for the other orientation.
    pub fn resolve(
        &self,
        photo: &RgbImage,
        photo_up: PhotoUp,
        recogniser: Option<&Recogniser>,
        margin: f32,
        trace: bool,
    ) -> Result<PageDirection, String> {
        self.resolve_with(
            photo,
            photo_up,
            recogniser.map(|r| move |crop: &GrayImage| r.read(crop)),
            margin,
            trace,
        )
    }

    // The real crop/call owner, injectable for tests without loading models.
    fn resolve_with(
        &self,
        photo: &RgbImage,
        photo_up: PhotoUp,
        mut recognise: Option<impl FnMut(&GrayImage) -> Result<LineRead, String>>,
        margin: f32,
        trace: bool,
    ) -> Result<PageDirection, String> {
        if !margin.is_finite() || margin < 0. {
            return Err("page direction requires a finite nonnegative crop margin".into());
        }
        let steep = self.angle_degrees().abs() > 45.;
        let mut observations = None;
        let (branch, reason) = if self.evidence != AxisEvidence::LargestRectangle {
            ("geometry_fallback", PolarityReason::NoMeasuredAxis)
        } else if photo_up.known() && !steep {
            ("photo_up", PolarityReason::PhotoUp)
        } else {
            let branch = if photo_up.known() {
                "anchor_ocr_steep_axis"
            } else {
                "anchor_ocr_unknown_photo_up"
            };
            let reason = if let Some(recognise) = recognise.as_mut() {
                let anchor = self.anchor.as_ref().expect("measured axis has an anchor");
                let frame = self
                    .direction(false)
                    .frame(&anchor.quad, margin)
                    .ok_or("invalid page orientation anchor crop")?;
                let crop = level_crop_in(photo, frame);
                let mut read = |crop: &GrayImage, polarity| {
                    let _call = crate::timing::span("page_frame.polarity_ocr");
                    recognise(crop).map_err(|e| format!("page polarity {polarity} anchor OCR: {e}"))
                };
                let base = read(&crop, "base")?;
                let reversed = read(&upside_down(&crop), "reversed")?;
                let reason = if usable_confidence(&base).is_none()
                    && usable_confidence(&reversed).is_none()
                {
                    PolarityReason::UnusableReads
                } else {
                    match (word_witness(&base), word_witness(&reversed)) {
                        (Some(_), None) => PolarityReason::BaseRead,
                        (None, Some(_)) => PolarityReason::ReversedRead,
                        _ => PolarityReason::InconclusiveReads,
                    }
                };
                observations = Some((frame, crop.dimensions(), base, reversed));
                reason
            } else {
                PolarityReason::NoRecogniser
            };
            (branch, reason)
        };
        let reversed = reason == PolarityReason::ReversedRead;
        let direction = self.direction(reversed);
        let orientation_reads = if observations.is_some() { 2 } else { 0 };
        let mut decisions = trace.then(DecisionTrace::default);
        DecisionTrace::push(&mut decisions, || {
            let anchor = self.anchor.as_ref().map(|a| {
                json!({
                    "source_id":a.source_id, "quad":a.quad.0,
                    "rectangle":{"cx":a.rectangle.cx, "cy":a.rectangle.cy,
                        "width":a.rectangle.w, "height":a.rectangle.h,
                        "angle_degrees":a.rectangle.angle},
                })
            });
            let reads = observations.as_ref().map(|(frame, size, base, other)| {
                json!({"base_frame":{"cx":frame.cx, "cy":frame.cy,
                    "width":frame.w, "height":frame.h, "angle_degrees":frame.angle},
                    "crop_size":[size.0, size.1], "margin":margin,
                    "base":observation(base), "reversed":observation(other),
                    "reverse_transform":"rotate_base_crop_180",
                    "comparison":"exact_bip39_word_witness_in_only_one_orientation"})
            });
            json!({"rule":"page_writing_direction", "status":"evaluated",
                "coordinate_frame":"canonical_photo", "anchor":anchor,
                "axis_reason":self.evidence.reason(), "axis_angle_degrees":self.angle_degrees(),
                "photo_up":{"source":photo_up.source(), "known":photo_up.known()},
                "steep_axis":{"threshold_degrees":45.0, "comparison":"abs(axis) > threshold", "result":steep},
                "branch":branch, "reason":reason.label(),
                "orientation_reads":orientation_reads, "anchor_reads":reads,
                "reversed":reversed, "direction_degrees":direction.angle_degrees(),
                "level_rotation_degrees_ccw":-direction.angle_degrees(),
                "polarity_supported":matches!(reason, PolarityReason::PhotoUp | PolarityReason::BaseRead | PolarityReason::ReversedRead),
            })
        });
        Ok(PageDirection {
            direction,
            reason,
            orientation_reads,
            decisions,
        })
    }
}

fn usable_confidence(read: &LineRead) -> Option<f32> {
    (!read.text.trim().is_empty()
        && read.confidence.is_finite()
        && (0. ..=1.).contains(&read.confidence))
    .then_some(read.confidence)
}

/// Literal token boundaries plus at most one existing label prefix. In
/// particular, no hybrid normalization, spelling repair or recursive stripping
/// can turn a substring or a digit-like letter into an orientation witness.
fn word_witness(read: &LineRead) -> Option<&str> {
    usable_confidence(read)?;
    read.text.split_whitespace().find_map(|token| {
        let word = if let Some(prefix) = crate::numbering::dotted::prefix(token) {
            prefix.remainder
        } else {
            crate::numbering::past_label(token).unwrap_or(token)
        };
        Word::from_bip39(word).map(|_| word)
    })
}

fn observation(read: &LineRead) -> serde_json::Value {
    json!({"literal":read.text, "confidence":read.confidence,
        "confidence_finite":read.confidence.is_finite(),
        "nonfinite_confidence":(!read.confidence.is_finite()).then(|| read.confidence.to_string()),
        "literal_nonblank":!read.text.trim().is_empty(),
        "usable":usable_confidence(read).is_some(),
        "word_witness":word_witness(read)})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::Quad;
    use image::Rgb;

    fn axis(angle: f32) -> PageAxis {
        let mut axis =
            PageAxis::from_detections(&[Quad([(20., 20.), (60., 20.), (60., 30.), (20., 30.)])]);
        // Isolate the policy's exact 45° boundary from min-area-rect rounding.
        // Projection/rotation from real quads is covered by page_frame's tests.
        axis.angle = angle;
        axis
    }

    fn read(text: &str, confidence: f32) -> LineRead {
        LineRead {
            text: text.into(),
            confidence,
        }
    }

    fn forbidden(_: &GrayImage) -> Result<LineRead, String> {
        panic!("this branch must not call an orientation reader")
    }

    fn event(direction: &PageDirection) -> serde_json::Value {
        direction.decisions.as_ref().unwrap().to_json()["events"][0].clone()
    }

    #[test]
    fn known_photo_up_through_45_degrees_never_reads_an_anchor() {
        for source in [PhotoUp::Camera, PhotoUp::Exif, PhotoUp::Explicit] {
            for angle in [-45., -12., 0., 12., 45.] {
                let page = axis(angle)
                    .resolve_with(&RgbImage::new(0, 0), source, Some(forbidden), 0.15, true)
                    .unwrap();
                assert_eq!(page.reason, PolarityReason::PhotoUp);
                assert_eq!(page.direction().angle_degrees(), angle);
                assert_eq!(page.orientation_reads, 0);
                let e = event(&page);
                assert_eq!(e["photo_up"]["source"], source.source());
                assert_eq!(e["branch"], "photo_up");
                assert_eq!(e["steep_axis"]["result"], false);
                assert!(e["anchor_reads"].is_null());
            }
        }
        assert_eq!(PhotoUp::default(), PhotoUp::Unknown);
    }

    #[test]
    fn unknown_or_steep_pages_read_exactly_base_then_its_half_turn() {
        let photo = RgbImage::from_fn(80, 60, |x, y| Rgb([(x + 2 * y) as u8; 3]));
        let original = photo.clone();
        for (source, angle, branch) in [
            (PhotoUp::Unknown, 0., "anchor_ocr_unknown_photo_up"),
            (PhotoUp::Unknown, 45., "anchor_ocr_unknown_photo_up"),
            (PhotoUp::Camera, 45.001, "anchor_ocr_steep_axis"),
            (PhotoUp::Exif, -45.001, "anchor_ocr_steep_axis"),
            (PhotoUp::Explicit, 90., "anchor_ocr_steep_axis"),
        ] {
            let axis = axis(angle);
            let frame = axis
                .direction(false)
                .frame(&axis.anchor.as_ref().unwrap().quad, 0.15)
                .unwrap();
            let base = level_crop_in(&photo, frame);
            let expected = [base.clone(), upside_down(&base)];
            assert_ne!(expected[0], expected[1]);
            for trace in [false, true] {
                let mut calls = 0;
                let result = axis
                    .resolve_with(
                        &photo,
                        source,
                        Some(|crop: &GrayImage| {
                            assert_eq!(crop, &expected[calls]);
                            calls += 1;
                            Ok(read(
                                if calls == 1 { "7." } else { "basket" },
                                calls as f32 / 4.,
                            ))
                        }),
                        0.15,
                        trace,
                    )
                    .unwrap();
                assert_eq!(calls, 2);
                assert_eq!(result.orientation_reads, 2);
                assert_eq!(result.reason, PolarityReason::ReversedRead);
                assert_eq!(result.direction().angle_degrees(), angle + 180.);
                assert_eq!(result.decisions.is_some(), trace);
                if trace {
                    let e = event(&result);
                    assert_eq!(e["branch"], branch);
                    assert_eq!(e["anchor"]["source_id"], 0);
                    assert_eq!(
                        e["anchor_reads"]["crop_size"],
                        json!([base.width(), base.height()])
                    );
                    assert_eq!(e["anchor_reads"]["base"]["confidence"], 0.25);
                    assert_eq!(e["anchor_reads"]["reversed"]["confidence"], 0.5);
                    assert_eq!(
                        e["level_rotation_degrees_ccw"],
                        json!(-result.direction().angle_degrees())
                    );
                }
            }
        }
        assert_eq!(
            photo, original,
            "canonical pixels must never be rotated in place"
        );
    }

    #[test]
    fn only_one_exact_word_witness_can_decide_regardless_of_confidence_order() {
        let cases = [
            (
                read("7.", 0.8),
                read("basket", 0.5),
                PolarityReason::ReversedRead,
            ),
            (
                read("basket", 0.5),
                read("7.", 0.8),
                PolarityReason::BaseRead,
            ),
            (
                read(" \n\t", 1.),
                read(".", 0.),
                PolarityReason::InconclusiveReads,
            ),
            (
                read("not-a-word", 0.),
                read("", 0.9),
                PolarityReason::InconclusiveReads,
            ),
            (read("basket", 0.), read("", 1.), PolarityReason::BaseRead),
            (
                read("", 1.),
                read("basket", 0.),
                PolarityReason::ReversedRead,
            ),
            (
                read("basket", 0.1),
                read("expand", 0.9),
                PolarityReason::InconclusiveReads,
            ),
            (
                read("basket", 0.9),
                read("expand", 0.1),
                PolarityReason::InconclusiveReads,
            ),
            (
                read("basket", 0.5),
                read("basket", 0.5),
                PolarityReason::InconclusiveReads,
            ),
            (read("", 0.9), read(" ", 1.), PolarityReason::UnusableReads),
            (
                read("basket", f32::NAN),
                read("expand", f32::INFINITY),
                PolarityReason::UnusableReads,
            ),
            (
                read("basket", f32::NEG_INFINITY),
                read("expand", 0.2),
                PolarityReason::ReversedRead,
            ),
            (
                read("basket", 1.01),
                read("expand", 0.2),
                PolarityReason::ReversedRead,
            ),
            (
                read("basket", 0.2),
                read("expand", -0.01),
                PolarityReason::BaseRead,
            ),
        ];
        for (base, reverse, reason) in cases {
            for trace in [false, true] {
                let mut calls = 0;
                let result = axis(0.)
                    .resolve_with(
                        &RgbImage::new(80, 60),
                        PhotoUp::Unknown,
                        Some(|_: &GrayImage| {
                            calls += 1;
                            Ok(if calls == 1 {
                                base.clone()
                            } else {
                                reverse.clone()
                            })
                        }),
                        0.15,
                        trace,
                    )
                    .unwrap();
                assert_eq!(calls, 2);
                assert_eq!(result.reason, reason);
                assert_eq!(
                    result.direction().angle_degrees(),
                    if reason == PolarityReason::ReversedRead {
                        180.
                    } else {
                        0.
                    }
                );
                assert_eq!(result.decisions.is_some(), trace);
                if trace {
                    let e = event(&result);
                    assert_eq!(e["anchor_reads"]["base"]["literal"], base.text);
                    assert_eq!(
                        e["anchor_reads"]["base"]["word_witness"],
                        json!(word_witness(&base))
                    );
                    assert_eq!(
                        e["anchor_reads"]["reversed"]["word_witness"],
                        json!(word_witness(&reverse))
                    );
                    assert_eq!(
                        e["anchor_reads"]["base"]["usable"],
                        usable_confidence(&base).is_some()
                    );
                    assert_eq!(
                        e["polarity_supported"],
                        matches!(
                            reason,
                            PolarityReason::BaseRead | PolarityReason::ReversedRead
                        )
                    );
                    if !base.confidence.is_finite() {
                        assert!(e["anchor_reads"]["base"]["confidence"].is_null());
                        assert_eq!(e["anchor_reads"]["base"]["confidence_finite"], false);
                    }
                }
            }
        }
    }

    #[test]
    fn word_witness_uses_complete_tokens_and_one_label_prefix_not_normalization() {
        for (literal, expected) in [
            ("BaSkEt", Some("BaSkEt")),
            ("9.security", Some("security")),
            ("G.corn", Some("corn")),
            ("7)state", Some("state")),
            (" nonsense\n12.\tSTORY ", Some("STORY")),
            ("1)notaword 2)basket", Some("basket")),
            (".state", Some("state")),
            (".5tate", None),
            ("..state", None),
            ("1.G.corn", None),
            ("notbasket", None),
            ("bask et", None),
            ("co.rn", None),
            ("basket!", None),
            ("7. G. . 12)", None),
            ("eparl prochs", None),
        ] {
            assert_eq!(word_witness(&read(literal, 0.5)), expected, "{literal}");
        }
    }

    #[test]
    fn missing_geometry_and_reader_are_explicit_zero_read_fallbacks() {
        let square = Quad([(0., 0.), (10., 0.), (10., 10.), (0., 10.)]);
        for quads in [vec![], vec![Quad([(f32::NAN, 0.); 4])], vec![square]] {
            let axis = PageAxis::from_detections(&quads);
            for source in [PhotoUp::Unknown, PhotoUp::Camera] {
                let result = axis
                    .resolve_with(&RgbImage::new(0, 0), source, Some(forbidden), 0.15, true)
                    .unwrap();
                assert_eq!(result.reason, PolarityReason::NoMeasuredAxis);
                assert_eq!(result.direction().angle_degrees(), 0.);
                assert_eq!(result.orientation_reads, 0);
                assert_eq!(event(&result)["axis_reason"], axis.evidence.reason());
                assert_eq!(event(&result)["photo_up"]["known"], source.known());
            }
        }
        for (source, angle) in [(PhotoUp::Unknown, 0.), (PhotoUp::Camera, 90.)] {
            // Public wrapper, no recogniser/model instance or inferred provenance.
            let result = axis(angle)
                .resolve(&RgbImage::new(0, 0), source, None, 0.15, true)
                .unwrap();
            assert_eq!(result.reason, PolarityReason::NoRecogniser);
            assert_eq!(result.direction().angle_degrees(), angle);
            assert_eq!(result.orientation_reads, 0);
            assert_eq!(event(&result)["polarity_supported"], false);
        }
    }

    #[test]
    fn reader_errors_abort_without_retry_or_a_fallback_vote() {
        for fail_on in [1, 2] {
            let mut calls = 0;
            let error = axis(0.)
                .resolve_with(
                    &RgbImage::new(80, 60),
                    PhotoUp::Unknown,
                    Some(|_: &GrayImage| {
                        calls += 1;
                        if calls == fail_on {
                            Err("reader failed".into())
                        } else {
                            Ok(read("valid", 0.8))
                        }
                    }),
                    0.15,
                    true,
                )
                .unwrap_err();
            assert_eq!(calls, fail_on);
            assert_eq!(
                error,
                format!(
                    "page polarity {} anchor OCR: reader failed",
                    if fail_on == 1 { "base" } else { "reversed" }
                )
            );
        }
    }

    #[test]
    fn invalid_margin_is_rejected_before_any_crop_or_read() {
        for margin in [-0.1, f32::NAN, f32::INFINITY] {
            assert!(
                axis(0.)
                    .resolve_with(
                        &RgbImage::new(0, 0),
                        PhotoUp::Unknown,
                        Some(forbidden),
                        margin,
                        false,
                    )
                    .is_err()
            );
        }
    }
}
