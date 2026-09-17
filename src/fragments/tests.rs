use super::*;
use crate::sources::{Outcome, Part};
use crate::split::EmptySplit;
use crate::split::frame_of;

fn q(x: f32, y: f32, w: f32, h: f32) -> Quad {
    Quad([(x, y), (x + w, y), (x + w, y + h), (x, y + h)])
}

fn source(detector: usize, quad: Quad) -> RawSource {
    RawSource {
        decision: None,
        union: None,
        detector,
        quad: quad.clone(),
        outcome: Outcome::Parts(vec![Part {
            part: 0,
            quad,
            word_index: Some(detector),
        }]),
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
    .map(|(i, q)| source(i, q))
    .collect()
}

fn owned(plan: &Plan, i: usize) -> &Ownership {
    match &plan.decisions[i] {
        Decision::Tiny { scale, ownership } => {
            assert!(!scale.supports.contains(&i));
            ownership
        }
        other => panic!("expected tiny source, got {other:?}"),
    }
}

#[test]
fn a_clipped_descender_is_owned_ink_not_a_claim_of_full_crop_coverage() {
    let sources = scene(q(300., 190., 40., 40.));
    let before = sources.clone();
    let plan = plan(&sources, 0.15);
    assert_eq!(
        owned(&plan, 4),
        &Ownership::Descender(Observation {
            source: 0,
            part: Some(0)
        })
    );
    assert_eq!(
        sources, before,
        "planning must not alter a crop or attach a reading"
    );
    let duplicate = plan_for(q(300., 160., 40., 30.));
    assert_eq!(
        owned(&duplicate, 4),
        &Ownership::Duplicate(Observation {
            source: 0,
            part: Some(0)
        })
    );
}

fn plan_for(fragment: Quad) -> Plan {
    plan(&scene(fragment), 0.15)
}

#[test]
fn an_ending_union_uses_full_raw_geometry_even_if_its_split_is_empty_or_clipped() {
    for empty in [false, true] {
        let mut sources = scene(q(423., 120., 40., 30.));
        sources[4].outcome = if empty {
            Outcome::Empty(EmptySplit::NoInkAfterRules)
        } else {
            Outcome::Parts(vec![Part {
                part: 0,
                quad: q(423., 120., 7., 30.),
                word_index: Some(4),
            }])
        };
        let plan = plan(&sources, 0.15);
        let Ownership::Ending {
            owner,
            union,
            footprint,
        } = owned(&plan, 4)
        else {
            panic!("no ending proposal");
        };
        assert_eq!(
            *owner,
            Observation {
                source: 0,
                part: Some(0)
            }
        );
        assert_eq!(
            bounds_in(frame_of(&sources[0].quad), union),
            [-150., -50., 213., 50.]
        );
        assert_eq!(frame_of(footprint).w, 393.);
        assert_eq!(frame_of(footprint).h, 130.);
        assert_eq!(
            plan.observations[4].part,
            if empty { None } else { Some(0) }
        );
        assert_eq!(sources[4].quad.0[1].0, 463.);
    }
}

#[test]
fn short_words_and_empty_whole_words_are_not_tiny() {
    let mut sources = scene(q(300., 190., 40., 40.));
    sources.push(source(5, q(100., 900., 115., 100.))); // fee-sized
    let mut absorb = source(6, q(100., 1100., 400., 100.));
    absorb.outcome = Outcome::Empty(EmptySplit::FlatPhotoSamples);
    sources.push(absorb);
    let p = plan(&sources, 0.15);
    assert!(matches!(p.decisions[5], Decision::Word(_)));
    assert!(matches!(p.decisions[6], Decision::Word(_)));
    assert!(p.observations.contains(&Observation {
        source: 6,
        part: None
    }));
    assert_eq!(p.decisions.len(), 7); // Not contingent on thirteen boxes.
}

#[test]
fn a_multiword_raw_owner_resolves_the_part_not_a_final_word_index() {
    let mut sources = scene(q(423., 120., 40., 30.));
    sources[0].outcome = Outcome::Parts(vec![
        Part {
            part: 2,
            quad: q(100., 100., 100., 100.),
            word_index: Some(19),
        },
        Part {
            part: 7,
            quad: q(250., 100., 150., 100.),
            word_index: Some(3),
        },
    ]);
    let p = plan(&sources, 0.15);
    let Ownership::Ending { owner, union, .. } = owned(&p, 4) else {
        panic!("no ending proposal");
    };
    assert_eq!(
        *owner,
        Observation {
            source: 0,
            part: Some(7)
        }
    );
    assert_eq!(owner.quad(&sources), &sources[0].parts()[1].quad);
    assert_eq!(union.0[0].0, 250., "do not include the unrelated left word");
}

#[test]
fn scale_counts_other_raw_sources_once_not_each_split_part() {
    let mut sources = scene(q(300., 190., 40., 40.));
    sources.remove(3);
    sources.remove(2);
    let original = sources[0].quad.clone();
    sources[0].outcome = Outcome::Parts(
        (0..6)
            .map(|part| Part {
                part,
                quad: original.clone(),
                word_index: Some(part),
            })
            .collect(),
    );
    let p = plan(&sources, 0.15);
    assert_eq!(p.observations.len(), 8);
    assert_eq!(p.decisions[2], Decision::NoScale(NoScale::TooFewSupports));
}

fn transformed(q: &Quad, scale: f32, angle: f32) -> Quad {
    Quad(q.0.map(|(x, y)| {
        let (dx, dy) = turn(x * scale, y * scale, angle);
        (3000. + dx, 3000. + dy)
    }))
}

#[test]
fn ownership_scales_and_rotates_with_writing_without_pixel_cutoffs() {
    for scale in [0.25, 1., 4.] {
        for angle in [-12., 0., 12.] {
            let sources: Vec<_> = scene(q(300., 190., 40., 40.))
                .into_iter()
                .map(|s| source(s.detector, transformed(&s.quad, scale, angle)))
                .collect();
            let p = plan(&sources, 0.15);
            assert_eq!(
                owned(&p, 4),
                &Ownership::Descender(Observation {
                    source: 0,
                    part: Some(0)
                }),
                "{scale} {angle}"
            );
            if let Decision::Tiny { scale: s, .. } = &p.decisions[4] {
                assert!((s.height - 100. * scale).abs() < 0.01);
            }
        }
    }
    assert!(same_axis(-89., 89.));
    assert!(!same_axis(0., 16.));
}

#[test]
fn shared_page_direction_preserves_descender_and_ending_owners_at_quarter_and_half_turns() {
    for angle in [0., 17., 90., 180., 270.] {
        let d = crate::page_frame::test_direction(angle);
        let writing = WritingFrame::Page(d);
        for (fragment, ending) in [
            (q(300., 190., 40., 40.), false),
            (q(423., 120., 30., 30.), true),
        ] {
            let mut sources: Vec<_> = scene(fragment)
                .into_iter()
                .map(|s| source(s.detector, d.quad_to_photo(&s.quad)))
                .collect();
            let before = sources.clone();
            let plan = plan_with_writing(&sources, 0.15, writing);
            let owner = Observation {
                source: 0,
                part: Some(0),
            };
            if ending {
                let Ownership::Ending {
                    owner: found,
                    union,
                    footprint,
                } = owned(&plan, 4)
                else {
                    panic!("missing ending at {angle}")
                };
                assert_eq!(*found, owner);
                let f = writing.crop_frame(union, 0.15);
                assert!((writing.frame_of(footprint).w - f.w.round()).abs() < 0.002);
                assert!((writing.frame_of(footprint).h - f.h.round()).abs() < 0.002);
            } else {
                assert_eq!(owned(&plan, 4), &Ownership::Descender(owner));
            }
            assert_eq!(sources, before);
            let input = crate::repair::Input::with_writing(&mut sources, 0.15, writing);
            assert_eq!(input.layout.writing, writing);
            assert_eq!(sources[0].quad, before[0].quad);
            assert!(sources[4].parts()[0].word_index.is_none());
            assert!(owner.word_index(&sources).is_some());
        }
    }
}

#[test]
fn conflicting_axes_or_columns_never_authorize_an_ownership_guess() {
    let mut sources = scene(q(300., 190., 40., 40.));
    let f = frame_of(&sources[1].quad);
    let turned = quad_in(
        Frame { angle: 20., ..f },
        [-f.w / 2., -f.h / 2., f.w / 2., f.h / 2.],
    );
    sources[1] = source(1, turned);
    assert_eq!(
        plan(&sources, 0.15).decisions[4],
        Decision::NoScale(NoScale::ConflictingAxes)
    );
    let mut sources = scene(q(300., 190., 40., 40.));
    sources[4].outcome = Outcome::Parts(vec![
        Part {
            part: 0,
            quad: sources[4].quad.clone(),
            word_index: Some(4),
        },
        Part {
            part: 1,
            quad: q(1000., 190., 40., 40.),
            word_index: Some(5),
        },
    ]);
    assert_eq!(
        plan(&sources, 0.15).decisions[4],
        Decision::NoScale(NoScale::MultipleColumns)
    );
}

#[test]
fn multiple_owners_are_ambiguous_even_when_one_is_closer() {
    let mut sources = scene(q(300., 190., 40., 40.));
    sources.push(source(5, q(95., 95., 310., 100.)));
    assert_eq!(
        owned(&plan(&sources, 0.15), 4),
        &Ownership::Unowned(Unowned::Ambiguous)
    );
}

#[test]
fn other_words_veto_descender_and_union_footprints() {
    let mut sources = scene(q(300., 190., 40., 40.));
    sources.push(source(5, q(320., 220., 120., 100.)));
    assert_eq!(
        owned(&plan(&sources, 0.15), 4),
        &Ownership::Unowned(Unowned::OtherObservation)
    );
    let mut sources = scene(q(423., 120., 40., 30.));
    // Does not touch the union itself (right=463); touches the reader margin.
    sources.push(source(5, q(470., 160., 120., 100.)));
    assert_eq!(
        owned(&plan(&sources, 0.15), 4),
        &Ownership::Unowned(Unowned::OtherObservation)
    );
}

#[test]
fn leading_labels_win_over_a_possible_right_ending() {
    let mut sources = scene(q(423., 120., 30., 30.));
    sources.push(source(5, q(470., 100., 180., 100.)));
    assert!(matches!(
        plan(&sources, 0.15).decisions[4],
        Decision::PossibleLabel(_)
    ));
    // A detached piece above the word is not a descender or a right ending.
    assert_eq!(
        owned(&plan_for(q(300., 50., 40., 40.)), 4),
        &Ownership::Unowned(Unowned::NoOwner)
    );
}
