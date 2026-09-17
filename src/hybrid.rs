//! The entrant's raw read against the list: what `scripts/words/hybrid.py`
//! pins, ported. A raw read is normalised to lowercase letters, its
//! distance to a list word is a weighted edit distance where an edit at
//! the first character costs more, and a read is near the list when
//! some word lies within the budget. The weights are the pinned
//! `CURRENT` set and never change here; a new set is a new plan.

use crate::vocabulary::Word;

pub mod scoring;
mod selection;
pub use selection::{
    Branch, Candidate, Confirmation, Selection, SelectionError, Support, TextSource, select,
    select_bounded,
};

/// The pinned edit weights and budget.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Weights {
    /// Swapping one character.
    pub substitute: f32,
    /// Dropping a raw character.
    pub delete: f32,
    /// Adding one the word has.
    pub insert: f32,
    /// An edit that consumes the raw read's first character, or inserts
    /// ahead of it, costs this many times more.
    pub first: f32,
    /// A read within this distance of a word is near the list.
    pub budget: f32,
}

/// `hybrid.CURRENT`.
pub const CURRENT: Weights = Weights {
    substitute: 1.0,
    delete: 2.0,
    insert: 1.0,
    first: 2.0,
    budget: 3.0,
};

/// The raw read as a candidate spelling: lowercase letters only.
pub fn normalise(raw: &str) -> String {
    raw.to_lowercase()
        .chars()
        .filter(char::is_ascii_lowercase)
        .collect()
}

/// Weighted edit distance from the raw read to the word.
pub fn distance(raw: &str, word: &str, w: &Weights) -> f32 {
    let raw: Vec<char> = raw.chars().collect();
    let word: Vec<char> = word.chars().collect();
    let mut prev: Vec<f32> = (0..=word.len())
        .map(|j| match j {
            0 => 0.0,
            _ => w.insert * w.first + (j as f32 - 1.0) * w.insert,
        })
        .collect();
    for (i, &ca) in raw.iter().enumerate() {
        let f = if i == 0 { w.first } else { 1.0 };
        let mut cur = Vec::with_capacity(word.len() + 1);
        cur.push(w.delete * w.first + i as f32 * w.delete);
        for (j, &cb) in word.iter().enumerate() {
            let swap = if ca == cb { 0.0 } else { w.substitute * f };
            let v = (prev[j + 1] + w.delete * f)
                .min(cur[j] + w.insert)
                .min(prev[j] + swap);
            cur.push(v);
        }
        prev = cur;
    }
    prev[word.len()]
}

/// The smallest distance from the normalised read to any list word,
/// with that word; `None` for an empty read.
pub fn closest(read: &str, w: &Weights) -> Option<(Word, f32)> {
    if read.is_empty() {
        return None;
    }
    Word::all()
        .map(|word| (word, distance(read, word.as_str(), w)))
        .min_by(|a, b| a.1.total_cmp(&b.1))
}

/// Historical distance-first pick, retained for the incumbent audits and golden.
/// Product callers use `select`. The raw read normalised is a list word, take it;
/// else the closest of the model's five within the budget, ties to
/// the more probable and then the earlier word; else the model's top.
/// `top` is the model's ranked list, most probable first, of which the
/// first five are looked at.
pub fn pick(raw: &str, top: &[(usize, f32)], w: &Weights) -> (usize, String) {
    let read = normalise(raw);
    if let Some(word) = Word::from_bip39(&read) {
        return (usize::from(word.index()), "raw is a list word".into());
    }
    let ours = top.first().map_or(0, |t| t.0);
    if read.is_empty() {
        return (ours, "no raw read: ours".into());
    }
    let scored = top.iter().take(5).map(|&(i, p)| {
        let word = Word::from_index(i as u16).map_or("", |w| w.as_str());
        (distance(&read, word, w), -p, word, i)
    });
    let Some((d, _, _, i)) = scored.min_by(|a, b| {
        a.0.total_cmp(&b.0)
            .then(a.1.total_cmp(&b.1))
            .then(a.2.cmp(b.2))
    }) else {
        return (ours, "no ranked words: ours".into());
    };
    if d <= w.budget {
        (i, format!("closest of our five, {d} edits"))
    } else {
        (
            ours,
            format!("nothing within {} (closest {d}): ours", w.budget),
        )
    }
}

/// Whether the raw read is a list word or within the budget of one.
pub fn near_list(raw: &str, w: &Weights) -> bool {
    near_list_evidence(raw, w).within_budget
}

/// Operands of the same predicate, available without a second distance search.
pub(crate) struct NearListEvidence {
    pub(crate) normalized: String,
    pub(crate) witness: Option<(Word, f32)>,
    pub(crate) within_budget: bool,
}

pub(crate) fn near_list_evidence(raw: &str, w: &Weights) -> NearListEvidence {
    let read = normalise(raw);
    // closest explicitly returns None for an empty read, regardless of weights
    // or vocabulary lengths; empty input never qualifies through arithmetic.
    let exact = Word::from_bip39(&read);
    let witness = exact.map(|word| (word, 0.0)).or_else(|| closest(&read, w));
    NearListEvidence {
        normalized: read,
        witness,
        // Preserve the exact-word shortcut even for diagnostic custom weights.
        within_budget: exact.is_some() || witness.is_some_and(|(_, d)| d <= w.budget),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_read_is_rejected_independently_of_distance_weights() {
        for cost in [0.0, -1.0, 0.1] {
            let weights = Weights {
                substitute: cost,
                delete: cost,
                insert: cost,
                first: 1.0,
                budget: 100.0,
            };
            for raw in ["", "12.", "...", " "] {
                assert!(!near_list(raw, &weights));
                assert!(near_list_evidence(raw, &weights).witness.is_none());
            }
        }
    }

    #[test]
    fn normalising_keeps_lowercase_letters_only() {
        assert_eq!(normalise("1. Walk!"), "walk");
        assert_eq!(normalise("12."), "");
        assert_eq!(normalise("5ILK"), "ilk");
    }

    #[test]
    fn the_distance_is_the_pinned_weighted_edit_distance() {
        let w = &CURRENT;
        assert_eq!(distance("walk", "walk", w), 0.0);
        assert_eq!(distance("wolk", "walk", w), 1.0);
        assert_eq!(distance("wak", "walk", w), 1.0);
        assert_eq!(distance("alk", "walk", w), 2.0);
        assert_eq!(distance("ilk", "silk", w), 2.0);
        assert_eq!(distance("velvet", "helmet", w), 3.0);
    }

    // The fixture is the stage's own picks over the sealed set, as
    // `harness word-score --recogniser` emitted them, so this holds
    // the pick to what it did on every crop the day the fixture was
    // made: a change to CURRENT or to the rule shows here by crop.

    #[test]
    fn a_read_is_near_the_list_within_the_budget() {
        let w = &CURRENT;
        assert!(near_list("walk", w));
        assert!(near_list("1. walk", w));
        assert!(near_list("wolk", w));
        assert!(near_list("5ILK", w));
        assert!(!near_list("12.", w));
        assert!(!near_list("", w));
        assert_eq!(
            closest("wolk", w).map(|(word, d)| (word.as_str(), d)),
            Some(("walk", 1.0))
        );
    }
}
