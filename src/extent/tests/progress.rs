use super::*;
use crate::progress::{Event, FinalRegion, Observer, Phase, Region, RegionState};
use std::sync::Mutex;

#[derive(Default)]
struct Recorder(Mutex<Vec<Event>>);

impl Observer for Recorder {
    fn on_event(&self, event: Event) {
        self.0.lock().unwrap().push(event);
    }
}

// The left column gets a real native join, which permutes the word indices
// BEFORE the right column's extent read. No models, phone or saved photos.
fn run_case(
    pair: bool,
    probability: Option<f32>,
    observed: bool,
) -> (PageScan, Report, Vec<FinalRegion>, Vec<Event>) {
    run_case_traced(pair, probability, observed, false)
}

fn run_case_traced(
    pair: bool,
    probability: Option<f32>,
    observed: bool,
    trace: bool,
) -> (PageScan, Report, Vec<FinalRegion>, Vec<Event>) {
    let mut quads = vec![
        q(100., 50., 100., 20.),
        q(100., 100., 35., 20.),
        q(140., 100., 60., 20.),
        q(100., 150., 100., 20.),
        q(400., 50., 100., 20.),
        q(400., 100., if pair { 35. } else { 100. }, 20.),
    ];
    if pair {
        quads.push(q(440., 100., 60., 20.));
    }
    quads.push(q(400., 150., 100., 20.));
    let count = quads.len();
    let (layout, mut scan, mut report) = scene(quads.clone());
    report.arm = Arm::E3;
    report.audit = Audit::None;
    for p in [1, 2]
        .into_iter()
        .chain(if pair { vec![5, 6] } else { vec![] })
    {
        scan.words[p].ranked[0].1 = 0.5;
        scan.words[p].select_fixture(0, false);
    }
    if pair {
        scan.words[6].stray = true;
    }
    if trace {
        for word in &mut scan.words {
            history::enable_fixture(word);
        }
    }
    let recorder = Recorder::default();
    let mut stage = Stage::new(observed.then_some(&recorder as &dyn Observer), count);
    for (i, w) in scan.words.iter().enumerate() {
        stage.region(i, &w.quad, RegionState::Found);
        stage.region(
            i,
            &w.quad,
            if w.stray {
                RegionState::Excluded
            } else {
                RegionState::Read
            },
        );
        stage.read_done(i + 1);
    }
    stage.checking();
    crate::joins::apply(
        &layout,
        &vec![false; count],
        &mut scan,
        CROP_MARGIN,
        &|quad| {
            let mut w = word(quad.clone(), 0);
            if trace {
                history::enable_fixture(&mut w);
            }
            if quad.0[0].0 > 300. {
                w.select_fixture(0, false);
            }
            Ok(w)
        },
        &mut stage,
    )
    .unwrap();
    assert_eq!(scan.words.len(), count + 1);
    assert_eq!(scan.words[2].joined_from, Some([1, 3]));
    for (source, index) in report.sources.iter_mut().zip(
        scan.words
            .iter()
            .enumerate()
            .filter_map(|(i, w)| w.joined_from.is_none().then_some(i)),
    ) {
        source.word_index = index;
    }
    let before = scan.clone();
    let target_ids = if pair { vec![5, 6] } else { vec![5] };
    let mut photo = RgbImage::from_pixel(800, 300, Rgb([255; 3]));
    paint(&mut photo, 394, 108, 416, 112);
    let mut calls = 0;
    let result = crate::extent::apply_with(
        &photo,
        &layout,
        &mut scan,
        &mut report,
        &mut stage,
        |quad, budget| {
            calls += 1;
            budget.classifier()?;
            budget.ocr()?;
            if observed {
                let events = recorder.0.lock().unwrap();
                // Reading updates arrive synchronously BEFORE the crop read,
                // using IDs 5/6, not their post-join word indices 6/7.
                let expected: Vec<_> = target_ids
                    .iter()
                    .map(|&id| {
                        Event::Region(Region {
                            id,
                            quad: quads[id as usize].clone(),
                            state: RegionState::Reading,
                        })
                    })
                    .collect();
                assert_eq!(&events[events.len() - expected.len()..], expected);
            }
            let mut candidate = word(quad.clone(), 0);
            if trace {
                history::enable_fixture(&mut candidate);
            }
            candidate.ranked[0].1 = probability.ok_or("test read failed")?;
            candidate.select_fixture(0, false);
            Ok(candidate)
        },
    );
    assert_eq!(calls, 1);
    assert_eq!(result.is_err(), probability.is_none());
    if probability.is_none_or(|p| p < 0.5) {
        assert_eq!(
            history::without_history(&scan),
            history::without_history(&before)
        );
    }
    let regions = stage.into_regions();
    if result.is_ok() {
        history::terminal(&mut scan, &regions, "scan_return");
    }
    let events = recorder.0.into_inner().unwrap();
    if observed {
        assert_eq!(regions.len(), scan.words.len());
        let mut indices: Vec<_> = regions.iter().map(|r| r.word_index as usize).collect();
        indices.sort_unstable();
        assert_eq!(indices, (0..scan.words.len()).collect::<Vec<_>>());
        assert_eq!(
            regions.iter().map(|r| r.id as usize).collect::<Vec<_>>(),
            (0..scan.words.len()).collect::<Vec<_>>()
        );
        for region in &regions[..count] {
            assert_eq!(
                scan.words[region.word_index as usize].quad,
                quads[region.id as usize]
            );
        }
        let join = &scan.words[regions[count].word_index as usize];
        assert_eq!(
            join.joined_from,
            Some([
                regions[1].word_index as usize,
                regions[2].word_index as usize
            ])
        );
        let checking = events
            .iter()
            .position(|e| {
                matches!(
                    e,
                    Event::Work {
                        phase: Phase::Checking,
                        ..
                    }
                )
            })
            .unwrap();
        assert!(
            events[checking + 1..]
                .iter()
                .all(|e| !matches!(e, Event::Work { .. }))
        );
        let reading: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                Event::Work {
                    phase: Phase::Reading,
                    completed,
                    total,
                } => Some((*completed, *total)),
                _ => None,
            })
            .collect();
        assert_eq!(
            reading,
            (0..=count as u32)
                .map(|n| (n, Some(count as u32)))
                .collect::<Vec<_>>()
        );
        assert!(events.iter().all(|e| !e.is_terminal())); // Packing still belongs to mobile.
    } else {
        assert!(regions.is_empty() && events.is_empty());
    }
    (scan, report, regions, events)
}

#[test]
fn replacement_history_survives_two_permutations_refusal_and_restoration_without_extra_work() {
    for (pair, probability) in [(true, 0.7), (false, 0.97), (true, 0.4)] {
        let (plain, plain_report, plain_regions, plain_events) =
            run_case_traced(pair, Some(probability), true, false);
        let (mut scan, mut report, regions, events) =
            run_case_traced(pair, Some(probability), true, true);
        assert_eq!(
            history::without_history(&scan),
            history::without_history(&plain)
        );
        assert_eq!(report, plain_report); // includes exact read/ROI budgets and work counters
        assert_eq!(regions, plain_regions);
        assert_eq!(events, plain_events);
        for (i, word) in scan.words.iter().enumerate() {
            let value = word.evidence.decisions.as_ref().unwrap().to_json();
            let events = value["events"].as_array().unwrap();
            let terminal = events.last().unwrap();
            assert_eq!(terminal["rule"], "terminal");
            assert_eq!(terminal["kept"], !word.stray);
            assert_eq!(terminal["links"][0]["word_index"], i);
            assert_eq!(
                terminal["region_id"],
                regions
                    .iter()
                    .find(|r| r.word_index as usize == i)
                    .unwrap()
                    .id
            );
            for e in events.iter().filter(|e| e["rule"] == "replacement_read") {
                let parents: Vec<_> = e["links"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|l| l["role"] == "parent")
                    .collect();
                for (link, before) in parents.iter().zip(e["parents"].as_array().unwrap()) {
                    let index = link["word_index"].as_u64().unwrap() as usize;
                    assert_eq!(before["quad"], json!(scan.words[index].quad.0));
                }
                for candidate in e["links"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|l| l["role"] == "candidate")
                {
                    let index = candidate["word_index"].as_u64().unwrap() as usize;
                    assert_eq!(e["candidate"]["quad"], json!(scan.words[index].quad.0));
                }
                if e["owner"] == "extent" && probability == 0.4 {
                    assert_eq!(e["reason"], "parent_surer");
                    assert_eq!(e["candidate_retained"], false);
                    assert_eq!(
                        e["discarded_read_history"]["events"][0]["rule"],
                        "fixture_read"
                    );
                }
            }
        }
        if let Some(trial) = report.trials.iter().position(|t| t.active) {
            let selected = report.trials[trial].selected.unwrap();
            let originals: Vec<_> = report.trials[trial]
                .parents
                .iter()
                .copied()
                .zip(report.trials[trial].original_strays.iter().copied())
                .collect();
            for active in [false, true, false] {
                report.choose_expansion(&mut scan, trial, active).unwrap();
                assert_eq!(scan.words[selected].stray, !active);
                for &(index, stray) in &originals {
                    assert_eq!(scan.words[index].stray, if active { true } else { stray });
                    let value = scan.words[index]
                        .evidence
                        .decisions
                        .as_ref()
                        .unwrap()
                        .to_json();
                    let events = value["events"].as_array().unwrap();
                    assert_eq!(events[events.len() - 2]["rule"], "keep_transition");
                    assert_eq!(events.last().unwrap()["scope"], "explicit_extent_choice");
                    assert_eq!(events.last().unwrap()["kept"], !scan.words[index].stray);
                }
            }
        }
    }
}

#[test]
fn single_and_pair_replacements_survive_both_owner_permutations_without_confirming() {
    for pair in [false, true] {
        let probability = Some(if pair { 0.7 } else { 0.97 });
        let (scan, report, regions, events) = run_case(pair, probability, true);
        let (plain, plain_report, _, _) = run_case(pair, probability, false);
        assert_eq!(format!("{scan:?}"), format!("{plain:?}"));
        assert_eq!(report, plain_report); // Includes work, originals and restoration flags.
        let expanded = report.trials.iter().find(|t| t.active).unwrap();
        let selected = expanded.selected.unwrap();
        assert_eq!(scan.words[selected].verdict(), Verdict::Uncertain);
        assert!(scan.words[selected].joined_from.is_none());
        assert_eq!(
            expanded.original_strays,
            if pair { vec![false, true] } else { vec![false] }
        );
        let (region, parents) = events
            .iter()
            .filter_map(|e| match e {
                Event::Replaced { region, parents } => Some((region, parents)),
                _ => None,
            })
            .next_back()
            .unwrap();
        assert_eq!(parents, &if pair { vec![5, 6] } else { vec![5] });
        assert_eq!(region.id as usize, scan.words.len() - 1);
        assert_eq!(regions[region.id as usize].word_index as usize, selected);
        assert_eq!(region.quad, scan.words[selected].quad);
        assert_eq!(region.state, RegionState::Read);
        assert!(!scan.words[selected].stray);
        assert_eq!(
            scan.words[selected].expanded_from,
            expanded
                .parents
                .iter()
                .zip(&expanded.original_strays)
                .map(|(&word_index, &stray)| crate::phrase::ExtentParent { word_index, stray })
                .collect::<Vec<_>>()
        );
        for (&id, &parent) in parents.iter().zip(&expanded.parents) {
            assert_eq!(regions[id as usize].word_index as usize, parent);
            assert!(scan.words[parent].stray);
        }
    }
}

#[test]
fn rejected_or_failed_extent_reads_restore_visible_and_excluded_parent_ids() {
    for probability in [Some(0.4), None] {
        let (scan, _, regions, events) = run_case(true, probability, true);
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, Event::Replaced { .. }))
                .count(),
            1
        ); // Native join only.
        for (event, id, state) in [
            (&events[events.len() - 2], 5, RegionState::Read),
            (events.last().unwrap(), 6, RegionState::Excluded),
        ] {
            assert_eq!(
                event,
                &Event::Region(Region {
                    id,
                    quad: scan.words[regions[id as usize].word_index as usize]
                        .quad
                        .clone(),
                    state,
                })
            );
        }
    }
}
