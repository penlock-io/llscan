//! Execution of fragment geometry decisions. Required whole-source reads run
//! before optional unions; both use the very same budget later passed to E3.

use crate::detect::Quad;
use crate::fragments::{Decision, Observation, Ownership};
use crate::layout::PageLayout;
use crate::phrase::history;
use crate::phrase::{ExtentParent, PageScan, WordBox};
use crate::progress::{RegionState, Stage};
use crate::read_budget::Budget;
use crate::recogniser::Preparation;
use crate::sources::{Outcome, RawSource, Union};
use crate::words::Verdict;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

pub(crate) struct Input {
    pub layout: PageLayout,
    pub observations: Vec<Observation>,
    pub quads: Vec<Quad>,
    rescue: Vec<bool>,
}

/// Whether the reserved reads may be regrouped, or must follow input order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Traversal {
    /// A concrete recogniser arm keeps an LRU of compiled plans keyed by crop
    /// width, so its hits, misses and plan builds are a function of the order
    /// the widths arrive in. Regrouping would rewrite that diagnostic. Those
    /// arms are built for diagnosis only, and so is this traversal.
    #[cfg(any(test, feature = "scan-profile"))]
    InInputOrder,
    /// The independent reads first and up to `workers` at a time, the budgeted
    /// rescues last.
    Grouped { workers: usize },
}

impl Traversal {
    pub(crate) fn for_recogniser(preparation: Option<Preparation>) -> Self {
        match preparation {
            None | Some(Preparation::Symbolic) => Self::Grouped {
                workers: default_workers(),
            },
            #[cfg(feature = "scan-profile")]
            Some(_) => Self::InInputOrder,
        }
    }
}

impl Input {
    /// Coalesce ordinary parts before reading. Each contributing raw source
    /// retains its old geometry and gets a fresh part linked to the one shared
    /// reading. No discarded fragment receives an invented independent read.
    pub(crate) fn coalesce_cells(
        &mut self,
        sources: &mut [RawSource],
        writing: crate::page_frame::WritingFrame,
        groups: &[(Vec<usize>, Quad)],
    ) -> Option<Vec<Vec<usize>>> {
        if groups.is_empty() {
            return None;
        }
        let mut owner = vec![None; self.quads.len()];
        for (g, (members, _)) in groups.iter().enumerate() {
            assert!(members.len() >= 2);
            for &i in members {
                assert!(!self.rescue[i] && owner[i].is_none());
                owner[i] = Some(g);
            }
        }
        let mut entries = Vec::new();
        for i in 0..self.quads.len() {
            let Some(g) = owner[i] else {
                entries.push((
                    self.observations[i],
                    self.quads[i].clone(),
                    self.rescue[i],
                    vec![i],
                ));
                continue;
            };
            let (members, q) = &groups[g];
            if i != members[0] {
                continue;
            }
            let mut primary = None;
            let mut seen = std::collections::HashSet::new();
            for &parent in members {
                let o = self.observations[parent];
                let Outcome::Parts(parts) = &mut sources[o.source].outcome else {
                    unreachable!()
                };
                parts
                    .iter_mut()
                    .find(|p| Some(p.part) == o.part)
                    .unwrap()
                    .word_index = None;
                if seen.insert(o.source) {
                    let part = parts.iter().map(|p| p.part).max().unwrap() + 1;
                    parts.push(crate::sources::Part {
                        part,
                        quad: q.clone(),
                        word_index: Some(i),
                    });
                    primary.get_or_insert(Observation {
                        source: o.source,
                        part: Some(part),
                    });
                }
            }
            entries.push((primary.unwrap(), q.clone(), false, members.clone()));
        }
        let quads: Vec<_> = entries.iter().map(|e| e.1.clone()).collect();
        let layout = PageLayout::with_writing(&quads, writing);
        let order = layout.column_order();
        let mut inverse = vec![0; self.quads.len()];
        for (new, &e) in order.iter().enumerate() {
            for &old in &entries[e].3 {
                inverse[old] = new;
            }
        }
        crate::sources::reindex(sources, &inverse);
        self.observations = order.iter().map(|&i| entries[i].0).collect();
        self.quads = order.iter().map(|&i| entries[i].1.clone()).collect();
        self.rescue = order.iter().map(|&i| entries[i].2).collect();
        self.layout = layout.reindexed(&order);
        Some(order.iter().map(|&i| entries[i].3.clone()).collect())
    }

    /// Replace a spanning split part with column intersections. Original parts
    /// remain in the ledger without a fabricated reading; fresh part IDs name
    /// the children. Returned parent indices also drive live-region identity.
    pub(crate) fn partition_columns(
        &mut self,
        sources: &mut [RawSource],
        writing: crate::page_frame::WritingFrame,
        mut split: impl FnMut(&Quad) -> Vec<Quad>,
    ) -> Option<Vec<usize>> {
        let mut entries = Vec::new();
        let mut changed = false;
        for (i, &o) in self.observations.iter().enumerate() {
            // Failed-mask raw rescues retain their mandatory whole-source path.
            let parts = if self.rescue[i] {
                vec![self.quads[i].clone()]
            } else {
                split(&self.quads[i])
            };
            if parts.len() < 2 {
                entries.push((o, self.quads[i].clone(), self.rescue[i], i));
                continue;
            }
            let Outcome::Parts(ledger) = &mut sources[o.source].outcome else {
                unreachable!()
            };
            ledger
                .iter_mut()
                .find(|p| Some(p.part) == o.part)
                .unwrap()
                .word_index = None;
            let first = ledger.iter().map(|p| p.part).max().unwrap() + 1;
            for (j, quad) in parts.into_iter().enumerate() {
                let part = first + j;
                ledger.push(crate::sources::Part {
                    part,
                    quad: quad.clone(),
                    word_index: None,
                });
                entries.push((
                    Observation {
                        source: o.source,
                        part: Some(part),
                    },
                    quad,
                    false,
                    i,
                ));
            }
            changed = true;
        }
        if !changed {
            return None;
        }
        let quads: Vec<_> = entries.iter().map(|e| e.1.clone()).collect();
        let layout = PageLayout::with_writing(&quads, writing);
        let order = layout.column_order();
        self.layout = layout.reindexed(&order);
        self.observations = order.iter().map(|&i| entries[i].0).collect();
        self.quads = order.iter().map(|&i| entries[i].1.clone()).collect();
        self.rescue = order.iter().map(|&i| entries[i].2).collect();
        for (i, o) in self.observations.iter().enumerate() {
            match &mut sources[o.source].outcome {
                Outcome::Parts(parts) => {
                    parts
                        .iter_mut()
                        .find(|p| Some(p.part) == o.part)
                        .unwrap()
                        .word_index = Some(i)
                }
                Outcome::Rescued { word_index, .. } => *word_index = i,
                Outcome::Empty(_) => unreachable!(),
            }
        }
        Some(order.iter().map(|&i| entries[i].3).collect())
    }

    #[cfg(test)]
    pub(crate) fn new(sources: &mut [RawSource], margin: f32) -> Self {
        Self::with_writing(sources, margin, crate::page_frame::WritingFrame::Local)
    }

    pub(crate) fn with_writing(
        sources: &mut [RawSource],
        margin: f32,
        writing: crate::page_frame::WritingFrame,
    ) -> Self {
        let plan = crate::fragments::plan_with_writing(sources, margin, writing);
        let order: Vec<_> = plan
            .layout
            .column_order()
            .into_iter()
            .filter(|&i| {
                let observation = plan.observations[i];
                match &plan.decisions[observation.source] {
                    Decision::Tiny { .. } => false,
                    Decision::Word(_) | Decision::PossibleLabel(_) => true,
                    Decision::NoScale(_) => observation.part.is_some(),
                }
            })
            .collect();
        let observations: Vec<_> = order.iter().map(|&i| plan.observations[i]).collect();
        let quads = observations
            .iter()
            .map(|o| o.quad(sources).clone())
            .collect();
        for (source, decision) in sources.iter_mut().zip(plan.decisions) {
            source.decision = Some(decision);
            if let Outcome::Parts(parts) = &mut source.outcome {
                for part in parts {
                    part.word_index = None;
                }
            }
        }
        let mut rescue = Vec::new();
        for (word_index, observation) in observations.iter().enumerate() {
            let source = &mut sources[observation.source];
            match &mut source.outcome {
                Outcome::Parts(parts) => {
                    let part = parts
                        .iter_mut()
                        .find(|p| Some(p.part) == observation.part)
                        .expect("selected part");
                    part.word_index = Some(word_index);
                    rescue.push(false);
                }
                Outcome::Empty(reason) => {
                    source.outcome = Outcome::Rescued {
                        reason: *reason,
                        word_index,
                    };
                    rescue.push(true);
                }
                Outcome::Rescued { .. } => panic!("select a fresh segmentation once"),
            }
        }
        let unavailable: Vec<_> = sources
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                s.decision
                    .as_ref()
                    .and_then(Decision::owner)
                    .filter(|o| o.word_index(sources).is_none())
                    .map(|_| i)
            })
            .collect();
        for i in unavailable {
            if let Some(Decision::Tiny { ownership, .. }) = &mut sources[i].decision {
                *ownership = Ownership::Unowned(crate::fragments::Unowned::OwnerUnavailable);
            }
        }
        Self {
            layout: plan.layout.selected(&order),
            observations,
            quads,
            rescue,
        }
    }

    #[cfg(test)]
    pub(crate) fn prepare_with<T: Send>(
        &self,
        budget: &mut Budget,
        margin: f32,
        has_ocr: bool,
        progress: &Stage<'_>,
        read: impl Fn(&Quad, bool, Option<&mut Budget>) -> Result<T, String> + Sync,
    ) -> Result<Vec<T>, String> {
        self.reserve(budget, margin, has_ocr)?;
        self.prepare_reserved_with(
            Traversal::Grouped { workers: 1 },
            budget,
            progress,
            Clone::clone,
            read,
        )
    }

    /// Required recovery gets first claim before the optional label-anchor pass.
    pub(crate) fn reserve(
        &self,
        budget: &mut Budget,
        margin: f32,
        has_ocr: bool,
    ) -> Result<(), String> {
        // Reserve all mandatory whole-word work before any optional read. A
        // failure returns the existing scan-error/photo-retake path, not a
        // successful partial phrase or an invented WordBox.
        for (quad, &rescue) in self.quads.iter().zip(&self.rescue) {
            if rescue {
                budget
                    .read_margin(quad, margin, has_ocr)
                    .map_err(|why| format!("required raw-source rescue refused: {why}"))?;
            }
        }
        Ok(())
    }

    pub(crate) fn ordinary(&self) -> impl Iterator<Item = (usize, &Quad)> {
        self.quads
            .iter()
            .enumerate()
            .filter(|(i, _)| !self.rescue[*i])
    }

    fn rescues(&self) -> impl Iterator<Item = (usize, &Quad)> {
        self.quads
            .iter()
            .enumerate()
            .filter(|(i, _)| self.rescue[*i])
    }

    /// Reads every reserved observation, returned in input order.
    ///
    /// Only a rescue draws on the shared allowance, so an ordinary read can
    /// neither observe nor consume it: ordinary reads are independent of each
    /// other and of their order, and the rescue tail alone keeps input order
    /// because its claims accumulate. Splitting the two populations is what
    /// lets the ordinary ones later run at once.
    ///
    /// A read that fails does not cancel a read at a lower index, so the
    /// failure reported is the lowest-index one however the passes are
    /// scheduled.
    pub(crate) fn prepare_reserved_with<T: Send>(
        &self,
        traversal: Traversal,
        budget: &mut Budget,
        progress: &Stage<'_>,
        display: impl Fn(&Quad) -> Quad + Sync,
        read: impl Fn(&Quad, bool, Option<&mut Budget>) -> Result<T, String> + Sync,
    ) -> Result<Vec<T>, String> {
        // On the thread that owns the pass, so it is the wall clock the reads
        // actually took rather than the sum of what each of them spent.
        let _elapsed = crate::timing::span("phrase.prepare_elapsed");
        let mut pass = Pass::new(self.quads.len());
        // A failed mask cannot justify cutting a prefix off the rescue.
        // Whole-read textual label evidence still reaches normal numbering.
        let workers = match traversal {
            #[cfg(any(test, feature = "scan-profile"))]
            Traversal::InInputOrder => {
                crate::timing::read_workers(1);
                for (k, (quad, &rescue)) in self.quads.iter().zip(&self.rescue).enumerate() {
                    pass.run(k, quad, progress, &display, || {
                        read(quad, !rescue, rescue.then_some(&mut *budget))
                    });
                }
                return pass.finish();
            }
            Traversal::Grouped { workers } => workers,
        };
        let ordinary: Vec<(usize, &Quad)> = self.ordinary().collect();
        let workers = if crate::inference_limit::scope_active() {
            1
        } else {
            effective_workers(workers, ordinary.len())
        };
        crate::timing::read_workers(workers.max(1));
        if workers > 1 {
            read_at_once(workers, &ordinary, &mut pass, progress, &display, &read);
        } else {
            for &(k, quad) in &ordinary {
                pass.run(k, quad, progress, &display, || read(quad, true, None));
            }
        }
        for (k, quad) in self.rescues() {
            pass.run(k, quad, progress, &display, || {
                read(quad, false, Some(&mut *budget))
            });
        }
        pass.finish()
    }
}

/// One traversal of the reserved reads: what each produced, how many have
/// been announced, and the earliest failure seen.
///
/// Reads may be run in any order. The slot each result lands in is its input
/// index, the announced count is a count of deliveries rather than a position,
/// and a failure only suppresses reads at higher indices, so none of the three
/// depends on the order the passes choose.
struct Pass<T> {
    out: Vec<Option<T>>,
    delivered: usize,
    failure: Option<(usize, String)>,
}

impl<T> Pass<T> {
    fn new(len: usize) -> Self {
        Self {
            out: (0..len).map(|_| None).collect(),
            delivered: 0,
            failure: None,
        }
    }

    fn superseded(&self, index: usize) -> bool {
        self.failure
            .as_ref()
            .is_some_and(|(failed, _)| *failed < index)
    }

    /// Announces a read that is about to begin, or declines it because a read
    /// at a lower index has already failed.
    fn begin(&self, index: usize, shown: &Quad, progress: &Stage<'_>) -> bool {
        if self.superseded(index) {
            return false;
        }
        progress.region(index, shown, RegionState::Reading);
        true
    }

    /// Takes one read's outcome and announces it. Both the slot and the
    /// delivered count are assigned here, so neither depends on the order the
    /// reads were started or finished in.
    fn record(
        &mut self,
        index: usize,
        shown: &Quad,
        progress: &Stage<'_>,
        result: Result<T, String>,
    ) {
        match result {
            Ok(value) => {
                self.out[index] = Some(value);
                progress.region(index, shown, RegionState::Read);
                self.delivered += 1;
                progress.read_done(self.delivered);
            }
            Err(why) => {
                if self.failure.as_ref().is_none_or(|(f, _)| index < *f) {
                    self.failure = Some((index, why));
                }
            }
        }
    }

    fn run(
        &mut self,
        index: usize,
        quad: &Quad,
        progress: &Stage<'_>,
        display: impl Fn(&Quad) -> Quad,
        read: impl FnOnce() -> Result<T, String>,
    ) {
        let shown = display(quad);
        if !self.begin(index, &shown, progress) {
            return;
        }
        let result = read();
        self.record(index, &shown, progress, result);
    }

    fn finish(self) -> Result<Vec<T>, String> {
        if let Some((_, why)) = self.failure {
            return Err(why);
        }
        Ok(self
            .out
            .into_iter()
            .map(|slot| slot.expect("every observation is either ordinary or a rescue"))
            .collect())
    }
}

/// The most threads a page may be read on however many are asked for.
///
/// This is not a policy, only a guard: neither a caller nor the phone sweep's
/// override should be able to turn a number into an exhausted thread table,
/// and past a phone's core count a sweep has its answer anyway. The policy is
/// the core count, which `available_parallelism` already reports honestly,
/// including when the system has restricted this app to fewer.
const MAX_SPAWNED: usize = 16;

/// A harness's chosen worker count, or zero for the measured default.
#[cfg(feature = "scan-profile")]
static CHOSEN_WORKERS: AtomicUsize = AtomicUsize::new(0);

/// Fixes how many threads read a page, so that one process can compare
/// schedules. Zero restores the default. Diagnostic builds only.
#[cfg(feature = "scan-profile")]
pub fn set_read_workers(workers: usize) {
    CHOSEN_WORKERS.store(workers, Ordering::Relaxed);
}

fn default_workers() -> usize {
    // A harness comparing schedules in one process, then the phone sweep's
    // variable. The app reads neither, because a diagnostic build is the only
    // one that compiles the lookups.
    #[cfg(feature = "scan-profile")]
    match CHOSEN_WORKERS.load(Ordering::Relaxed) {
        0 => {}
        chosen => return chosen.clamp(1, MAX_SPAWNED),
    }
    #[cfg(feature = "scan-profile")]
    if let Some(chosen) = std::env::var("VISION_READ_WORKERS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
    {
        return chosen.clamp(1, MAX_SPAWNED);
    }
    std::thread::available_parallelism().map_or(1, |n| n.get())
}

/// How many threads a page of this size actually warrants.
///
/// A thread with no word to read is a thread that costs a spawn and reads
/// nothing, so the request is never the number spawned.
fn effective_workers(requested: usize, reads: usize) -> usize {
    requested.clamp(1, MAX_SPAWNED).min(reads)
}

/// Runs the independent reads on `workers` threads, publishing through one
/// boundary. The count is already bounded by the work available.
///
/// Announcing a start and recording an outcome happen under the lock; claiming
/// an index and the read itself do not. So the observer sees one event at a
/// time however the reads interleave, its sequence stays gap-free and the
/// delivered count never goes backwards, while two reads are genuinely running
/// beside each other. Which word is announced when is not fixed, and is not
/// meant to be.
fn read_at_once<T: Send>(
    workers: usize,
    ordinary: &[(usize, &Quad)],
    pass: &mut Pass<T>,
    progress: &Stage<'_>,
    display: &(impl Fn(&Quad) -> Quad + Sync),
    read: &(impl Fn(&Quad, bool, Option<&mut Budget>) -> Result<T, String> + Sync),
) {
    let next = AtomicUsize::new(0);
    let shared = Mutex::new(pass);
    let samples = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                // A worker's timings belong to the scan, not to the thread that
                // happened to run them.
                let _returned = ReturnSamples(&samples);
                loop {
                    let Some(&(index, quad)) = ordinary.get(next.fetch_add(1, Ordering::Relaxed))
                    else {
                        return;
                    };
                    let shown = display(quad);
                    if !shared
                        .lock()
                        .expect("a read cannot poison the pass")
                        .begin(index, &shown, progress)
                    {
                        continue;
                    }
                    let result = read(quad, true, None);
                    shared
                        .lock()
                        .expect("a read cannot poison the pass")
                        .record(index, &shown, progress, result);
                }
            });
        }
    });
    for taken in samples.into_inner().expect("a read cannot poison the pass") {
        crate::timing::absorb(taken);
    }
}

/// Hands a worker's timings back however the worker leaves.
struct ReturnSamples<'a>(&'a Mutex<Vec<crate::timing::Drained>>);

impl Drop for ReturnSamples<'_> {
    fn drop(&mut self) {
        if let Ok(mut samples) = self.0.lock() {
            samples.push(crate::timing::drain());
        }
    }
}

pub(crate) fn unions(
    scan: &mut PageScan,
    labels: &[bool],
    margin: f32,
    has_ocr: bool,
    budget: &mut Budget,
    progress: &mut Stage<'_>,
    mut read: impl FnMut(&Quad, &mut Budget) -> Result<WordBox, String>,
) -> Result<(), String> {
    let proposals: Vec<_> = scan
        .sources
        .iter()
        .enumerate()
        .filter_map(|(source, s)| match &s.decision {
            Some(Decision::Tiny {
                ownership: Ownership::Ending { owner, union, .. },
                ..
            }) => Some((source, *owner, union.clone())),
            _ => None,
        })
        .collect();
    for (source, owner, quad) in &proposals {
        let mut outcome = Union {
            word_index: None,
            selected: false,
            reason: "owner_unavailable",
        };
        let Some(parent) = owner.word_index(&scan.sources) else {
            scan.sources[*source].union = Some(outcome);
            continue;
        };
        let original = &scan.words[parent];
        let ending_count = proposals
            .iter()
            .filter(|(_, other, _)| other == owner)
            .count();
        let blocked = if original.geometry_owned() {
            Some("cell_owned")
        } else if ending_count != 1 {
            Some("multiple_endings")
        } else if labels[parent]
            || original.number.is_some()
            || original.label.is_some()
            || original.apart
            || original.narrowed
            || original.joined_from.is_some()
            || !original.expanded_from.is_empty()
        {
            Some("protected_parent")
        } else {
            None
        };
        if let Some(reason) = blocked {
            outcome.reason = reason;
        } else if let Err(reason) = budget.read_margin(quad, margin, has_ocr) {
            outcome.reason = reason;
        } else {
            progress.region(parent, &original.quad, RegionState::Reading);
            let restore = |progress: &Stage<'_>| {
                progress.region(
                    parent,
                    &original.quad,
                    if original.stray {
                        RegionState::Excluded
                    } else {
                        RegionState::Read
                    },
                )
            };
            let mut candidate = match read(quad, budget) {
                Ok(candidate) => candidate,
                Err(error) => {
                    restore(progress);
                    return Err(error);
                }
            };
            let reason = union_reason(original, &candidate);
            if reason == "no_reading" {
                restore(progress);
                history::read_trial(
                    scan,
                    &[parent],
                    &mut candidate,
                    None,
                    "fragment_union",
                    reason,
                    union_comparisons,
                );
                scan.sources[*source].union = Some(Union {
                    word_index: None,
                    selected: false,
                    reason,
                });
                continue;
            }
            let selected = reason == "selected";
            candidate.column_rank = original.column_rank;
            candidate.row_rank = original.row_rank;
            candidate.expanded_from = vec![ExtentParent {
                word_index: parent,
                stray: original.stray,
            }];
            let index = scan.words.len();
            if !selected {
                restore(progress);
            }
            history::read_trial(
                scan,
                &[parent],
                &mut candidate,
                Some(index),
                "fragment_union",
                reason,
                union_comparisons,
            );
            if selected {
                progress.replaced(&candidate.quad, &[parent]);
                scan.words[parent].set_stray_recorded(
                    true,
                    "fragment_union",
                    "selected_replacement",
                );
            } else {
                candidate.set_stray_recorded(true, "fragment_union", reason);
                progress.alternative(&candidate.quad);
            }
            scan.words.push(candidate);
            outcome = Union {
                word_index: Some(index),
                selected,
                reason,
            };
        }
        if outcome.word_index.is_none() {
            history::record(scan, &[parent], |s| {
                serde_json::json!({"rule":"replacement_guard","owner":"fragment_union",
                "status":"evaluated","candidate_read_status":"skipped","reason":outcome.reason,"links":history::links(&[parent],None),
                "source_detector":s.sources[*source].detector,"ending_count":ending_count,"required_ending_count":1,
                "label_box":labels[parent],"parent":history::reading(&s.words[parent]),"requested_quad":quad.0,
                "budget":{"reads":budget.reads,"read_pixels":budget.read_pixels,"classifiers":budget.classifiers,"ocrs":budget.ocrs,
                    "max_reads":crate::read_budget::MAX_READS,"max_read_pixels":crate::read_budget::MAX_READ_PIXELS}})
            });
        }
        scan.sources[*source].union = Some(outcome);
    }
    Ok(())
}

fn union_comparisons() -> serde_json::Value {
    serde_json::json!({"ordered_guards":["no_reading","uncertain","pick_disagrees","less_confident","accepted_spelling_changed"],
        "uncertain":"candidate must be confirmed and kept",
        "pick_disagrees":"candidate pick == candidate classifier top",
        "less_confident":"candidate top probability >= parent top probability (missing parent top = 0)",
        "accepted_spelling_changed":"an accepted parent must retain its selected identity",
        "success":"selected"})
}

/// Lower-ink ownership is a zero-reread decision. Later repair owners must not
/// turn it into a replacement of the parent's original crop and reading.
pub(crate) fn owns_lower_fragment(scan: &PageScan, word_index: usize) -> bool {
    scan.sources.iter().any(|s| {
        matches!(&s.decision,
        Some(Decision::Tiny { ownership: Ownership::Descender(o) | Ownership::Duplicate(o), .. })
            if o.word_index(&scan.sources) == Some(word_index))
    })
}

fn union_reason(parent: &WordBox, candidate: &WordBox) -> &'static str {
    let Some(&(top, probability)) = candidate.ranked.first() else {
        return "no_reading";
    };
    if candidate.verdict() != Verdict::Accept || candidate.stray {
        return "uncertain";
    }
    if candidate.pick() != Some(top) {
        return "pick_disagrees";
    }
    if probability < parent.ranked.first().map_or(0.0, |p| p.1) {
        return "less_confident";
    }
    if parent.verdict() == Verdict::Accept && parent.pick() != candidate.pick() {
        return "accepted_spelling_changed";
    }
    "selected"
}

#[cfg(test)]
mod tests;
