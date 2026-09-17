use super::*;
use crate::numbering::Numbering;
use crate::progress::{Event, Observer, Phase};
use crate::recogniser::LineRead;
use crate::sources::Part;
use crate::split::EmptySplit;
use image::GrayImage;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

fn q(x: f32, y: f32, w: f32, h: f32) -> Quad {
    Quad([(x, y), (x + w, y), (x + w, y + h), (x, y + h)])
}

fn source(detector: usize, quad: Quad) -> RawSource {
    RawSource {
        detector,
        quad: quad.clone(),
        outcome: Outcome::Parts(vec![Part {
            part: 0,
            quad,
            word_index: Some(detector),
        }]),
        decision: None,
        union: None,
    }
}

fn scene(fragment: Quad) -> Vec<RawSource> {
    [
        q(100., 100., 300., 100.),
        q(100., 300., 450., 100.),
        q(100., 500., 400., 100.),
        q(100., 700., 400., 100.),
        fragment,
    ]
    .into_iter()
    .enumerate()
    .map(|(i, quad)| source(i, quad))
    .collect()
}

fn empty(sources: &mut Vec<RawSource>, y: f32) {
    let mut raw = source(sources.len(), q(100., y, 400., 100.));
    raw.outcome = Outcome::Empty(EmptySplit::FlatPhotoSamples);
    sources.push(raw);
}

#[test]
fn cell_coalescence_preserves_both_raw_sources_and_one_read_owner() {
    let mut sources = vec![
        source(0, q(100., 100., 180., 60.)),
        source(1, q(310., 100., 30., 60.)),
        source(2, q(100., 300., 200., 60.)),
    ];
    let mut input = Input::new(&mut sources, 0.15);
    let first = input
        .observations
        .iter()
        .position(|o| o.source == 0)
        .unwrap();
    let second = input
        .observations
        .iter()
        .position(|o| o.source == 1)
        .unwrap();
    let union = q(100., 100., 240., 60.);
    let parents = input
        .coalesce_cells(
            &mut sources,
            crate::page_frame::WritingFrame::Local,
            &[(vec![first, second], union.clone())],
        )
        .unwrap();
    assert_eq!(input.quads.len(), 2);
    let merged = parents.iter().position(|p| p.len() == 2).unwrap();
    for raw in &sources[..2] {
        assert_eq!(raw.parts()[0].word_index, None);
        assert_eq!(raw.parts()[1].word_index, Some(merged));
        assert_eq!(raw.parts()[1].quad, union);
    }
    assert_eq!(input.quads[merged], union);
    assert_eq!(input.observations[merged].quad(&sources), &union);
}

#[test]
fn column_partition_preserves_original_geometry_and_links_each_child() {
    let original = q(100., 100., 600., 100.);
    let mut sources = vec![
        source(0, original.clone()),
        source(1, q(100., 300., 200., 100.)),
    ];
    let mut input = Input::new(&mut sources, 0.15);
    let children = vec![q(110., 100., 200., 100.), q(500., 100., 180., 100.)];
    let parents = input
        .partition_columns(
            &mut sources,
            crate::page_frame::WritingFrame::Local,
            |quad| {
                if *quad == original {
                    children.clone()
                } else {
                    vec![quad.clone()]
                }
            },
        )
        .unwrap();
    assert_eq!(input.quads.len(), 3);
    assert_eq!(parents.iter().filter(|&&p| p == 0).count(), 2);
    assert_eq!(sources[0].quad, original);
    assert_eq!(sources[0].parts()[0].quad, original);
    assert_eq!(sources[0].parts()[0].word_index, None);
    for part in &sources[0].parts()[1..] {
        let i = part.word_index.unwrap();
        assert_eq!(input.quads[i], part.quad);
        assert_eq!(input.observations[i].part, Some(part.part));
    }
    assert_eq!(input.layout.column_order(), vec![0, 1, 2]);
}

fn word(quad: Quad, rank: usize) -> WordBox {
    WordBox {
        quad,
        crop: GrayImage::new(2, 3),
        ranked: vec![(0, 0.97), (1, 0.03)],
        selection: Some(
            crate::hybrid::select(Some("abandon"), None, &[(0, 0.97), (1, 0.03)], 0.85, 0.75)
                .unwrap(),
        ),
        column_rank: rank,
        row_rank: rank,
        turned: false,
        raw: Some(LineRead {
            text: "abandon".into(),
            confidence: 0.9,
        }),
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
fn selected_disagreement_and_agreement_drive_union_identity_protection() {
    let mut parent = word(q(0., 0., 10., 10.), 0);
    let mut candidate = parent.clone();
    candidate.ranked = vec![(1, 0.99), (0, 0.01)];
    candidate.selection =
        Some(crate::hybrid::select(Some("ability"), None, &candidate.ranked, 0.85, 0.75).unwrap());
    assert_eq!(
        union_reason(&parent, &candidate),
        "accepted_spelling_changed"
    );
    parent.selection =
        Some(crate::hybrid::select(Some("ability"), None, &parent.ranked, 0.85, 0.75).unwrap());
    assert_eq!(parent.verdict(), Verdict::Uncertain);
    assert_eq!(union_reason(&parent, &candidate), "selected");
}

fn scan(input: &Input, sources: Vec<RawSource>) -> PageScan {
    PageScan {
        page_direction: None,
        width: 1000,
        height: 2000,
        sources,
        words: input
            .quads
            .iter()
            .enumerate()
            .map(|(k, q)| word(q.clone(), k))
            .collect(),
        numbering: Numbering::None,
        join_trials: Vec::new(),
    }
}

#[derive(Default)]
struct Recorder(Mutex<Vec<Event>>);
impl Observer for Recorder {
    fn on_event(&self, event: Event) {
        self.0.lock().unwrap().push(event);
    }
}

#[test]
fn tiny_ink_has_no_reader_call_or_progress_id_and_whole_empty_is_read_once() {
    let mut sources = scene(q(300., 190., 40., 40.));
    empty(&mut sources, 900.);
    let raw = sources[5].quad.clone();
    let input = Input::new(&mut sources, 0.15);
    assert_eq!(input.quads.len(), 5);
    assert_eq!(input.layout.column_order(), vec![0, 1, 2, 3, 4]);
    assert_eq!(sources[4].parts()[0].word_index, None);
    assert_eq!(
        sources[4].decision.as_ref().unwrap().reason(),
        "owned_descender"
    );
    assert_eq!(
        sources[5].outcome,
        Outcome::Rescued {
            reason: EmptySplit::FlatPhotoSamples,
            word_index: 4,
        }
    );
    let observer = Recorder::default();
    let stage = Stage::new(Some(&observer), input.quads.len());
    let mut budget = Budget::default();
    let calls = Mutex::new(Vec::new());
    input
        .prepare_with(&mut budget, 0.15, true, &stage, |quad, labels, extra| {
            calls
                .lock()
                .unwrap()
                .push((quad.clone(), labels, extra.is_some()));
            if let Some(b) = extra {
                b.classifier()?;
                b.ocr()?;
            }
            Ok(())
        })
        .unwrap();
    let calls = calls.into_inner().unwrap();
    assert_eq!(calls.len(), 5);
    assert!(
        calls[..4]
            .iter()
            .all(|(_, labels, extra)| *labels && !extra)
    );
    assert_eq!(calls[4], (raw, false, true));
    assert!(!calls.iter().any(|(quad, _, _)| *quad == sources[4].quad));
    let events = observer.0.lock().unwrap();
    assert!(events.contains(&Event::Work {
        phase: Phase::Reading,
        completed: 5,
        total: Some(5)
    }));
    assert_eq!(stage.into_regions().len(), 5);
}

#[test]
fn required_rescues_that_cannot_fit_fail_before_any_reader_call() {
    let mut sources = scene(q(300., 190., 40., 40.));
    for i in 0..5 {
        empty(&mut sources, 900. + i as f32 * 200.);
    }
    let input = Input::new(&mut sources, 0.15);
    assert_eq!(input.rescue.iter().filter(|&&r| r).count(), 5);
    let result = input.prepare_with::<()>(
        &mut Budget::default(),
        0.15,
        true,
        &Stage::new(None, input.quads.len()),
        |_, _, _| panic!("preflight must fail"),
    );
    assert_eq!(
        result.unwrap_err(),
        "required raw-source rescue refused: read_budget"
    );
}

#[test]
fn missing_scale_keeps_parts_and_accounts_for_unread_empty_sources() {
    let mut sources = vec![source(0, q(100., 100., 400., 100.))];
    empty(&mut sources, 300.);
    let input = Input::new(&mut sources, 0.15);
    assert_eq!(input.quads.len(), 1);
    assert!(!input.rescue[0]);
    assert_eq!(
        sources[1].outcome,
        Outcome::Empty(EmptySplit::FlatPhotoSamples)
    );
    assert_eq!(
        sources[1].decision.as_ref().unwrap().reason(),
        "too_few_supports"
    );
}

#[test]
fn accepted_and_inactive_unions_keep_their_actual_reading_and_reindexed_links() {
    for selected in [true, false] {
        let mut outcomes = Vec::new();
        for trace in [false, true] {
            let mut sources = scene(q(423., 120., 40., 30.));
            let input = Input::new(&mut sources, 0.15);
            let mut scan = scan(&input, sources);
            if trace {
                for word in &mut scan.words {
                    history::enable_fixture(word);
                }
            }
            let parent = scan.words[0].clone();
            let observer = Recorder::default();
            let mut progress = Stage::new(Some(&observer), scan.words.len());
            let mut calls = 0;
            unions(
                &mut scan,
                &[false; 4],
                0.15,
                true,
                &mut Budget::default(),
                &mut progress,
                |quad, b| {
                    calls += 1;
                    b.classifier()?;
                    b.ocr()?;
                    let mut candidate = word(quad.clone(), 999);
                    if trace {
                        history::enable_fixture(&mut candidate);
                    }
                    candidate.crop = GrayImage::new(7, 3);
                    candidate.ranked = vec![
                        (if selected { 0 } else { 1 }, 0.99),
                        (if selected { 1 } else { 0 }, 0.01),
                    ];
                    candidate.select_fixture(candidate.ranked[0].0, true);
                    candidate.raw.as_mut().unwrap().text = "union read".into();
                    Ok(candidate)
                },
            )
            .unwrap();
            assert_eq!(calls, 1);
            scan.reindex_words(&mut progress);
            let result = scan.sources[4].union.as_ref().unwrap();
            assert_eq!(result.selected, selected);
            assert_eq!(
                result.reason,
                if selected {
                    "selected"
                } else {
                    "accepted_spelling_changed"
                }
            );
            let index = result.word_index.unwrap();
            assert_eq!(index, 1);
            let alternative = &scan.words[index];
            assert_eq!(alternative.crop.dimensions(), (7, 3));
            assert_eq!(alternative.raw.as_ref().unwrap().text, "union read");
            assert_eq!(alternative.stray, !selected);
            let parent_index = alternative.expanded_from[0].word_index;
            assert_eq!(scan.sources[0].parts()[0].word_index, Some(parent_index));
            assert_eq!(scan.words[parent_index].quad, parent.quad);
            assert_eq!(scan.words[parent_index].crop, parent.crop);
            assert_eq!(
                scan.words[parent_index].raw.as_ref().unwrap().text,
                "abandon"
            );
            assert_eq!(scan.words[parent_index].stray, selected);
            assert_eq!(scan.sources[4].parts()[0].word_index, None);
            assert_eq!(
                scan.sources[4]
                    .decision
                    .as_ref()
                    .unwrap()
                    .owner()
                    .unwrap()
                    .word_index(&scan.sources),
                Some(parent_index)
            );
            let events = observer.0.lock().unwrap();
            assert_eq!(
                events.iter().any(|e| matches!(e, Event::Replaced { .. })),
                selected
            );
            if !selected {
                assert!(events.iter().any(|e| matches!(e,
            Event::Region(r) if r.id == 4 && r.state == RegionState::Excluded)));
            }
            assert_eq!(
                progress
                    .into_regions()
                    .iter()
                    .find(|r| r.id == 4)
                    .unwrap()
                    .word_index,
                index as u32
            );
            if trace {
                let value = scan.words[index]
                    .evidence
                    .decisions
                    .as_ref()
                    .unwrap()
                    .to_json();
                let e = value["events"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|e| e["rule"] == "replacement_read")
                    .unwrap();
                assert_eq!(e["owner"], "fragment_union");
                assert_eq!(e["reason"], result.reason);
                assert_eq!(e["links"][0]["word_index"], parent_index);
                assert_eq!(e["links"][1]["word_index"], index);
                assert_eq!(e["candidate_retained"], true); // refused unions are saved inactive alternatives
            }
            outcomes.push(history::without_history(&scan));
        }
        assert_eq!(outcomes[0], outcomes[1]);
    }
}

#[test]
fn rescues_union_and_later_extent_share_the_same_four_reads() {
    let mut sources = scene(q(423., 120., 40., 30.));
    for i in 0..3 {
        empty(&mut sources, 900. + i as f32 * 200.);
    }
    let input = Input::new(&mut sources, 0.15);
    let mut budget = Budget::default();
    let mut progress = Stage::new(None, input.quads.len());
    let extra_calls = AtomicUsize::new(0);
    input
        .prepare_with(&mut budget, 0.15, true, &progress, |_, _, extra| {
            if let Some(b) = extra {
                extra_calls.fetch_add(1, Ordering::Relaxed);
                b.classifier()?;
                b.ocr()?;
            }
            Ok(())
        })
        .unwrap();
    assert_eq!(extra_calls.load(Ordering::Relaxed), 3);
    let mut scan = scan(&input, sources);
    unions(
        &mut scan,
        &[false; 7],
        0.15,
        true,
        &mut budget,
        &mut progress,
        |quad, b| {
            extra_calls.fetch_add(1, Ordering::Relaxed);
            b.classifier()?;
            b.ocr()?;
            Ok(word(quad.clone(), 0))
        },
    )
    .unwrap();
    assert_eq!(extra_calls.load(Ordering::Relaxed), 4);
    assert_eq!(budget.read(&q(10., 10., 40., 10.)), Err("read_budget"));
    assert!(budget.ocr().is_err());
    scan.reindex_words(&mut progress);
    for source in &scan.sources[5..] {
        let Outcome::Rescued { word_index, .. } = source.outcome else {
            panic!("not rescued")
        };
        assert_eq!(scan.words[word_index].quad, source.quad);
    }
}

#[test]
fn optional_union_refusal_and_read_failure_never_fabricate_an_alternative() {
    for exhausted in [true, false] {
        let mut sources = scene(q(423., 120., 40., 30.));
        let input = Input::new(&mut sources, 0.15);
        let mut scan = scan(&input, sources);
        let mut budget = Budget::default();
        if exhausted {
            for _ in 0..4 {
                budget.read(&q(10., 10., 40., 10.)).unwrap();
            }
        }
        let result = unions(
            &mut scan,
            &[false; 4],
            0.15,
            true,
            &mut budget,
            &mut Stage::new(None, 4),
            |_, _| {
                assert!(!exhausted);
                Err("injected reader failure".into())
            },
        );
        assert_eq!(scan.words.len(), 4);
        assert!(!scan.words[0].stray);
        if exhausted {
            result.unwrap();
            assert_eq!(
                scan.sources[4].union.as_ref().unwrap().reason,
                "read_budget"
            );
            assert_eq!(scan.sources[4].union.as_ref().unwrap().word_index, None);
        } else {
            assert_eq!(result.unwrap_err(), "injected reader failure");
        }
    }
}

/// A page whose rescues sit between ordinary words, not after them.
fn interleaved() -> Vec<RawSource> {
    let mut sources = vec![
        source(0, q(100., 100., 300., 100.)),
        source(1, q(100., 500., 450., 100.)),
        source(2, q(100., 900., 400., 100.)),
    ];
    empty(&mut sources, 300.);
    empty(&mut sources, 700.);
    sources
}

/// Which index each read was given, in the order the reads happened, and
/// whether that read was handed the shared allowance.
fn calls_of(
    input: &Input,
    traversal: Traversal,
    budget: &mut Budget,
    stage: &Stage<'_>,
) -> Vec<(usize, bool)> {
    let seen = Mutex::new(Vec::new());
    let index = |quad: &Quad| input.quads.iter().position(|q| q == quad).unwrap();
    input
        .prepare_reserved_with(traversal, budget, stage, Clone::clone, |quad, _, extra| {
            seen.lock().unwrap().push((index(quad), extra.is_some()));
            if let Some(b) = extra {
                b.classifier()?;
                b.ocr()?;
            }
            Ok(())
        })
        .unwrap();
    seen.into_inner().unwrap()
}

/// Every schedule a page can be read on, including one that really does run
/// several reads at once.
const TRAVERSALS: [Traversal; 3] = [
    Traversal::Grouped { workers: 1 },
    Traversal::Grouped { workers: 8 },
    Traversal::InInputOrder,
];

fn rescue_indices(input: &Input) -> Vec<usize> {
    (0..input.quads.len())
        .filter(|&k| input.rescue[k])
        .collect()
}

fn reserved(input: &Input) -> Budget {
    let mut budget = Budget::default();
    input.reserve(&mut budget, 0.15, true).unwrap();
    budget
}

#[test]
fn the_fixture_really_does_put_a_rescue_between_two_ordinary_words() {
    let mut sources = interleaved();
    let input = Input::new(&mut sources, 0.15);
    let rescues = rescue_indices(&input);
    assert_eq!(rescues.len(), 2, "two rescues: {:?}", input.rescue);
    let last = input.quads.len() - 1;
    assert!(
        rescues.iter().any(|&k| k > 0 && k < last),
        "a rescue must sit between ordinary words or these tests prove nothing: {:?}",
        input.rescue
    );
}

#[test]
fn grouping_moves_the_rescues_last_and_input_order_leaves_them_where_they_are() {
    let mut sources = interleaved();
    let input = Input::new(&mut sources, 0.15);
    let rescues = rescue_indices(&input);
    let stage = Stage::new(None, input.quads.len());

    let grouped = calls_of(
        &input,
        Traversal::Grouped { workers: 1 },
        &mut reserved(&input),
        &stage,
    );
    let boundary = grouped.iter().position(|(_, extra)| *extra).unwrap();
    assert!(
        grouped[boundary..].iter().all(|(_, extra)| *extra),
        "the rescue tail is contiguous and last: {grouped:?}"
    );
    let tail: Vec<usize> = grouped[boundary..].iter().map(|(k, _)| *k).collect();
    assert_eq!(
        tail, rescues,
        "cumulative claims keep the tail in input order"
    );
    assert!(
        grouped.iter().all(|&(k, extra)| extra == input.rescue[k]),
        "the allowance reaches rescues and only rescues: {grouped:?}"
    );

    let ordered = calls_of(
        &input,
        Traversal::InInputOrder,
        &mut reserved(&input),
        &stage,
    );
    assert_eq!(
        ordered.iter().map(|(k, _)| *k).collect::<Vec<_>>(),
        (0..input.quads.len()).collect::<Vec<_>>(),
        "a concrete arm's plan cache sees the widths in input order"
    );
    assert_ne!(
        ordered, grouped,
        "the fixture must distinguish the two traversals"
    );
}

#[test]
fn every_traversal_returns_each_read_in_its_own_input_slot() {
    let mut sources = interleaved();
    let input = Input::new(&mut sources, 0.15);
    let index = |quad: &Quad| input.quads.iter().position(|q| q == quad).unwrap();
    for traversal in TRAVERSALS {
        let stage = Stage::new(None, input.quads.len());
        let values = input
            .prepare_reserved_with(
                traversal,
                &mut reserved(&input),
                &stage,
                Clone::clone,
                |quad, _, extra| {
                    if let Some(b) = extra {
                        b.classifier()?;
                        b.ocr()?;
                    }
                    Ok(format!("read {}", index(quad)))
                },
            )
            .unwrap();
        assert_eq!(
            values,
            (0..input.quads.len())
                .map(|k| format!("read {k}"))
                .collect::<Vec<_>>(),
            "{traversal:?} must not permute the results"
        );
    }
}

#[test]
fn every_read_is_announced_once_and_the_delivered_count_never_goes_backwards() {
    let mut sources = interleaved();
    let input = Input::new(&mut sources, 0.15);
    for traversal in TRAVERSALS {
        let observer = Recorder::default();
        let stage = Stage::new(Some(&observer), input.quads.len());
        calls_of(&input, traversal, &mut reserved(&input), &stage);

        let events = observer.0.lock().unwrap();
        let mut open = Vec::new();
        let mut closed = Vec::new();
        for event in events.iter() {
            if let Event::Region(region) = event {
                match region.state {
                    RegionState::Reading => {
                        assert!(!open.contains(&region.id), "{:?} starts twice", region.id);
                        open.push(region.id);
                    }
                    RegionState::Read => {
                        assert!(
                            open.contains(&region.id),
                            "{:?} finishes unstarted",
                            region.id
                        );
                        closed.push(region.id);
                    }
                    _ => {}
                }
            }
        }
        assert_eq!(closed.len(), input.quads.len());
        let counts: Vec<u32> = events
            .iter()
            .filter_map(|e| match e {
                Event::Work {
                    phase: Phase::Reading,
                    completed,
                    ..
                } => Some(*completed),
                _ => None,
            })
            .collect();
        assert_eq!(counts, (0..=input.quads.len() as u32).collect::<Vec<_>>());
    }
}

#[test]
fn more_rescues_than_the_allowance_covers_are_refused_before_any_read() {
    let mut sources = interleaved();
    for y in [1100., 1300., 1500., 1700.] {
        empty(&mut sources, y);
    }
    let input = Input::new(&mut sources, 0.15);
    assert!(
        rescue_indices(&input).len() > crate::read_budget::MAX_READS,
        "the fixture must ask for more rescues than the allowance covers"
    );
    let mut budget = Budget::default();
    assert!(input.reserve(&mut budget, 0.15, true).is_err());
}

#[test]
fn a_page_with_no_words_and_a_page_with_one_both_read_cleanly() {
    for traversal in TRAVERSALS {
        let mut none: Vec<RawSource> = Vec::new();
        let input = Input::new(&mut none, 0.15);
        assert!(input.quads.is_empty());
        let stage = Stage::new(None, 0);
        assert!(calls_of(&input, traversal, &mut Budget::default(), &stage).is_empty());

        let mut one = vec![source(0, q(100., 100., 300., 100.))];
        let input = Input::new(&mut one, 0.15);
        let stage = Stage::new(None, input.quads.len());
        assert_eq!(
            calls_of(&input, traversal, &mut Budget::default(), &stage),
            vec![(0, false)]
        );
    }
}

#[test]
#[cfg(feature = "scan-profile")]
fn only_the_symbolic_arm_is_free_to_regroup() {
    use crate::recogniser::Preparation;
    assert!(matches!(
        Traversal::for_recogniser(None),
        Traversal::Grouped { .. }
    ));
    assert!(matches!(
        Traversal::for_recogniser(Some(Preparation::Symbolic)),
        Traversal::Grouped { .. }
    ));
    for concrete in [Preparation::Optimized, Preparation::TypedOnly] {
        assert_eq!(
            Traversal::for_recogniser(Some(concrete)),
            Traversal::InInputOrder,
            "{concrete:?} keeps an LRU whose diagnostics follow the input order"
        );
    }
}

/// An observer that fails if two events are ever published at once.
#[derive(Default)]
struct Exclusive {
    inside: std::sync::atomic::AtomicBool,
    events: Mutex<Vec<Event>>,
}

impl Observer for Exclusive {
    fn on_event(&self, event: Event) {
        assert!(
            !self.inside.swap(true, Ordering::SeqCst),
            "two events were published at once; the app's stream would be malformed"
        );
        std::thread::sleep(std::time::Duration::from_micros(200));
        self.events.lock().unwrap().push(event);
        self.inside.store(false, Ordering::SeqCst);
    }
}

/// Holds every arrival until the whole party is present, or gives up.
///
/// A read that waits here proves the others are running beside it; a run that
/// cannot assemble the party times out and fails, rather than passing because
/// a sleep happened to be long enough.
struct Gate {
    waiting: Mutex<usize>,
    party: usize,
    open: std::sync::Condvar,
}

impl Gate {
    fn new(party: usize) -> Self {
        Self {
            waiting: Mutex::new(0),
            party,
            open: std::sync::Condvar::new(),
        }
    }

    fn assemble(&self) {
        let mut waiting = self.waiting.lock().unwrap();
        *waiting += 1;
        if *waiting >= self.party {
            self.open.notify_all();
            return;
        }
        let (waiting, timeout) = self
            .open
            .wait_timeout_while(waiting, std::time::Duration::from_secs(10), |w| {
                *w < self.party
            })
            .unwrap();
        assert!(
            !timeout.timed_out(),
            "only {} of {} reads ever ran at once",
            *waiting,
            self.party
        );
    }
}

#[test]
fn the_reads_of_a_page_genuinely_run_beside_each_other() {
    let mut sources = interleaved();
    let input = Input::new(&mut sources, 0.15);
    let ordinary = input.quads.len() - rescue_indices(&input).len();
    assert!(ordinary >= 2, "the fixture needs reads to overlap");
    let observer = Exclusive::default();
    let stage = Stage::new(Some(&observer), input.quads.len());
    let index = |quad: &Quad| input.quads.iter().position(|q| q == quad).unwrap();
    let gate = Gate::new(ordinary);

    let values = input
        .prepare_reserved_with(
            Traversal::Grouped { workers: 8 },
            &mut reserved(&input),
            &stage,
            Clone::clone,
            |quad, _, extra| {
                // A rescue reads alone, after the party has dispersed.
                if extra.is_none() {
                    gate.assemble();
                }
                if let Some(b) = extra {
                    b.classifier()?;
                    b.ocr()?;
                }
                Ok(index(quad))
            },
        )
        .unwrap();

    assert_eq!(values, (0..input.quads.len()).collect::<Vec<_>>());
    let events = observer.events.lock().unwrap();
    let counts: Vec<u32> = events
        .iter()
        .filter_map(|e| match e {
            Event::Work {
                phase: Phase::Reading,
                completed,
                ..
            } => Some(*completed),
            _ => None,
        })
        .collect();
    assert_eq!(counts, (0..=input.quads.len() as u32).collect::<Vec<_>>());
}

/// Lets the reads finish one at a time, in an order the test chooses.
///
/// A turn only advances once the previous read's `Read` has actually been
/// published, so what is asserted is the order the app would see rather than
/// the order the closures happened to return in.
struct ReleaseOrder {
    remaining: Mutex<Vec<usize>>,
    turn: std::sync::Condvar,
    events: Mutex<Vec<Event>>,
}

impl ReleaseOrder {
    /// `order` finishes front to back.
    fn new(order: Vec<usize>) -> Self {
        Self {
            remaining: Mutex::new(order.into_iter().rev().collect()),
            turn: std::sync::Condvar::new(),
            events: Mutex::new(Vec::new()),
        }
    }

    fn wait_for(&self, index: usize) {
        let mut remaining = self.remaining.lock().unwrap();
        while remaining.last() != Some(&index) {
            let (next, timeout) = self
                .turn
                .wait_timeout(remaining, std::time::Duration::from_secs(10))
                .unwrap();
            assert!(!timeout.timed_out(), "read {index} was never let go");
            remaining = next;
        }
    }

    fn finished(&self) -> Vec<usize> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                Event::Region(region) if region.state == RegionState::Read => {
                    Some(region.id as usize)
                }
                _ => None,
            })
            .collect()
    }
}

impl Observer for ReleaseOrder {
    fn on_event(&self, event: Event) {
        if let Event::Region(region) = &event
            && region.state == RegionState::Read
        {
            let mut remaining = self.remaining.lock().unwrap();
            if remaining.last() == Some(&(region.id as usize)) {
                remaining.pop();
                self.turn.notify_all();
            }
        }
        self.events.lock().unwrap().push(event);
    }
}

#[test]
fn reads_that_finish_backwards_still_land_in_their_own_slots() {
    let mut sources = interleaved();
    let input = Input::new(&mut sources, 0.15);
    let n = input.quads.len();
    let ordinary: Vec<usize> = (0..n).filter(|&k| !input.rescue[k]).collect();
    let backwards: Vec<usize> = ordinary.iter().rev().copied().collect();
    let index = |quad: &Quad| input.quads.iter().position(|q| q == quad).unwrap();
    let observer = ReleaseOrder::new(backwards.clone());
    let stage = Stage::new(Some(&observer), n);
    let gate = Gate::new(ordinary.len());

    let values = input
        .prepare_reserved_with(
            Traversal::Grouped { workers: 8 },
            &mut reserved(&input),
            &stage,
            Clone::clone,
            |quad, _, extra| {
                let k = index(quad);
                if extra.is_none() {
                    gate.assemble();
                    observer.wait_for(k);
                }
                if let Some(b) = extra {
                    b.classifier()?;
                    b.ocr()?;
                }
                Ok(format!("read {k}"))
            },
        )
        .unwrap();

    assert_eq!(
        values,
        (0..n).map(|k| format!("read {k}")).collect::<Vec<_>>()
    );
    let finished: Vec<usize> = observer
        .finished()
        .into_iter()
        .filter(|id| ordinary.contains(id))
        .collect();
    assert_eq!(
        finished, backwards,
        "the reads finished last first, so that is the order the app saw"
    );
}

#[test]
fn the_accumulator_keeps_the_earliest_failure_whatever_order_outcomes_arrive_in() {
    // The pass is what decides, so it is what is asked directly: a worker
    // cannot be made to record at a chosen moment, and a test that pretends
    // otherwise only orders the closures.
    let quad = q(0., 0., 10., 10.);
    for (earlier, later) in [(0usize, 3usize), (1, 2)] {
        for (first, second) in [(earlier, later), (later, earlier)] {
            let stage = Stage::new(None, 4);
            let mut pass: Pass<()> = Pass::new(4);
            pass.record(first, &quad, &stage, Err(format!("refused {first}")));
            pass.record(second, &quad, &stage, Err(format!("refused {second}")));
            assert_eq!(
                pass.finish(),
                Err(format!("refused {earlier}")),
                "recorded {first} then {second}"
            );
        }
    }
}

#[test]
fn a_failure_anywhere_reports_the_lowest_index_that_failed() {
    let mut sources = interleaved();
    let input = Input::new(&mut sources, 0.15);
    let n = input.quads.len();
    let index = |quad: &Quad| input.quads.iter().position(|q| q == quad).unwrap();
    for traversal in TRAVERSALS {
        for first in 0..n {
            for second in 0..n {
                let stage = Stage::new(None, n);
                let refused = input.prepare_reserved_with(
                    traversal,
                    &mut reserved(&input),
                    &stage,
                    Clone::clone,
                    |quad, _, _| {
                        let k = index(quad);
                        if k == first || k == second {
                            Err(format!("refused {k}"))
                        } else {
                            Ok(String::new())
                        }
                    },
                );
                assert_eq!(refused, Err(format!("refused {}", first.min(second))));
            }
        }
    }
}

#[test]
#[cfg(feature = "scan-profile")]
fn a_live_call_budget_keeps_every_read_on_the_counting_thread() {
    let mut sources = interleaved();
    let input = Input::new(&mut sources, 0.15);
    let stage = Stage::new(None, input.quads.len());
    let threads = Mutex::new(std::collections::HashSet::new());
    let scope = crate::inference_limit::Scope::new(0, 0).unwrap();
    input
        .prepare_reserved_with(
            Traversal::Grouped { workers: 8 },
            &mut reserved(&input),
            &stage,
            Clone::clone,
            |_, _, extra| {
                threads.lock().unwrap().insert(std::thread::current().id());
                if let Some(b) = extra {
                    b.classifier()?;
                    b.ocr()?;
                }
                Ok(())
            },
        )
        .unwrap();
    drop(scope);
    assert_eq!(
        threads.into_inner().unwrap().len(),
        1,
        "a per-thread budget counts nothing a worker does"
    );
}

#[test]
fn a_page_never_spawns_more_threads_than_it_has_words_to_read() {
    assert_eq!(effective_workers(4, 0), 0, "no work wants no thread");
    assert_eq!(effective_workers(4, 1), 1);
    assert_eq!(effective_workers(4, 2), 2);
    assert_eq!(effective_workers(8, 3), 3, "asking for more than there is");
    assert_eq!(effective_workers(1, 8), 1);
    assert_eq!(
        effective_workers(0, 8),
        1,
        "nobody reads zero words at a time"
    );
    assert_eq!(
        effective_workers(usize::MAX, 1000),
        MAX_SPAWNED,
        "a number is not permission to exhaust the thread table"
    );
    assert_eq!(effective_workers(usize::MAX, 5), 5);
}

#[test]
fn a_failing_page_joins_its_workers_before_it_returns() {
    let mut sources = interleaved();
    let input = Input::new(&mut sources, 0.15);
    let n = input.quads.len();
    let ordinary: Vec<usize> = (0..n).filter(|&k| !input.rescue[k]).collect();
    let index = |quad: &Quad| input.quads.iter().position(|q| q == quad).unwrap();
    let observer = Late::default();
    let stage = Stage::new(Some(&observer), n);
    let gate = Gate::new(ordinary.len());

    // Every ordinary read is in flight before the first of them fails, so a
    // pass that returned without joining would leave the others announcing.
    let refused = input.prepare_reserved_with(
        Traversal::Grouped { workers: 8 },
        &mut reserved(&input),
        &stage,
        Clone::clone,
        |quad, _, extra| {
            let k = index(quad);
            if extra.is_none() {
                gate.assemble();
                if k == ordinary[0] {
                    return Err(format!("refused {k}"));
                }
            }
            if let Some(b) = extra {
                b.classifier()?;
                b.ocr()?;
            }
            Ok(())
        },
    );
    assert_eq!(refused, Err(format!("refused {}", ordinary[0])));
    observer.closed.store(true, Ordering::SeqCst);
}

/// Fails the test if an event arrives after the call that produced it returned.
#[derive(Default)]
struct Late {
    closed: std::sync::atomic::AtomicBool,
}

impl Observer for Late {
    fn on_event(&self, _: Event) {
        assert!(
            !self.closed.load(Ordering::SeqCst),
            "a worker outlived the read that spawned it"
        );
    }
}

#[test]
#[cfg(feature = "scan-profile")]
fn a_worker_s_timings_come_back_to_the_scan_that_spawned_it() {
    let mut sources = interleaved();
    let input = Input::new(&mut sources, 0.15);
    let ordinary: Vec<usize> = (0..input.quads.len())
        .filter(|&k| !input.rescue[k])
        .collect();
    let stage = Stage::new(None, input.quads.len());
    let gate = Gate::new(ordinary.len());
    crate::timing::take();

    input
        .prepare_reserved_with(
            Traversal::Grouped { workers: 8 },
            &mut reserved(&input),
            &stage,
            Clone::clone,
            |_, _, extra| {
                let _span = crate::timing::span("test.read");
                if extra.is_none() {
                    gate.assemble();
                }
                if let Some(b) = extra {
                    b.classifier()?;
                    b.ocr()?;
                }
                Ok(())
            },
        )
        .unwrap();

    let report = crate::timing::take();
    let spans: std::collections::HashMap<&str, &serde_json::Value> = report["spans"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| (s["name"].as_str().unwrap(), s))
        .collect();
    assert_eq!(
        spans["test.read"]["calls"],
        input.quads.len(),
        "every read's span reached the report, whichever thread ran it"
    );
    assert_eq!(
        report["read_workers"],
        ordinary.len(),
        "the report says how many threads that sum came from"
    );
    assert_eq!(
        spans["phrase.prepare_elapsed"]["calls"], 1,
        "the pass is timed once, on the thread that owns it, whatever it spawned"
    );
    assert!(spans["phrase.prepare_elapsed"]["ms"].as_f64().unwrap() > 0.);
}

#[test]
#[cfg(feature = "scan-profile")]
fn every_schedule_says_how_many_threads_it_used() {
    let mut sources = interleaved();
    let input = Input::new(&mut sources, 0.15);
    for traversal in [Traversal::Grouped { workers: 1 }, Traversal::InInputOrder] {
        let stage = Stage::new(None, input.quads.len());
        crate::timing::take();
        calls_of(&input, traversal, &mut reserved(&input), &stage);
        assert_eq!(
            crate::timing::take()["read_workers"],
            1,
            "{traversal:?} read on one thread and must say so"
        );
    }
}
