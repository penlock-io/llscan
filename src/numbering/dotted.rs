//! Dotted-label role and matching evidence, independent of ordinal trust.
//!
//! The shared phrase reader applies these rules while retaining crop-owned
//! literal observations and exporting the matching source separately.

use super::{Label, Numbering};
use crate::hybrid::normalise;
use crate::vocabulary::Word;

/// A leading dotted label parsed exactly once, borrowing the literal text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Prefix<'a> {
    /// The zero, one or two alphanumeric characters before the first dot.
    pub head: &'a str,
    /// Everything after that dot and its following whitespace, unmodified.
    pub remainder: &'a str,
    /// Bytes removed from the original literal, including leading whitespace.
    /// This is a text offset, never a crop coordinate.
    pub consumed: usize,
}

impl Prefix<'_> {
    /// Exact positive ASCII digits are observed ordinals. All other dotted
    /// heads retain their mandatory label role with an unknown ordinal.
    pub fn label(&self) -> Label {
        if !self.head.is_empty() && self.head.chars().all(|c| c.is_ascii_digit()) {
            if let Ok(n) = self.head.parse::<u32>() {
                if n > 0 {
                    return Label::Exact(n);
                }
            }
        }
        Label::Shaped
    }
}

/// A dot alone or a one/two-alphanumeric head immediately followed by a dot.
/// Does not recurse into the remainder or strip digit-like word letters.
pub fn prefix(literal: &str) -> Option<Prefix<'_>> {
    let start = literal.trim_start();
    let mut chars = 0;
    for (offset, ch) in start.char_indices() {
        if ch == '.' {
            let remainder = start[offset + 1..].trim_start();
            return Some(Prefix {
                head: &start[..offset],
                remainder,
                consumed: literal.len() - remainder.len(),
            });
        }
        if !ch.is_alphanumeric() || chars == 2 {
            return None;
        }
        chars += 1;
    }
    None
}

/// A complete dotted label token, not an attached prefix plus word.
pub fn token(literal: &str) -> Option<Label> {
    let parsed = prefix(literal)?;
    parsed.remainder.is_empty().then(|| parsed.label())
}

/// Whether label evidence establishes a list, irrespective of sequence trust.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListMode {
    /// Neither positive dotted evidence nor a recognized sequence.
    Unnumbered,
    /// Positive dotted evidence, a held sequence, or contradictory numbers.
    Numbered,
}

impl ListMode {
    /// One dotted observation is sufficient; no count or confidence input.
    pub fn from_evidence(has_dotted_label: bool, sequence: &Numbering) -> Self {
        if has_dotted_label || !matches!(sequence, Numbering::None) {
            Self::Numbered
        } else {
            Self::Unnumbered
        }
    }
}

/// Which actual crop observation supplies the matching text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchOrigin {
    /// The original whole-crop literal observation.
    Whole,
    /// The already read, adopted word-only crop; not a speculative rejected cut.
    WordOnly,
}

/// Why a matching observation was selected. This is not a new OCR result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchReason {
    /// The label-cleaned original is already an exact vocabulary word.
    CleanedWholeWord,
    /// Without a safe cut, the dot may be internal noise in a correct word.
    /// Preserve that whole literal while retaining the dotted label evidence.
    WholeWordWithoutCut,
    /// An adopted crop supplied its own literal read.
    WordOnly,
    /// There was no adopted cut; use the one-time cleaned original remainder.
    CleanedWhole,
    /// No dotted prefix is present in the literal (e.g. a separate label).
    Whole,
}

/// Text selected for hybrid matching, explicitly tied to a literal observation.
/// The caller retains that literal and its actual crop/geometry unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Matching<'a> {
    /// Text before ordinary hybrid normalization; digit-like letters survive.
    pub text: &'a str,
    /// Observation that owns these bytes.
    pub origin: MatchOrigin,
    /// Bytes omitted at the start of that literal, zero for a verbatim read.
    /// Nonzero values must be presented as a transformation, never raw OCR.
    pub removed_prefix_bytes: usize,
    /// Explicit selection/refusal rationale.
    pub reason: MatchReason,
}

/// Select matching evidence for an already identified dotted-label word.
/// `word_only` must be an adopted crop's actual read, not a rejected proposal.
/// A complete label-only literal returns no word evidence. This function
/// performs no OCR, confidence check, geometric cut or ordinal assignment.
pub fn matching<'a>(whole: &'a str, word_only: Option<&'a str>) -> Option<Matching<'a>> {
    let parsed = prefix(whole);
    let exact_word = |text: &str| Word::from_bip39(&normalise(text)).is_some();
    if let Some(p) = parsed {
        if p.remainder.is_empty() {
            return None;
        }
        // Without an adopted geometric cut, a second valid word in the
        // remainder is not evidence to prefer it (ab.use must not become use).
        if word_only.is_none() && exact_word(whole) {
            return Some(Matching {
                text: whole,
                origin: MatchOrigin::Whole,
                removed_prefix_bytes: 0,
                reason: MatchReason::WholeWordWithoutCut,
            });
        }
        if exact_word(p.remainder) {
            return Some(Matching {
                text: p.remainder,
                origin: MatchOrigin::Whole,
                removed_prefix_bytes: p.consumed,
                reason: MatchReason::CleanedWholeWord,
            });
        }
    }
    if let Some(literal) = word_only {
        // A cut may leave the same label's dot at the crop edge. Remove at
        // most that dot, never another alphanumeric head from the word.
        let text = literal
            .trim_start()
            .strip_prefix('.')
            .map(str::trim_start)
            .unwrap_or(literal);
        return Some(Matching {
            text,
            origin: MatchOrigin::WordOnly,
            removed_prefix_bytes: literal.len() - text.len(),
            reason: MatchReason::WordOnly,
        });
    }
    Some(match parsed {
        Some(p) => Matching {
            text: p.remainder,
            origin: MatchOrigin::Whole,
            removed_prefix_bytes: p.consumed,
            reason: MatchReason::CleanedWhole,
        },
        None => Matching {
            text: whole,
            origin: MatchOrigin::Whole,
            removed_prefix_bytes: 0,
            reason: MatchReason::Whole,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dotted_roles_do_not_require_a_digit_shape_or_a_known_ordinal() {
        for (text, label, remainder) in [
            ("G.corn", Label::Shaped, "corn"),
            ("s.riot", Label::Shaped, "riot"),
            ("H.young", Label::Shaped, "young"),
            ("1.dice", Label::Exact(1), "dice"),
            ("12. sunny", Label::Exact(12), "sunny"),
            (".5tate", Label::Shaped, "5tate"),
            ("0.word", Label::Shaped, "word"),
            ("A.word", Label::Shaped, "word"),
            ("２.word", Label::Shaped, "word"),
        ] {
            let p = prefix(text).unwrap();
            assert_eq!((p.label(), p.remainder), (label, remainder), "{text}");
            assert_eq!(&text[p.consumed..], remainder);
        }
        for text in [".", "H.", "G.", "s.", "0.", "H. \t"] {
            assert_eq!(token(text), Some(Label::Shaped), "{text}");
            assert_eq!(matching(text, None), None, "label-only {text}");
        }
        assert_eq!(token("12."), Some(Label::Exact(12)));
        assert_eq!(token("H.young"), None);
    }

    #[test]
    fn prefix_offsets_are_utf8_safe_and_never_consume_a_second_head() {
        let text = " \tÉ.  5tate";
        let p = prefix(text).unwrap();
        assert_eq!(p.head, "É");
        assert_eq!(p.remainder, "5tate");
        assert_eq!(&text[p.consumed..], "5tate");
        assert_eq!(prefix(".5.tate").unwrap().remainder, "5.tate");
        assert_eq!(prefix("..state").unwrap().remainder, ".state");
    }

    #[test]
    fn undotted_reads_and_long_word_heads_do_not_trigger_the_new_rule() {
        for text in [
            "", "word.", "can.cel", "5ILK", "word!", "7) state", "7 .state", "123.word",
        ] {
            assert_eq!(prefix(text), None, "{text}");
        }
    }

    #[test]
    fn list_mode_never_changes_trusted_order_or_repair_permission() {
        let held = Numbering::Held {
            numbers: vec![Some(1)],
            count: 1,
            missing: vec![],
            out_of_place: vec![],
            unresolved: vec![],
        };
        let inconsistent = Numbering::Inconsistent {
            detail: "duplicate".into(),
        };
        for (sequence, held_expected, repair_expected) in [
            (Numbering::None, false, true),
            (held, true, false),
            (inconsistent, false, false),
        ] {
            for dotted in [false, true] {
                let mode = ListMode::from_evidence(dotted, &sequence);
                assert_eq!(
                    mode == ListMode::Numbered,
                    dotted || !matches!(sequence, Numbering::None)
                );
                assert_eq!(sequence.has_held_sequence(), held_expected);
                assert_eq!(sequence.permits_word_repair(), repair_expected);
            }
        }
    }

    #[test]
    fn two_letter_dot_inside_a_correct_word_keeps_it_when_no_cut_is_safe() {
        for text in ["co.rn", "st.ate", "be.ach"] {
            assert!(prefix(text).is_some());
            assert_eq!(
                ListMode::from_evidence(true, &Numbering::None),
                ListMode::Numbered
            );
            let selected = matching(text, None).unwrap();
            assert_eq!(selected.text, text);
            assert_eq!(selected.removed_prefix_bytes, 0);
            assert_eq!(selected.reason, MatchReason::WholeWordWithoutCut);
            assert!(Word::from_bip39(&normalise(selected.text)).is_some());
        }
    }

    #[test]
    fn matching_preserves_useful_original_evidence_without_faking_literal_ocr() {
        for (whole, word_only, expected) in [
            ("9.Hobby", ".Hobby", "Hobby"),
            ("G.corn", "com", "corn"),
            ("s.riot", "riat", "riot"),
        ] {
            let chosen = matching(whole, Some(word_only)).unwrap();
            assert_eq!(chosen.text, expected);
            assert_eq!(chosen.origin, MatchOrigin::Whole);
            assert_eq!(chosen.reason, MatchReason::CleanedWholeWord);
            assert_eq!(&whole[chosen.removed_prefix_bytes..], expected);
        }
    }

    #[test]
    fn no_cut_preserves_the_whole_word_even_when_the_remainder_is_another_word() {
        for (literal, whole_word, shorter_word) in [
            ("ab.use", "abuse", "use"),
            ("al.one", "alone", "one"),
            ("a.gain", "again", "gain"),
        ] {
            let parsed = prefix(literal).unwrap();
            assert_eq!(parsed.remainder, shorter_word);
            assert_eq!(normalise(literal), whole_word);
            assert!(Word::from_bip39(whole_word).is_some());
            assert!(Word::from_bip39(shorter_word).is_some());
            assert_eq!(parsed.label(), Label::Shaped);
            assert_eq!(
                ListMode::from_evidence(true, &Numbering::None),
                ListMode::Numbered
            );

            let uncut = matching(literal, None).unwrap();
            assert_eq!(uncut.text, literal);
            assert_eq!(uncut.origin, MatchOrigin::Whole);
            assert_eq!(uncut.removed_prefix_bytes, 0);
            assert_eq!(uncut.reason, MatchReason::WholeWordWithoutCut);

            // An adopted word-only cut still permits the existing cleaned
            // original selection; the preservation guard is not unconditional.
            let cut = matching(literal, Some(shorter_word)).unwrap();
            assert_eq!(cut.text, shorter_word);
            assert_eq!(cut.origin, MatchOrigin::Whole);
            assert_eq!(cut.removed_prefix_bytes, parsed.consumed);
            assert_eq!(cut.reason, MatchReason::CleanedWholeWord);
        }
        let numeric = matching("1.dice", None).unwrap();
        assert_eq!(numeric.text, "1.dice");
        assert_eq!(normalise(numeric.text), "dice");
        assert_eq!(numeric.reason, MatchReason::WholeWordWithoutCut);
    }

    #[test]
    fn state_requires_the_actual_word_only_observation_not_a_substitution() {
        let uncut = matching(".5tate", None).unwrap();
        assert_eq!(uncut.text, "5tate");
        assert_eq!(uncut.reason, MatchReason::CleanedWhole);
        let cut = matching(".5tate", Some("State")).unwrap();
        assert_eq!(cut.text, "State");
        assert_eq!(cut.origin, MatchOrigin::WordOnly);
        assert_eq!(cut.removed_prefix_bytes, 0);
        let still_wrong = matching(".5tate", Some(".5tate")).unwrap();
        assert_eq!(still_wrong.text, "5tate");
        assert_eq!(still_wrong.removed_prefix_bytes, 1);
        let another_dot = matching(".5tate", Some("co.rn")).unwrap();
        assert_eq!(another_dot.text, "co.rn", "do not strip another head");
    }
}
