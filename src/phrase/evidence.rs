//! Crop-owned literal observations and explicitly transformed matching evidence.

use super::*;

/// An actual pre-trim observation, never a second phrase entry.
#[derive(Clone, Debug)]
pub struct OriginalRead {
    /// Original polygon in photo coordinates.
    pub quad: Quad,
    /// Exact level crop read, including margin and any half turn.
    pub crop: GrayImage,
    /// Verbatim OCR of that crop.
    pub raw: LineRead,
}

/// The input selected for the unchanged hybrid/near-list policy.
#[derive(Clone, Debug)]
pub struct MatchingRead {
    /// Text before ordinary hybrid normalization, not literal OCR.
    pub text: String,
    /// True if this refers to OriginalRead, false for WordBox.raw.
    pub original: bool,
    /// Leading UTF-8 bytes removed from the owning literal.
    pub removed_prefix_bytes: usize,
}

/// An observation of a label, independent of a trusted sequence number.
#[derive(Clone, Debug)]
pub struct LabelRead {
    /// Source geometry; no word-index link to lose during reindexing.
    pub corners: [(f32, f32); 4],
    /// Verbatim whole/token read, including punctuation.
    pub literal: String,
    /// Whole prefix, leading token, standalone label, or beside-word label.
    pub origin: String,
    /// Exact positive digits only; never a guessed letter-to-digit mapping.
    pub ordinal: Option<u32>,
    /// Positive mandatory dotted-label evidence.
    pub dotted: bool,
    /// More than one equally near geometric owner; no attachment was chosen.
    pub ambiguous: bool,
}

/// Layout and raster ownership independent of recognition confidence. The
/// historical name also covers a local prefix or compact fragment assembly;
/// those have no invented grid column, row or ordinal.
#[derive(Clone, Debug)]
pub struct CellSupport {
    /// Zero-based grid column, absent for non-grid ownership.
    pub column: Option<usize>,
    /// Zero-based row within the column, absent for non-grid ownership.
    pub row: Option<usize>,
    /// Inferred list position; this does not confirm the selected word.
    pub ordinal: Option<u32>,
    /// Cell domain in canonical photo coordinates, not the recognition crop.
    pub quad: [(f32, f32); 4],
    /// Measured word-side ink pixels remaining after ruling-line removal.
    pub ink_pixels: usize,
    /// Stable diagnostic explanation of the geometric support.
    pub basis: &'static str,
}

impl CellSupport {
    /// Structured explanation for exported readings and the box report.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({"column":self.column,"row":self.row,"ordinal":self.ordinal,
            "quad":self.quad,"ink_pixels":self.ink_pixels,"basis":self.basis})
    }
}

/// Label observations and at most one retained pre-trim OCR observation.
#[derive(Clone, Debug, Default)]
pub struct WordEvidence {
    /// Independent geometric support; does not confirm a recognition guess.
    pub cell: Option<CellSupport>,
    /// All observations, not a last-write-wins label.
    pub labels: Vec<LabelRead>,
    /// Contradictory exact ordinal observations for this word, or a duplicate.
    pub conflict: bool,
    /// Literal observation before an adopted cut.
    pub original: Option<OriginalRead>,
    /// Explicit source/transform supplied to the hybrid.
    pub matching: Option<MatchingRead>,
    /// Opt-in history of the actual crop/reader and selection decisions.
    pub decisions: Option<DecisionTrace>,
}

impl WordEvidence {
    /// Diagnostic serialization shared by CLI consumers. Original crop pixels
    /// are exported separately as original_crop, not fabricated from this box.
    pub fn to_json(&self) -> serde_json::Value {
        let mut value = serde_json::json!({
            "cell": self.cell.as_ref().map(CellSupport::to_json),
            "labels": self.labels.iter().map(|l| serde_json::json!({
                "corners": l.corners, "literal": l.literal, "origin": l.origin,
                "ordinal": l.ordinal, "dotted": l.dotted, "ambiguous": l.ambiguous,
            })).collect::<Vec<_>>(),
            "conflict": self.conflict,
            "matching": self.matching.as_ref().map(|m| serde_json::json!({
                "text": m.text, "original": m.original, "removed_prefix_bytes": m.removed_prefix_bytes,
            })),
            "original": self.original.as_ref().map(|r| serde_json::json!({
                "corners": r.quad.0, "raw": r.raw.text, "confidence": r.raw.confidence,
                "crop_size": [r.crop.width(), r.crop.height()],
            })),
        });
        if let Some(trace) = &self.decisions {
            value["decisions"] = trace.to_json();
        }
        value
    }
}

impl Prepared {
    /// Change crop and literal together. A rejected proposal never reaches here.
    pub(super) fn adopt(&mut self, cut: u32, after: Past, margin: f32) -> OriginalRead {
        let quad = self.quad.clone();
        self.quad = narrowed_in(&quad, cut as f32, margin, self.writing);
        DecisionTrace::push(&mut self.decisions, || {
            serde_json::json!({"rule":"adopt_cut", "status":"evaluated",
            "cut_x":cut, "before_quad":quad.0, "after_quad":self.quad.0,
            "before_size":[self.crop.width(), self.crop.height()], "after_size":[after.crop.width(), after.crop.height()]})
        });
        let crop = std::mem::replace(&mut self.crop, after.crop);
        self.ranked = after.ranked;
        let raw = self
            .read
            .replace(after.read)
            .expect("trim requires whole OCR");
        OriginalRead { quad, crop, raw }
    }
}

#[cfg(test)]
pub(super) fn dotted_matching(whole: &str, current: Option<&str>) -> Option<MatchingRead> {
    dotted_matching_traced(whole, current, &mut None)
}

pub(super) fn dotted_matching_traced(
    whole: &str,
    current: Option<&str>,
    trace: &mut Option<DecisionTrace>,
) -> Option<MatchingRead> {
    let matching = crate::numbering::dotted::matching(whole, current);
    DecisionTrace::push(trace, || {
        serde_json::json!({"rule":"dotted_matching", "status":"evaluated",
        "whole_literal":whole, "adopted_cut_literal":current,
        "matching":matching.map(|m| serde_json::json!({"text":m.text,"origin":format!("{:?}",m.origin),
            "removed_prefix_bytes":m.removed_prefix_bytes,"reason":format!("{:?}",m.reason)}))})
    });
    matching.map(|m| MatchingRead {
        text: m.text.to_owned(),
        original: current.is_some() && m.origin == crate::numbering::dotted::MatchOrigin::Whole,
        removed_prefix_bytes: m.removed_prefix_bytes,
    })
}

/// A cut needs token corroboration, not merely a dot somewhere in a whole read.
/// The existing mask supplies the gap. Never infer a second cut from a remainder.
pub(super) fn dotted_cut(whole: &str, token: Option<&str>) -> bool {
    use crate::numbering::dotted;
    let Some(token) = token else { return false };
    dotted::token(token).is_some()
        || dotted::prefix(whole).is_some_and(|p| {
            // Whole OCR may mistake a digit for a letter (B/8). A numeric
            // token corroborates the label ROLE, not its ordinal.
            (!p.head.is_empty() && token.trim().eq_ignore_ascii_case(p.head))
                || matches!(label_of(token), Some(Label::Exact(_)))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adopted_crop_keeps_both_literals_and_selects_original_hobby_evidence() {
        let whole = Quad([(0., 0.), (80., 0.), (80., 20.), (0., 20.)]);
        let mut p = Prepared {
            cell: None,
            writing: WritingFrame::Local,
            decisions: None,
            quad: whole.clone(),
            crop: GrayImage::new(80, 20),
            ranked: vec![(0, 0.4)],
            turned: false,
            read: Some(LineRead {
                text: "9.Hobby".into(),
                confidence: 0.8,
            }),
            cut: Some(12),
            token: None,
        };
        let original = p.adopt(
            12,
            Past {
                crop: GrayImage::new(68, 20),
                ranked: vec![(0, 0.5)],
                read: LineRead {
                    text: ". Hobby".into(),
                    confidence: 0.7,
                },
            },
            CROP_MARGIN,
        );
        let matching =
            dotted_matching(&original.raw.text, p.read.as_ref().map(|r| r.text.as_str())).unwrap();
        assert_eq!(original.quad, whole);
        assert_eq!(original.crop.width(), 80);
        assert_eq!(original.raw.text, "9.Hobby");
        assert_eq!(p.read.as_ref().unwrap().text, ". Hobby");
        assert_eq!(p.crop.width(), 68);
        assert_ne!(p.quad, original.quad);
        assert_eq!(matching.text, "Hobby");
        assert!(matching.original);
        assert_eq!(matching.removed_prefix_bytes, 2);
        let word = finish_evidenced(
            p,
            &Calibration {
                min_prob: 0.9,
                min_margin: 0.8,
                digest: String::new(),
                rule: CANON_RULE.into(),
            },
            true,
            false,
            None,
            WordEvidence {
                original: Some(original),
                matching: Some(matching),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            Word::from_index(word.pick().unwrap() as u16)
                .unwrap()
                .as_str(),
            "hobby"
        );
        assert_eq!(word.raw.as_ref().unwrap().text, ". Hobby");
        assert!(!word.stray);
        let json = word.evidence.to_json();
        assert_eq!(json["original"]["raw"], "9.Hobby");
        assert_eq!(json["matching"]["original"], true);
    }

    #[test]
    fn cut_needs_its_own_token_and_state_comes_only_from_a_real_read() {
        assert!(dotted_cut(".5tate", Some("7.")));
        assert!(dotted_cut("G.corn", Some("G")));
        assert!(dotted_cut("B.BANANA", Some("8")));
        assert!(dotted_cut("G.corn", Some("6")));
        assert!(dotted_cut("", Some("7.")));
        assert!(!dotted_cut("BANANA", Some("8")));
        assert!(!dotted_cut("B.BANANA", Some("")));
        assert!(!dotted_cut("B.BANANA", None));
        assert!(!dotted_cut("co.rn", Some("c")));
        assert!(!dotted_cut(".5tate", Some("5tate")));
        assert_eq!(dotted_matching(".5tate", None).unwrap().text, "5tate");
        let m = dotted_matching(".5tate", Some("State")).unwrap();
        assert_eq!(m.text, "State");
        assert!(!m.original);
    }

    #[test]
    fn actual_word_only_state_read_reaches_the_hybrid_without_rewriting_ocr() {
        let p = Prepared {
            cell: None,
            writing: WritingFrame::Local,
            decisions: None,
            quad: Quad([(0., 0.), (40., 0.), (40., 20.), (0., 20.)]),
            crop: GrayImage::new(40, 20),
            ranked: vec![(Word::from_bip39("estate").unwrap().index() as usize, 0.4)],
            turned: false,
            read: Some(LineRead {
                text: "State".into(),
                confidence: 0.8,
            }),
            token: None,
            cut: None,
        };
        let word = finish_evidenced(
            p,
            &Calibration {
                min_prob: 0.9,
                min_margin: 0.8,
                digest: String::new(),
                rule: CANON_RULE.into(),
            },
            true,
            false,
            None,
            WordEvidence {
                matching: dotted_matching(".5tate", Some("State")),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            Word::from_index(word.pick().unwrap() as u16)
                .unwrap()
                .as_str(),
            "state"
        );
        assert_eq!(word.raw.unwrap().text, "State");
        assert!(!word.stray);
    }
}
