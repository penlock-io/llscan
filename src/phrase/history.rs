//! Record actual replacement outcomes, retaining rejected reader evidence and
//! explicit links. This module does not decide whether a replacement is allowed.

use super::{DecisionTrace, PageScan, WordBox};
use crate::progress::FinalRegion;
use serde_json::{Value, json};

pub(crate) fn reading(word: &WordBox) -> Value {
    json!({"quad":word.quad.0,"ranked":super::decisions::ranks(&word.ranked),
        "selection":word.selection.as_ref().map(|s| s.diagnostic()),
        "kept":!word.stray,"number":word.number,"label":word.label,
        "apart":word.apart,"narrowed":word.narrowed,
        "is_join":word.joined_from.is_some(),"is_expansion":!word.expanded_from.is_empty(),
        "raw":word.raw.as_ref().map(|r| json!({"literal":r.text,"confidence":r.confidence}))})
}

pub(crate) fn links(parents: &[usize], candidate: Option<usize>) -> Value {
    json!(
        parents
            .iter()
            .map(|&p| json!({"role":"parent","word_index":p}))
            .chain(candidate.map(|p| json!({"role":"candidate","word_index":p})))
            .collect::<Vec<_>>()
    )
}

/// Build once, only if someone is collecting; the closure reads the state at
/// the call site before any mutation, not a reconstruction from final outputs.
pub(crate) fn record(
    scan: &mut PageScan,
    parents: &[usize],
    make: impl FnOnce(&PageScan) -> Value,
) {
    if !parents
        .iter()
        .any(|&p| scan.words[p].evidence.decisions.is_some())
    {
        return;
    }
    let event = make(scan);
    for &p in parents {
        DecisionTrace::push(&mut scan.words[p].evidence.decisions, || event.clone());
    }
}

/// Attach a read's actual acceptance/refusal and operands to both sides. If the
/// candidate is discarded, its nested history stays on the parent; it never
/// becomes that parent's crop orientation or literal OCR.
pub(crate) fn read_trial(
    scan: &mut PageScan,
    parents: &[usize],
    candidate: &mut WordBox,
    candidate_index: Option<usize>,
    owner: &str,
    reason: &str,
    limits: impl FnOnce() -> Value,
) {
    if candidate.evidence.decisions.is_none()
        && !parents
            .iter()
            .any(|&p| scan.words[p].evidence.decisions.is_some())
    {
        return;
    }
    let event = json!({"rule":"replacement_read","status":"evaluated","owner":owner,"reason":reason,
        "links":links(parents,candidate_index),"parents":parents.iter().map(|&p| reading(&scan.words[p])).collect::<Vec<_>>(),
        "candidate":reading(candidate),"candidate_retained":candidate_index.is_some(),
        "discarded_read_history":if candidate_index.is_none() {candidate.evidence.decisions.as_ref().map(DecisionTrace::to_json)} else {None},
        "comparisons":limits(),"evaluation":"first failed guard returns the recorded reason; subsequent guards are skipped"});
    for &p in parents {
        DecisionTrace::push(&mut scan.words[p].evidence.decisions, || event.clone());
    }
    DecisionTrace::push(&mut candidate.evidence.decisions, || event);
}

impl WordBox {
    pub(crate) fn set_stray_recorded(&mut self, stray: bool, owner: &str, reason: &str) {
        let before = self.stray;
        self.stray = stray;
        DecisionTrace::push(&mut self.evidence.decisions, || {
            json!({"rule":"keep_transition","status":"evaluated",
            "owner":owner,"reason":reason,"before_kept":!before,"after_kept":!self.stray})
        });
    }
}

/// Called after every native repair/reindex owner has finished. Does not rerun
/// eligibility, order choice or checksum selection; it records their result.
pub fn terminal(scan: &mut PageScan, regions: &[FinalRegion], scope: &str) {
    for (index, word) in scan.words.iter_mut().enumerate() {
        if word.evidence.decisions.is_none() {
            continue;
        }
        let event = json!({"rule":"terminal","status":"evaluated","scope":scope,
            "links":[{"role":"observation","word_index":index}],
            "region_id":regions.iter().find(|r| r.word_index as usize == index).map(|r| r.id),
            "kept":!word.stray,"column_rank":word.column_rank,"row_rank":word.row_rank,
            "number":word.number,"selection":word.selection.as_ref().map(|s| s.diagnostic())});
        DecisionTrace::push(&mut word.evidence.decisions, || event);
    }
}

#[cfg(test)]
pub(crate) fn without_history(scan: &PageScan) -> String {
    let mut plain = scan.clone();
    for word in &mut plain.words {
        word.evidence.decisions = None;
    }
    format!("{plain:?}")
}

#[cfg(test)]
pub(crate) fn enable_fixture(word: &mut WordBox) {
    word.evidence.decisions = Some(DecisionTrace::default());
    DecisionTrace::push(
        &mut word.evidence.decisions,
        || json!({"rule":"fixture_read","status":"evaluated"}),
    );
}
