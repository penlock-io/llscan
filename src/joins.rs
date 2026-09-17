//! A bounded union of two uncertain pieces, supported by the stage's rows.
//! No alternative layout, label inference, word-count target or transitive join.

use crate::detect::Quad;
use crate::layout::PageLayout;
#[cfg(test)]
use crate::numbering::Numbering;
use crate::page_frame::WritingFrame;
use crate::phrase::history;
use crate::phrase::{PageScan, WordBox};
use crate::progress::{RegionState, Stage};
use crate::words::Verdict;

/// Maximum additional literal crop reads, before any model/OCR call.
pub const MAX_READS: usize = 4;
/// Total additional grayscale crop pixels retained or tried on one page.
pub const MAX_CROP_PIXELS: u64 = 1_048_576;

/// Numeric evidence for a union read, using the existing calibration.
#[derive(Clone, Debug)]
pub struct JoinRead {
    /// Model top word index.
    pub top: usize,
    /// Model top probability.
    pub probability: f32,
    /// Top-minus-runner-up margin.
    pub margin: f32,
    /// The hybrid pick's word index.
    pub pick: Option<usize>,
    /// Whether the ordinary finish/keep rule retained the read.
    pub kept: bool,
}

/// One two-observation row and the reason it was accepted or refused.
#[derive(Clone, Debug)]
pub struct JoinTrial {
    /// Stage column group (before unions).
    pub column: usize,
    /// Stage row group (before unions).
    pub row: usize,
    /// The original pieces' indices in the returned page's words.
    pub parents: [usize; 2],
    /// Stable diagnostic reason; only `joined` selects a union.
    pub reason: &'static str,
    /// Present only if the pre-read guards and budget allowed a read.
    pub read: Option<JoinRead>,
}

#[cfg(test)]
fn candidates(layout: &PageLayout) -> Vec<JoinTrial> {
    candidates_observed(layout, |_, _| {})
}

pub(crate) fn candidates_observed(
    layout: &PageLayout,
    mut record: impl FnMut(&JoinTrial, &[(usize, Vec<usize>)]),
) -> Vec<JoinTrial> {
    let mut trials = Vec::new();
    for (column, members) in layout.columns.iter().enumerate() {
        let rows: Vec<_> = layout
            .rows
            .iter()
            .enumerate()
            .filter_map(|(row, indices)| {
                let in_column: Vec<_> = indices
                    .iter()
                    .copied()
                    .filter(|i| members.contains(i))
                    .collect();
                (!in_column.is_empty()).then_some((row, in_column))
            })
            .collect();
        for (row, pair) in rows.iter().filter(|(_, r)| r.len() == 2) {
            let supported =
                rows.len() >= 3 && rows.iter().all(|(other, r)| other == row || r.len() == 1);
            let trial = JoinTrial {
                column,
                row: *row,
                parents: [pair[0], pair[1]],
                reason: if supported {
                    "candidate"
                } else {
                    "row_structure"
                },
                read: None,
            };
            record(&trial, &rows);
            trials.push(trial);
        }
    }
    trials
}

// Enclose both original quads in photo coordinates. No padding or ink is
// removed here; the existing reader applies its normal margin and leveling.
pub(crate) fn union(a: &Quad, b: &Quad) -> Quad {
    let xs = a.0.iter().chain(&b.0).map(|p| p.0);
    let ys = a.0.iter().chain(&b.0).map(|p| p.1);
    let (x0, x1) = (
        xs.clone().fold(f32::MAX, f32::min),
        xs.fold(f32::MIN, f32::max),
    );
    let (y0, y1) = (
        ys.clone().fold(f32::MAX, f32::min),
        ys.fold(f32::MIN, f32::max),
    );
    Quad([(x0, y0), (x1, y0), (x1, y1), (x0, y1)])
}

pub(crate) fn union_in(a: &Quad, b: &Quad, writing: WritingFrame) -> Quad {
    writing.unproject(&union(&writing.project(a), &writing.project(b)))
}

fn read_reason(a: &WordBox, b: &WordBox, joined: &WordBox) -> &'static str {
    if a.verdict() == Verdict::Accept || b.verdict() == Verdict::Accept {
        return "parent_accepted";
    }
    if joined.verdict() != Verdict::Accept {
        return "union_uncertain";
    }
    let Some(&(top, probability)) = joined.ranked.first() else {
        return "union_uncertain";
    };
    if joined.pick() != Some(top) {
        return "pick_disagrees";
    }
    if joined.stray {
        return "union_dropped";
    }
    if [a, b]
        .iter()
        .any(|w| w.ranked.first().is_none_or(|r| r.1 > probability))
    {
        return "parent_surer";
    }
    "joined"
}

/// Applies the frozen, bounded rule using this scan's original layout and reads.
/// Original pieces remain intact as inactive alternatives, with explicit links.
pub(crate) fn apply(
    layout: &PageLayout,
    labels: &[bool],
    scan: &mut PageScan,
    margin: f32,
    crop_reader: &dyn Fn(&Quad) -> Result<WordBox, String>,
    progress: &mut Stage<'_>,
) -> Result<(), String> {
    let _scope = crate::timing::span("phrase.joins");
    let mut trials = candidates_observed(layout, |trial, rows| {
        history::record(scan, &trial.parents, |_| {
            serde_json::json!({"rule":"join_structure","status":"evaluated",
            "links":history::links(&trial.parents,None),"reason":trial.reason,
            "column":trial.column,"row":trial.row,"column_row_sizes":rows.iter().map(|(r,p)| (*r,p.len())).collect::<Vec<_>>(),
            "minimum_rows":3,"comparison":"at least 3 rows; this row has 2 observations and every other row has 1"})
        });
    });
    let (mut reads, mut pixels) = (0, 0);
    for trial in &mut trials {
        if trial.reason != "candidate" {
            continue;
        }
        let [a, b] = trial.parents;
        let (left, right) = (&scan.words[a], &scan.words[b]);
        if crate::repair::owns_lower_fragment(scan, a)
            || crate::repair::owns_lower_fragment(scan, b)
        {
            trial.reason = "owned_lower_ink";
            record_guard(scan, trial);
            continue;
        }
        // This narrow repair does not reinterpret numbers, labels, trims or
        // the region rule. Those readings remain observations for correction.
        trial.reason = if left.geometry_owned() || right.geometry_owned() {
            "cell_owned"
        } else if !scan.numbering.permits_word_repair() {
            "numbering"
        } else if labels[a] || labels[b] {
            "label"
        } else if left.apart || right.apart {
            "apart"
        } else if left.narrowed || right.narrowed {
            "trimmed"
        } else if scan.words.iter().any(|w| {
            !w.expanded_from.is_empty()
                && (w
                    .expanded_from
                    .iter()
                    .any(|p| p.word_index == a || p.word_index == b))
        }) || !left.expanded_from.is_empty()
            || !right.expanded_from.is_empty()
        {
            "replacement_parent"
        } else if left.verdict() == Verdict::Accept || right.verdict() == Verdict::Accept {
            "parent_accepted"
        } else {
            "candidate"
        };
        if trial.reason != "candidate" {
            record_guard(scan, trial);
            continue;
        }
        let quad = union_in(&left.quad, &right.quad, layout.writing);
        let f = layout.writing.crop_frame(&quad, margin);
        let cost = crate::read_budget::pixels_ceil(f).unwrap_or(u64::MAX);
        if reads == MAX_READS || cost > MAX_CROP_PIXELS - pixels {
            trial.reason = "budget";
            history::record(scan, &trial.parents, |_| {
                serde_json::json!({"rule":"replacement_guard","owner":"row_join",
                "status":"evaluated","candidate_read_status":"skipped","reason":"budget","links":history::links(&trial.parents,None),
                "reads":reads,"max_reads":MAX_READS,"requested_pixels":cost,"used_pixels":pixels,"max_pixels":MAX_CROP_PIXELS,
                "comparison":"reads < max_reads and requested_pixels <= max_pixels - used_pixels"})
            });
            continue;
        }
        reads += 1;
        pixels += cost;
        progress.region(a, &left.quad, RegionState::Reading);
        progress.region(b, &right.quad, RegionState::Reading);
        let _read = crate::timing::span("phrase.join_read");
        let mut joined = crop_reader(&quad)?;
        trial.reason = read_reason(left, right, &joined);
        let (top, probability) = joined.ranked.first().copied().unwrap_or((0, 0.0));
        trial.read = Some(JoinRead {
            top,
            probability,
            margin: probability - joined.ranked.get(1).map_or(0.0, |r| r.1),
            pick: joined.pick(),
            kept: !joined.stray,
        });
        let selected = trial.reason == "joined";
        if selected {
            // Anchor the union in the original traversals; do not let its
            // bounding rectangle regroup or reorder unrelated observations.
            joined.column_rank = left.column_rank.min(right.column_rank);
            joined.row_rank = left.row_rank.min(right.row_rank);
            joined.joined_from = Some([a, b]);
        }
        let candidate_index = selected.then_some(scan.words.len());
        history::read_trial(
            scan,
            &[a, b],
            &mut joined,
            candidate_index,
            "row_join",
            trial.reason,
            || {
                serde_json::json!({
            "ordered_guards":["parent_accepted","union_uncertain","pick_disagrees","union_dropped","parent_surer"],
            "parent_accepted":"both parents must be uncertain","union_uncertain":"candidate must be confirmed and have a classifier top",
            "pick_disagrees":"candidate pick == classifier top","union_dropped":"candidate must be kept",
            "parent_surer":"every parent must have a top probability <= candidate top probability","success":"joined"})
            },
        );
        if selected {
            scan.words[a].set_stray_recorded(true, "row_join", "selected_replacement");
            scan.words[b].set_stray_recorded(true, "row_join", "selected_replacement");
            progress.joined(&joined.quad, [a, b]);
            scan.words.push(joined);
        } else {
            for i in [a, b] {
                let word = &scan.words[i];
                progress.region(
                    i,
                    &word.quad,
                    if word.stray {
                        RegionState::Excluded
                    } else {
                        RegionState::Read
                    },
                );
            }
        }
    }
    for i in 0..labels.len() {
        history::record(scan, &[i], |_| {
            serde_json::json!({"rule":"replacement_stage","owner":"row_join","status":"evaluated",
            "links":history::links(&[i],None),"trial_count":trials.iter().filter(|t| t.parents.contains(&i)).count()})
        });
    }
    if scan.words.iter().any(|w| w.joined_from.is_some()) {
        let inverse = scan.reindex_words(progress);
        for trial in &mut trials {
            trial.parents = trial.parents.map(|p| inverse[p]);
        }
    }
    scan.join_trials = trials;
    Ok(())
}

fn record_guard(scan: &mut PageScan, trial: &JoinTrial) {
    history::record(scan, &trial.parents, |s| {
        serde_json::json!({"rule":"replacement_guard","owner":"row_join",
        "status":"evaluated","candidate_read_status":"skipped","reason":trial.reason,"links":history::links(&trial.parents,None),
        "parents":trial.parents.iter().map(|&p| history::reading(&s.words[p])).collect::<Vec<_>>(),
        "numbering_permits_repair":s.numbering.permits_word_repair(),
        "evaluation":"pre-read guard refused; no candidate reader call"})
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::progress::{Event, Observer};
    use crate::sources::{Outcome, Part, RawSource};
    use image::GrayImage;
    use std::sync::Mutex;

    #[test]
    fn observed_joins_use_the_same_result_permutation_and_rejected_trials_restore_state() {
        struct Recorder(Mutex<Vec<Event>>);
        impl Observer for Recorder {
            fn on_event(&self, event: Event) {
                self.0.lock().unwrap().push(event);
            }
        }
        for joined_probability in [0.999, 0.55] {
            let quads = vec![
                quad(0., 0., 100.),
                quad(0., 30., 35.),
                quad(40., 30., 60.),
                quad(0., 60., 100.),
            ];
            let mut scan = PageScan {
                page_direction: None,
                width: 120,
                height: 100,
                sources: quads
                    .iter()
                    .enumerate()
                    .map(|(i, q)| RawSource {
                        decision: None,
                        union: None,
                        detector: i,
                        quad: q.clone(),
                        outcome: Outcome::Parts(vec![Part {
                            part: 0,
                            quad: q.clone(),
                            word_index: Some(i),
                        }]),
                    })
                    .collect(),
                numbering: Numbering::None,
                join_trials: vec![],
                words: quads
                    .iter()
                    .enumerate()
                    .map(|(i, q)| WordBox {
                        quad: q.clone(),
                        column_rank: i,
                        row_rank: i,
                        ..word(0.65)
                    })
                    .collect(),
            };
            let recorder = Recorder(Mutex::new(Vec::new()));
            // Two other words carry the same observed label, below MIN_RUN.
            // Actual aggregation/fitting must leave the eligible middle join
            // available, while keeping both conflict diagnostics and list mode.
            let candidates: Vec<_> = quads
                .iter()
                .enumerate()
                .map(|(i, q)| crate::phrase::Candidate {
                    quad: q,
                    read: Some(if i == 0 || i == 3 { "1.word" } else { "word" }),
                    token: None,
                    word_like: true,
                })
                .collect();
            let labelled = crate::phrase::label_evidence(&candidates);
            scan.numbering = labelled.numbering(&[0, 1, 2, 3], &[0, 1, 2, 3]);
            for (i, w) in scan.words.iter_mut().enumerate() {
                w.evidence = labelled.observations[i].clone();
                w.label = labelled.label_read[i].clone();
            }
            assert!(scan.words[0].evidence.conflict && scan.words[3].evidence.conflict);
            assert_eq!(
                scan.list_mode(),
                crate::numbering::dotted::ListMode::Numbered
            );
            assert!(scan.numbering.permits_word_repair());
            let mut progress = Stage::new(Some(&recorder), quads.len());
            let calls = std::cell::Cell::new(0);
            apply(&PageLayout::new(&quads), &[false; 4], &mut scan, 0.15, &|q| {
                calls.set(calls.get() + 1);
                // Reading updates are already visible before the reader runs.
                let events = recorder.0.lock().unwrap();
                assert!(matches!(events.last(), Some(Event::Region(r)) if r.id == 2 && r.state == RegionState::Reading));
                Ok(WordBox { quad: q.clone(), ..word(joined_probability) })
            }, &mut progress).unwrap();
            assert_eq!(calls.get(), 1);
            for source in &scan.sources {
                let part = &source.parts()[0];
                assert_eq!(part.part, 0);
                assert_eq!(source.quad, quads[source.detector]);
                assert_eq!(part.quad, source.quad);
                assert_eq!(scan.words[part.word_index.unwrap()].quad, part.quad);
                assert!(scan.words[part.word_index.unwrap()].joined_from.is_none());
            }
            let mappings = progress.into_regions();
            let events = recorder.0.lock().unwrap();
            if joined_probability > 0.9 {
                assert_eq!(
                    mappings.iter().map(|r| r.word_index).collect::<Vec<_>>(),
                    vec![0, 1, 3, 4, 2]
                );
                assert_eq!(scan.words[2].joined_from, Some([1, 3]));
                assert!(
                    matches!(events.last(), Some(Event::Replaced { region, parents })
                    if region.id == 4 && region.quad == scan.words[2].quad && parents == &[1, 2])
                );
            } else {
                assert_eq!(mappings.len(), 4);
                assert!(events.iter().all(|e| !matches!(e, Event::Replaced { .. })));
                assert!(
                    matches!(events.last(), Some(Event::Region(r)) if r.id == 2 && r.state == RegionState::Read)
                );
            }
            for region in &mappings {
                let word = &scan.words[region.word_index as usize];
                if region.id < 4 {
                    assert_eq!(word.quad, quads[region.id as usize]);
                }
            }
        }
    }

    fn quad(x: f32, y: f32, width: f32) -> Quad {
        Quad([(x, y), (x + width, y), (x + width, y + 10.), (x, y + 10.)])
    }

    #[test]
    fn one_double_row_needs_two_other_single_rows_without_filtering_observations() {
        let quads = vec![
            quad(0., 0., 100.),
            quad(0., 30., 35.),
            quad(40., 30., 60.),
            quad(0., 60., 100.),
        ];
        let trials = candidates(&PageLayout::new(&quads));
        assert_eq!(trials.len(), 1);
        assert_eq!(trials[0].parents, [1, 2]);
        assert_eq!(trials[0].reason, "candidate");
        let isolated = candidates(&PageLayout::new(&quads[..3]));
        assert!(isolated.iter().all(|t| t.reason == "row_structure"));
        let mut extra = quads.clone();
        extra.push(quad(70., 60., 20.));
        let multiple = candidates(&PageLayout::new(&extra));
        assert_eq!(multiple.len(), 2);
        assert!(multiple.iter().all(|t| t.reason == "row_structure"));
        // A missing row is not manufactured from a failed word read; a
        // genuine extra observation is still visible to the geometry rule.
        let fourteen: Vec<_> = (0..14).map(|i| quad(0., i as f32 * 30., 100.)).collect();
        assert!(candidates(&PageLayout::new(&fourteen)).is_empty());
    }

    #[test]
    fn overlapping_and_two_small_pieces_work_but_distinct_columns_do_not_join() {
        for (x, width) in [(25., 55.), (60., 20.)] {
            let q = [
                quad(0., 0., 100.),
                quad(0., 30., 40.),
                quad(x, 30., width),
                quad(0., 60., 100.),
            ];
            assert_eq!(candidates(&PageLayout::new(&q))[0].reason, "candidate");
            let merged = union(&q[1], &q[2]);
            for p in q[1].0.iter().chain(&q[2].0) {
                assert!(p.0 >= merged.0[0].0 && p.0 <= merged.0[2].0);
                assert!(p.1 >= merged.0[0].1 && p.1 <= merged.0[2].1);
            }
        }
        let columns: Vec<_> = [0., 200.]
            .iter()
            .flat_map(|&x| (0..3).map(move |y| quad(x, y as f32 * 30., 100.)))
            .collect();
        assert!(candidates(&PageLayout::new(&columns)).is_empty());
    }

    fn word(probability: f32) -> WordBox {
        let ranked = vec![(0, probability), (1, 1.0 - probability)];
        WordBox {
            quad: quad(0., 0., 100.),
            crop: GrayImage::new(1, 1),
            selection: Some(crate::hybrid::select(None, None, &ranked, 0.85, 0.75).unwrap()),
            ranked,
            column_rank: 0,
            row_rank: 0,
            turned: false,
            raw: None,
            evidence: Default::default(),
            narrowed: false,
            stray: false,
            number: None,
            label: None,
            apart: false,
            joined_from: None,
            expanded_from: Vec::new(),
        }
    }

    #[test]
    fn occupied_cell_is_not_a_generic_join_candidate_even_when_uncertain() {
        let quads = vec![
            quad(0., 0., 100.),
            quad(0., 30., 35.),
            quad(40., 30., 60.),
            quad(0., 60., 100.),
        ];
        let mut scan = PageScan {
            page_direction: None,
            width: 120,
            height: 100,
            sources: vec![],
            numbering: Numbering::None,
            join_trials: vec![],
            words: quads
                .iter()
                .map(|q| WordBox {
                    quad: q.clone(),
                    ..word(0.6)
                })
                .collect(),
        };
        scan.words[1].evidence.cell = Some(crate::phrase::CellSupport {
            column: Some(0),
            row: Some(1),
            ordinal: None,
            quad: quads[1].0,
            ink_pixels: 100,
            basis: "label_track_cell_and_word_ink",
        });
        apply(
            &PageLayout::new(&quads),
            &[false; 4],
            &mut scan,
            0.,
            &|_| panic!("cell content must not be reread as a generic union"),
            &mut Stage::new(None, 4),
        )
        .unwrap();
        assert_eq!(scan.join_trials[0].reason, "cell_owned");
        assert_eq!(scan.words.len(), 4);
        assert!(scan.words.iter().all(|w| !w.stray));
    }

    #[test]
    fn acceptance_is_not_keep_or_any_list_word_and_confident_pieces_are_protected() {
        let (a, b, mut joined) = (word(0.766), word(0.642), word(0.999));
        assert_eq!(read_reason(&a, &b, &joined), "joined");
        assert_eq!(read_reason(&a, &word(0.95), &joined), "parent_accepted");
        assert_eq!(read_reason(&a, &b, &word(0.594)), "union_uncertain");
        assert_eq!(read_reason(&a, &b, &word(0.86)), "union_uncertain"); // margin
        joined.select_fixture(1, false);
        assert_eq!(read_reason(&a, &b, &joined), "union_uncertain"); // disagreement cannot inherit top's accept
        joined.select_fixture(0, true);
        joined.stray = true;
        assert_eq!(read_reason(&a, &b, &joined), "union_dropped");
        let mut sure_but_uncalibrated = word(0.9999);
        sure_but_uncalibrated.select_fixture(0, false);
        assert_eq!(
            read_reason(&sure_but_uncalibrated, &b, &word(0.999)),
            "parent_surer"
        );
    }

    #[test]
    fn selected_disagreement_no_longer_borrows_parent_protection() {
        let mut parent = word(0.99);
        parent.selection =
            Some(crate::hybrid::select(Some("ability"), None, &parent.ranked, 0.85, 0.75).unwrap());
        assert_eq!(parent.pick(), Some(1));
        assert_eq!(parent.verdict(), Verdict::Uncertain);
        assert_eq!(read_reason(&parent, &word(0.6), &word(0.999)), "joined");
        assert_eq!(
            read_reason(&word(0.99), &word(0.6), &word(0.999)),
            "parent_accepted"
        );
    }
}
