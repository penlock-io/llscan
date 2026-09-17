//! Serializable observations; no confidence is presented as ground truth.
use crate::{ScanResult, Word};
use serde::Serialize;

/// A hybrid score is a ranking statistic, not a probability.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Score {
    /// A finite score; larger wins.
    Finite(f64),
    /// Zero model support results in negative infinity in the log formula.
    NegativeInfinity,
}

/// One classifier candidate, optionally scored by the selection branch.
#[derive(Debug, Serialize)]
pub struct Candidate {
    /// English BIP39 spelling.
    pub word: &'static str,
    /// Original classifier probability, without top-k renormalization.
    pub probability: f32,
    /// None when the executed selection branch did not score this candidate.
    pub hybrid_score: Option<Score>,
    /// Weighted OCR edit distance, when a hybrid comparison ran.
    pub edit_cost: Option<f32>,
}

/// One observed region; native observations also retain the exact crop pixels.
#[derive(Debug, Serialize)]
pub struct Observation {
    /// Index into `ScanResult.page.words`, not a printed label or phrase position.
    pub observation: usize,
    /// Final quadrilateral in the normalized image's pixel coordinates.
    pub quad: [(f32, f32); 4],
    /// Selected spelling; None is never substituted with a fabricated word.
    pub selected: Option<&'static str>,
    /// Whether excluded from the suggested phrase.
    pub excluded: bool,
    /// Printed label ordinal, when supported by the native stage.
    pub label_number: Option<u32>,
    /// Literal read of this exact final crop, before separate matching cleanup.
    pub ocr_text: Option<String>,
    /// OCR sequence confidence, distinct from classifier probability.
    pub ocr_confidence: Option<f32>,
    /// All supplied classifier candidates, in model ranking order.
    pub candidates: Vec<Candidate>,
    /// Native selection policy, branch, support and confirmation reasons.
    pub selection: Option<serde_json::Value>,
    /// Native label/exclusion and optional processing-decision evidence.
    pub evidence: serde_json::Value,
}

/// Stable high-level projection suitable for JSON or UI rendering.
#[derive(Debug, Serialize)]
pub struct Document {
    /// Canonical image dimensions.
    pub width: u32,
    /// Canonical image dimensions.
    pub height: u32,
    /// Native writing direction in image coordinates; not a fresh per-box guess.
    pub writing_direction_degrees: Option<f32>,
    /// Stage-chosen ordering policy, never chosen by checksum.
    pub initial_order: &'static str,
    /// Why the native stage suggested that order.
    pub order_basis: &'static str,
    /// Whether the order lacks sufficient independent supporting evidence.
    pub order_requires_review: bool,
    /// Kept observation indices in suggested phrase order.
    pub ordered_observations: Vec<usize>,
    /// Length/checksum validity only, not proof of correct recognition.
    pub checksum_valid: bool,
    /// All observations, including excluded/restorable originals with evidence.
    pub observations: Vec<Observation>,
}

impl ScanResult {
    /// Projects saved decisions only: does not rerun, crop, rank or reorder.
    pub fn document(&self) -> Document {
        let traversal = self.page.initial_traversal();
        let ordered_observations: Vec<_> = traversal
            .indices
            .iter()
            .copied()
            .filter(|&i| !self.page.words[i].stray)
            .collect();
        let words: Option<Vec<_>> = ordered_observations
            .iter()
            .map(|&i| {
                self.page.words[i]
                    .pick()
                    .and_then(|w| Word::from_index(w as u16))
            })
            .collect();
        let observations =
            self.page
                .words
                .iter()
                .enumerate()
                .map(|(i, w)| {
                    let candidates =
                        w.ranked
                            .iter()
                            .map(|&(index, p)| {
                                let scored = w.selection.as_ref().and_then(|s| {
                                    s.candidates().iter().find(|c| c.word() == index)
                                });
                                Candidate {
                                    word: Word::from_index(index as u16)
                                        .expect("validated model vocabulary")
                                        .as_str(),
                                    probability: p,
                                    hybrid_score: scored.map(|c| {
                                        if c.score().is_finite() {
                                            Score::Finite(c.score())
                                        } else {
                                            Score::NegativeInfinity
                                        }
                                    }),
                                    edit_cost: scored.map(|c| c.distance()),
                                }
                            })
                            .collect();
                    Observation {
                        observation: i,
                        quad: w.quad.0,
                        selected: w
                            .pick()
                            .and_then(|p| Word::from_index(p as u16))
                            .map(|w| w.as_str()),
                        excluded: w.stray,
                        label_number: w.number,
                        ocr_text: w.raw.as_ref().map(|r| r.text.clone()),
                        ocr_confidence: w.raw.as_ref().map(|r| r.confidence),
                        candidates,
                        selection: w.selection.as_ref().map(|s| s.diagnostic()),
                        evidence: w.evidence.to_json(),
                    }
                })
                .collect();
        Document {
            width: self.page.width,
            height: self.page.height,
            writing_direction_degrees: self
                .page
                .page_direction
                .as_ref()
                .map(|d| d.direction().angle_degrees()),
            initial_order: traversal.order.as_str(),
            order_basis: traversal.basis,
            order_requires_review: traversal.requires_review,
            ordered_observations,
            checksum_valid: words.is_some_and(|w| crate::vocabulary::checksum_ok(&w)),
            observations,
        }
    }
}
