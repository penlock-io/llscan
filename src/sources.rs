//! Raw detector provenance, retained before empty splits can disappear.
//! Raw/part identities and geometry never change when review words are
//! trimmed, hidden or replaced. Only the link to a returned word is reindexed.

use crate::detect::Quad;
use crate::split::{EmptySplit, Split, split_quad_accounted};
use image::RgbImage;

/// One segmentation part, independent of its current reading or keep state.
/// Grid coalescence appends the same combined region to each contributing raw
/// source, with a shared word link. The original parts remain geometry-only;
/// the coalescence trace records every original source/part in the combination.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    /// Index within this raw detection, before traversal sorting.
    pub part: usize,
    /// Geometry at the split boundary, before numbering trims or replacements.
    pub quad: Quad,
    /// Index of the original observation in the current word vector. A join
    /// or expansion leaves that observation intact, even when it is inactive.
    /// None means geometry-only excluded or superseded ink: no word reading was
    /// fabricated. Column partitioning retains the original part and appends
    /// fresh child parts before recognition; its trace records the relationship.
    pub word_index: Option<usize>,
}

/// The segmentation outcome of one raw source, including unsuccessful splits.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// One or more split parts, linked to their original review observations.
    Parts(Vec<Part>),
    /// No parts. The reason and raw geometry remain available to recovery;
    /// this alone does not establish that the source was noise or a fragment.
    Empty(EmptySplit),
    /// A failed whole-word split read from its original raw quad, not a fake part.
    Rescued {
        /// Why ordinary segmentation did not supply a word.
        reason: EmptySplit,
        /// The actual whole-raw reading in the returned word vector.
        word_index: usize,
    },
}

/// Reading outcome of a detached-ending proposal. Geometry stays in the decision.
#[derive(Clone, Debug, PartialEq)]
pub struct Union {
    /// Actual alternative reading, absent when refused before model execution.
    pub word_index: Option<usize>,
    /// Whether this interpretation was initially selected over its parent.
    pub selected: bool,
    /// Stable refusal/selection reason, independent of the spelling.
    pub reason: &'static str,
}

/// One accepted detector observation; its detector index is its stable ID.
#[derive(Clone, Debug, PartialEq)]
pub struct RawSource {
    /// Index in the accepted detector output, not traversal or final-word order.
    pub detector: usize,
    /// Complete original photo geometry, even if the split loses an ending.
    pub quad: Quad,
    /// Explicit outcome instead of silently extending a list with zero parts.
    pub outcome: Outcome,
    /// Geometry decision; none on a caller that did not enable fragment repair.
    pub decision: Option<crate::fragments::Decision>,
    /// Actual optional union-read result; never a fabricated reading.
    pub union: Option<Union>,
}

impl RawSource {
    /// The source's original split observations, or none for an empty split.
    pub fn parts(&self) -> &[Part] {
        match &self.outcome {
            Outcome::Parts(parts) => parts,
            Outcome::Empty(_) | Outcome::Rescued { .. } => &[],
        }
    }
}

/// Detector-order parts and their complete raw-source ledger. Layout is still
/// owned by PageLayout; this value does not fit or guess a second traversal.
pub struct Segmentation {
    /// Parts in detector/part order, before PageLayout assigns traversal ranks.
    pub words: Vec<Quad>,
    /// Exactly one entry per accepted raw detection, including empty splits.
    pub sources: Vec<RawSource>,
}

impl Segmentation {
    /// Performs the normal splitter once per raw source, recording its outcome.
    pub fn new(photo: &RgbImage, quads: &[Quad]) -> Self {
        Self::with_splitter(quads, |q| split_quad_accounted(photo, q, Split::LOCKED))
    }

    /// Split all raw sources along the already-resolved page direction. Ledger
    /// quads remain canonical, including sources with no resulting parts.
    pub fn in_direction(
        photo: &RgbImage,
        quads: &[Quad],
        direction: crate::page_frame::Direction,
    ) -> Self {
        Self::with_splitter(quads, |q| {
            crate::split::split_in_direction(photo, q, Split::LOCKED, direction)
        })
    }

    fn with_splitter(
        quads: &[Quad],
        mut split: impl FnMut(&Quad) -> Result<Vec<Quad>, EmptySplit>,
    ) -> Self {
        let mut words = Vec::new();
        let sources = quads
            .iter()
            .enumerate()
            .map(|(detector, quad)| {
                let outcome = match split(quad) {
                    Ok(parts) => {
                        assert!(
                            !parts.is_empty(),
                            "accounted splitting must name an empty reason"
                        );
                        let start = words.len();
                        let observations = parts
                            .iter()
                            .enumerate()
                            .map(|(part, quad)| Part {
                                part,
                                quad: quad.clone(),
                                word_index: Some(start + part),
                            })
                            .collect();
                        words.extend(parts);
                        Outcome::Parts(observations)
                    }
                    Err(reason) => Outcome::Empty(reason),
                };
                RawSource {
                    detector,
                    quad: quad.clone(),
                    outcome,
                    decision: None,
                    union: None,
                }
            })
            .collect();
        Self { words, sources }
    }
}

/// Applies an old-to-new word permutation without changing raw/part IDs,
/// original geometry or empty outcomes. Called by the existing traversal,
/// native-join and extent insertion owners, never by a second layout fit.
pub fn reindex(sources: &mut [RawSource], inverse: &[usize]) {
    for source in sources {
        if let Outcome::Parts(parts) = &mut source.outcome {
            for part in parts {
                part.word_index = part.word_index.map(|i| inverse[i]);
            }
        }
        if let Outcome::Rescued { word_index, .. } = &mut source.outcome {
            *word_index = inverse[*word_index];
        }
        if let Some(union) = &mut source.union {
            union.word_index = union.word_index.map(|i| inverse[i]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    fn quad(x: f32, y: f32, w: f32, h: f32) -> Quad {
        Quad([(x, y), (x + w, y), (x + w, y + h), (x, y + h)])
    }

    #[test]
    fn zero_one_and_multiple_parts_keep_all_sources_and_stable_ids() {
        let quads = [
            quad(0., 0., 40., 10.),
            quad(0., 20., 40., 10.),
            quad(0., 40., 40., 10.),
        ];
        let left = quad(0., 40., 15., 10.);
        let right = quad(25., 40., 15., 10.);
        let mut answers = [
            Err(EmptySplit::FlatPhotoSamples),
            Ok(vec![quads[1].clone()]),
            Ok(vec![left.clone(), right.clone()]),
        ]
        .into_iter();
        let mut segmentation = Segmentation::with_splitter(&quads, |_| answers.next().unwrap());
        assert!(answers.next().is_none());
        assert_eq!(segmentation.words, vec![quads[1].clone(), left, right]);
        assert_eq!(segmentation.sources.len(), quads.len());
        for (i, s) in segmentation.sources.iter().enumerate() {
            assert_eq!((s.detector, &s.quad), (i, &quads[i]));
        }
        assert_eq!(
            segmentation.sources[0].outcome,
            Outcome::Empty(EmptySplit::FlatPhotoSamples)
        );
        let before = segmentation.sources.clone();
        reindex(&mut segmentation.sources, &[2, 0, 1]);
        assert_eq!(segmentation.sources[0], before[0]);
        assert_eq!(segmentation.sources[1].parts()[0].word_index, Some(2));
        assert_eq!(segmentation.sources[2].parts()[0].word_index, Some(0));
        assert_eq!(segmentation.sources[2].parts()[1].word_index, Some(1));
        for (old, new) in before.iter().zip(&segmentation.sources) {
            for (a, b) in old.parts().iter().zip(new.parts()) {
                assert_eq!((a.part, &a.quad), (b.part, &b.quad));
            }
        }
    }

    #[test]
    fn the_real_splitter_records_distinct_empty_causes_without_retrying() {
        let mut photo = RgbImage::from_pixel(200, 120, Rgb([80; 3]));
        // Only a long horizontal rule: real thresholded ink, then none.
        for x in 20..100 {
            photo.put_pixel(x, 60, Rgb([20; 3]));
        }
        // A narrow word that remains after rule stripping.
        for y in 90..105 {
            for x in 30..34 {
                photo.put_pixel(x, y, Rgb([20; 3]));
            }
        }
        let quads = [
            quad(20., 10., 1., 1.),
            quad(-100., 10., 50., 20.),
            quad(20., 20., 80., 20.),
            quad(20., 50., 80., 20.),
            quad(20., 85., 80., 25.),
        ];
        let segmentation = Segmentation::new(&photo, &quads);
        for (s, reason) in segmentation.sources.iter().zip([
            EmptySplit::NoFrame,
            EmptySplit::NoPhotoSamples,
            EmptySplit::FlatPhotoSamples,
            EmptySplit::NoInkAfterRules,
        ]) {
            assert_eq!(s.outcome, Outcome::Empty(reason));
        }
        assert_eq!(segmentation.sources.len(), 5);
        assert_eq!(segmentation.words.len(), 1);
        assert_eq!(segmentation.sources[4].parts()[0].word_index, Some(0));
    }
}
