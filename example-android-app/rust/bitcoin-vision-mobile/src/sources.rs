//! Geometry-only detector provenance. Unread ink is never a WordReading.

use bitcoin_vision::fragments::{Decision, Ownership};
use bitcoin_vision::sources::{Outcome, RawSource};

/// One original split part, whether or not it was eligible for reading.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct DetectionPart {
    /// Stable index within the raw detection.
    pub part: u32,
    /// Original polygon in canonical photo pixels.
    pub corners: Vec<f32>,
    /// Original reading in PhraseScan.words; absent for suppressed non-word ink.
    pub word_index: Option<u32>,
}

/// A fragment's geometric owner, with its current original-reading link.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct DetectionOwner {
    /// Stable raw detector identity, not a word index.
    pub detector_id: u32,
    /// Stable part ID; absent for a whole-raw recovery.
    pub part: Option<u32>,
    /// Current original reading in PhraseScan.words.
    pub word_index: Option<u32>,
}

/// A detached-ending proposal and its optional actual reading.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct DetectionUnion {
    /// Full union polygon; never the geometry of the original parent's reading.
    pub corners: Vec<f32>,
    /// Rounded, margin-bearing read footprint checked against neighbors.
    pub read_footprint: Vec<f32>,
    /// Actual union reading in PhraseScan.words, absent if not read.
    pub word_index: Option<u32>,
    /// Whether the scanner initially selected this alternative.
    pub selected: bool,
    /// Native selection or refusal reason; the app does not re-evaluate it.
    pub reason: String,
}

/// Every accepted raw detector source, including geometry-only excluded ink.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct DetectionSource {
    /// Stable index in accepted detector output.
    pub detector_id: u32,
    /// Full raw polygon in the same frame as the review photo.
    pub corners: Vec<f32>,
    /// All original split parts, including suppressed ones without readings.
    pub parts: Vec<DetectionPart>,
    /// Why segmentation returned no parts, including a later rescued source.
    pub empty_reason: Option<String>,
    /// Actual whole-raw recovery reading; absent if none was made.
    pub recovered_word: Option<u32>,
    /// Native geometry decision, absent on fixed worksheet fields.
    pub decision: Option<String>,
    /// Measured local writing height, when supported.
    pub local_height: Option<f32>,
    /// Independent raw detector identities supporting the local scale.
    pub support_ids: Vec<u32>,
    /// Unique native owner of excluded ink, when established.
    pub owner: Option<DetectionOwner>,
    /// Optional detached-ending proposal, with its real reading link.
    pub union: Option<DetectionUnion>,
}

fn corners(q: &bitcoin_vision::detect::Quad) -> Vec<f32> {
    q.0.iter().flat_map(|&(x, y)| [x, y]).collect()
}

pub(crate) fn pack(sources: &[RawSource]) -> Vec<DetectionSource> {
    sources
        .iter()
        .map(|source| {
            let decision = source.decision.as_ref();
            let (empty_reason, recovered_word) = match source.outcome {
                Outcome::Parts(_) => (None, None),
                Outcome::Empty(reason) => (Some(reason.as_str().to_owned()), None),
                Outcome::Rescued { reason, word_index } => {
                    (Some(reason.as_str().to_owned()), Some(word_index as u32))
                }
            };
            DetectionSource {
                detector_id: source.detector as u32,
                corners: corners(&source.quad),
                parts: source
                    .parts()
                    .iter()
                    .map(|p| DetectionPart {
                        part: p.part as u32,
                        corners: corners(&p.quad),
                        word_index: p.word_index.map(|i| i as u32),
                    })
                    .collect(),
                empty_reason,
                recovered_word,
                decision: decision.map(|d| d.reason().to_owned()),
                local_height: decision.and_then(Decision::scale).map(|s| s.height),
                support_ids: decision
                    .and_then(Decision::scale)
                    .map(|s| {
                        s.supports
                            .iter()
                            .map(|&i| sources[i].detector as u32)
                            .collect()
                    })
                    .unwrap_or_default(),
                owner: decision.and_then(Decision::owner).map(|o| DetectionOwner {
                    detector_id: sources[o.source].detector as u32,
                    part: o.part.map(|p| p as u32),
                    word_index: o.word_index(sources).map(|i| i as u32),
                }),
                union: match decision {
                    Some(Decision::Tiny {
                        ownership:
                            Ownership::Ending {
                                union, footprint, ..
                            },
                        ..
                    }) => Some(DetectionUnion {
                        corners: corners(union),
                        read_footprint: corners(footprint),
                        word_index: source
                            .union
                            .as_ref()
                            .and_then(|u| u.word_index)
                            .map(|i| i as u32),
                        selected: source.union.as_ref().is_some_and(|u| u.selected),
                        reason: source
                            .union
                            .as_ref()
                            .map_or("not_read", |u| u.reason)
                            .to_owned(),
                    }),
                    _ => None,
                },
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin_vision::detect::Quad;
    use bitcoin_vision::fragments::{Observation, Scale};
    use bitcoin_vision::sources::{Part, Union};
    use bitcoin_vision::split::EmptySplit;

    #[test]
    fn geometry_only_and_rescued_sources_keep_distinct_links_through_packing() {
        let parent = Quad([(10., 20.), (70., 20.), (70., 40.), (10., 40.)]);
        let tiny = Quad([(71., 25.), (75., 25.), (75., 30.), (71., 30.)]);
        let union = Quad([(10., 20.), (75., 20.), (75., 40.), (10., 40.)]);
        let sources = vec![
            RawSource {
                detector: 8,
                quad: parent.clone(),
                outcome: Outcome::Rescued {
                    reason: EmptySplit::NoInkAfterRules,
                    word_index: 3,
                },
                decision: None,
                union: None,
            },
            RawSource {
                detector: 12,
                quad: tiny.clone(),
                outcome: Outcome::Parts(vec![Part {
                    part: 2,
                    quad: tiny.clone(),
                    word_index: None,
                }]),
                decision: Some(Decision::Tiny {
                    scale: Scale {
                        height: 20.,
                        supports: vec![0],
                    },
                    ownership: Ownership::Ending {
                        owner: Observation {
                            source: 0,
                            part: None,
                        },
                        union: union.clone(),
                        footprint: union.clone(),
                    },
                }),
                union: Some(Union {
                    word_index: Some(4),
                    selected: false,
                    reason: "uncertain",
                }),
            },
        ];
        let packed = pack(&sources);
        assert_eq!(packed[0].recovered_word, Some(3));
        assert!(packed[0].parts.is_empty());
        assert_eq!(packed[1].parts[0].word_index, None);
        assert_eq!(packed[1].corners, corners(&tiny));
        assert_eq!(packed[1].support_ids, vec![8]);
        assert_eq!(
            packed[1].owner,
            Some(DetectionOwner {
                detector_id: 8,
                part: None,
                word_index: Some(3)
            })
        );
        let alternative = packed[1].union.as_ref().unwrap();
        assert_eq!(alternative.word_index, Some(4));
        assert_eq!(alternative.corners, corners(&union));
        assert!(!alternative.selected);
        assert_eq!(alternative.reason, "uncertain");
    }
}
