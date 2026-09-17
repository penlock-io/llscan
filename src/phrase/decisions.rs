//! Opt-in, crop-owned processing history. Never re-run readers to explain a read.

use serde_json::{Value, json};

/// Decision evidence attached to one observation, following it through reindexing.
/// It contains no additional image copies and is absent when collection is off.
#[derive(Clone, Debug, Default)]
pub struct DecisionTrace {
    events: Vec<Value>,
}

impl DecisionTrace {
    pub(super) fn append(trace: &mut Option<Self>, other: Option<Self>) {
        if let (Some(trace), Some(other)) = (trace, other) {
            trace.events.extend(other.events);
        }
    }
    /// Versioned diagnostic payload. Only recorded events are evidence; missing
    /// events in older exports must not be interpreted as skipped processing.
    pub fn to_json(&self) -> Value {
        json!({"schema":"penlock-word-decisions-v1", "events":self.events})
    }

    pub fn push(trace: &mut Option<Self>, event: impl FnOnce() -> Value) {
        if let Some(trace) = trace {
            trace.events.push(event());
        }
    }

    /// Only explicit top-level links belong to this stage's word namespace.
    /// Nested discarded-read histories contain crop events, not guessed IDs.
    pub(super) fn reindex(&mut self, inverse: &[usize]) {
        for event in &mut self.events {
            if let Some(links) = event.get_mut("links").and_then(Value::as_array_mut) {
                for link in links {
                    let old = link["word_index"].as_u64().expect("owned word link") as usize;
                    link["word_index"] = json!(inverse[old]);
                }
            }
        }
    }

    pub(super) fn eligibility(
        trace: &mut Option<Self>,
        selection: &crate::hybrid::Selection,
        site: &'static str,
    ) -> bool {
        let (result, evidence) = selection.word_like_traced(trace.is_some());
        Self::push(trace, || {
            json!({"rule":"word_like", "site":site,
            "status":"evaluated", "evidence":evidence.expect("trace requested")})
        });
        result
    }
}

pub(super) fn ranks(ranked: &[(usize, f32)]) -> Value {
    json!(
        ranked
            .iter()
            .take(5)
            .map(|&(i, p)| json!({
                "word":crate::vocabulary::Word::from_index(i as u16).map(|w| w.as_str()), "probability":p,
            }))
            .collect::<Vec<_>>()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::Quad;
    use crate::phrase::*;
    use crate::recogniser::LineRead;
    use image::{GrayImage, Rgb, RgbImage};

    fn event(trace: &Option<DecisionTrace>, rule: &str) -> Value {
        trace
            .as_ref()
            .unwrap()
            .events
            .iter()
            .find(|e| e["rule"] == rule)
            .unwrap()
            .clone()
    }

    fn index(word: &str) -> usize {
        crate::vocabulary::Word::from_bip39(word).unwrap().index() as usize
    }

    fn rect(x: f32, y: f32, width: f32) -> Quad {
        Quad([(x, y), (x + width, y), (x + width, y + 20.), (x, y + 20.)])
    }

    fn without_trace(mut value: Value) -> Value {
        value.as_object_mut().unwrap().remove("decisions");
        value
    }

    #[test]
    fn label_trace_records_skipped_shape_and_real_owner_comparisons_without_changing_fit() {
        let (small, wide, word, far) = (
            rect(0., 0., 20.),
            rect(0., 0., 100.),
            rect(30., 0., 100.),
            rect(200., 0., 100.),
        );
        for (raw, label_quad, targets, is_label, owner) in [
            ("x", &small, vec![&word], false, false),
            ("7", &small, vec![&word], true, true),
            ("7", &wide, vec![&word], false, false),
            ("7.", &wide, vec![&word], true, false), // dotted bypasses role size, not ownership size
            ("7.", &small, vec![&word, &word], true, false), // ambiguous nearest owner
            ("7.", &small, vec![&far], true, false),
        ] {
            let mut candidates = vec![Candidate {
                quad: label_quad,
                read: Some(raw),
                token: None,
                word_like: true,
            }];
            candidates.extend(targets.iter().map(|q| Candidate {
                quad: q,
                read: Some("basket"),
                token: None,
                word_like: true,
            }));
            let plain = labels::label_evidence_traced(&candidates, false);
            let traced = labels::label_evidence_traced(&candidates, true);
            assert_eq!(plain.label_box, traced.label_box);
            assert_eq!(plain.evidence, traced.evidence);
            assert_eq!(plain.label_read, traced.label_read);
            let order: Vec<_> = (0..candidates.len()).collect();
            assert_eq!(
                plain.numbering(&order, &order),
                traced.numbering(&order, &order)
            );
            for (a, b) in plain.observations.iter().zip(&traced.observations) {
                assert_eq!(a.to_json(), without_trace(b.to_json()));
            }
            assert_eq!(traced.label_box[0], is_label, "{raw}");
            let trace = &traced.observations[0].decisions;
            let role = event(trace, "standalone_label");
            if raw == "x" {
                assert_eq!(role["size_check"]["reason"], "no_label_parse");
                assert_eq!(
                    event(trace, "label_owner")["reason"],
                    "not_standalone_label"
                );
                assert_eq!(
                    event(trace, "legacy_prefix_label")["reason"],
                    "first_character_not_digit_shaped"
                );
            } else if raw == "7" {
                assert_eq!(
                    role["size_check"]["width"],
                    if is_label { 20. } else { 100. }
                );
                assert_eq!(role["size_check"]["height"], 20.);
                assert_eq!(role["size_check"]["aspect_limit"], json!(2.2_f32));
                assert_eq!(role["size_check"]["accepted"], is_label);
            } else {
                assert_eq!(role["size_check"]["reason"], "dotted_token_bypasses_size");
            }
            if is_label {
                assert_eq!(event(trace, "label_owner")["assigned"], owner);
                let attempts: Vec<_> = trace
                    .as_ref()
                    .unwrap()
                    .events
                    .iter()
                    .filter(|e| e["rule"] == "label_owner_candidate")
                    .collect();
                assert_eq!(attempts[0]["reason"], "target_is_label");
                if label_quad == &wide {
                    assert_eq!(
                        attempts[1]["geometry"]["placement"]["reason"],
                        "label_shape_failed"
                    );
                } else {
                    let placement = &attempts[1]["geometry"]["placement"];
                    assert_eq!(placement["axis_angle_degrees"], 0.);
                    assert_eq!(placement["label_height"], 20.);
                    assert_eq!(placement["gap_factor"], 2.);
                    assert_eq!(
                        placement["word_leading_x"],
                        if targets[0] == &far { 200. } else { 30. }
                    );
                }
                if owner {
                    assert_eq!(
                        event(&traced.observations[1].decisions, "label_attachment")["authorizes_cut"],
                        false
                    );
                }
            }
        }
    }

    #[test]
    fn region_trace_records_exact_size_boundary_and_ineligible_skips() {
        for main in [9, 10] {
            let mut quads: Vec<_> = (0..main).map(|i| rect(0., i as f32 * 25., 100.)).collect();
            quads.extend((0..3).map(|i| rect(1000., i as f32 * 25., 100.)));
            quads.push(rect(2000., 0., 100.));
            let layout = crate::layout::PageLayout::new(&quads);
            let mut eligible = vec![true; quads.len()];
            eligible[main + 3] = false;
            let plain = crate::region::apart_in(&layout, &eligible);
            let mut traces = vec![Some(DecisionTrace::default()); quads.len()];
            let traced = crate::region::apart_in_observed(&layout, &eligible, |i, make| {
                DecisionTrace::push(&mut traces[i], make)
            });
            assert_eq!(plain, traced);
            assert_eq!(traced[main], main == 10);
            let e = event(&traces[main], "region_exclusion");
            assert_eq!(e["group_size"], 3);
            assert_eq!(e["largest_group_size"], main);
            assert_eq!(e["max_excluded_size"], 3);
            assert_eq!(e["size_multiplier"], 3);
            assert_eq!(e["height_scale"], 20.);
            assert_eq!(e["horizontal_padding"], 30.);
            assert_eq!(e["vertical_padding"], 20.);
            assert_eq!(e["group_bounds"].as_array().unwrap().len(), 3);
            assert_eq!(
                event(&traces[main + 3], "region_exclusion")["reason"],
                "ineligible_for_region"
            );
            // The normal path never evaluates the JSON closure.
            assert_eq!(
                plain,
                crate::region::apart_in_observed(
                    &layout,
                    &eligible,
                    |_, make| DecisionTrace::push(&mut None, make)
                )
            );
        }
    }

    #[test]
    fn cut_trace_preserves_branch_reads_geometry_matching_and_selection() {
        for (raw, token, cut, after_raw, after_prob, number, held, can_read, narrowed, calls) in [
            (
                "9.Hobby",
                Some("9."),
                Some(12),
                ". Hobby",
                0.8,
                None,
                false,
                true,
                true,
                1,
            ),
            (
                "9.Hobby",
                Some("9."),
                Some(12),
                ". Hobby",
                0.1,
                None,
                false,
                true,
                true,
                1,
            ),
            (
                "9.Hobby",
                Some("9."),
                None,
                ". Hobby",
                0.8,
                None,
                false,
                true,
                false,
                0,
            ),
            (
                "9.Hobby",
                None,
                Some(12),
                ". Hobby",
                0.8,
                None,
                false,
                true,
                false,
                0,
            ),
            (
                "9 Hobby",
                Some("9"),
                Some(12),
                "Hobby",
                0.8,
                Some(9),
                true,
                true,
                true,
                1,
            ),
            (
                "9 Hobby",
                Some("9"),
                Some(12),
                "Hobby",
                0.1,
                Some(9),
                true,
                true,
                false,
                1,
            ),
            (
                "9 Hobby",
                None,
                None,
                "Hobby",
                0.8,
                Some(9),
                true,
                true,
                false,
                0,
            ),
            (
                "1 WALK",
                None,
                Some(12),
                "WALK",
                0.8,
                None,
                false,
                true,
                true,
                1,
            ),
            (
                "1 WALK",
                None,
                Some(12),
                "ALK",
                0.8,
                None,
                false,
                true,
                false,
                1,
            ),
            (
                "9.QQ",
                Some("9."),
                Some(12),
                "air",
                0.8,
                None,
                false,
                false,
                false,
                0,
            ),
            ("x", None, Some(12), "air", 0.8, None, false, true, false, 0),
            ("x", None, Some(12), "air", 0.8, None, true, true, false, 0),
        ] {
            let run = |enabled: bool| {
                let mut p = Prepared {
                    cell: None,
                    writing: WritingFrame::Local,
                    quad: rect(0., 0., 80.),
                    crop: GrayImage::new(80, 20),
                    ranked: vec![(index("air"), 0.8)],
                    turned: false,
                    read: Some(LineRead {
                        text: raw.into(),
                        confidence: 0.7,
                    }),
                    cut,
                    token: token.map(|text| LineRead {
                        text: text.into(),
                        confidence: 0.6,
                    }),
                    decisions: enabled.then(DecisionTrace::default),
                };
                let mut labelled = label_evidence(&[Candidate {
                    quad: &p.quad,
                    read: Some(raw),
                    token,
                    word_like: true,
                }]);
                let mut owned = labelled.observations.remove(0);
                let mut read_count = 0;
                let outcome = cuts::apply(
                    &mut p,
                    &mut owned,
                    number,
                    held,
                    labelled.label_box[0],
                    labelled.evidence[0],
                    can_read,
                    CROP_MARGIN,
                    |p, cut| {
                        read_count += 1;
                        Ok(Past {
                            crop: image::imageops::crop_imm(
                                &p.crop,
                                cut,
                                0,
                                p.crop.width() - cut,
                                p.crop.height(),
                            )
                            .to_image(),
                            ranked: vec![(index("hobby"), after_prob)],
                            read: LineRead {
                                text: after_raw.into(),
                                confidence: 0.65,
                            },
                        })
                    },
                )
                .unwrap();
                assert_eq!(outcome.0, narrowed, "{raw} {after_prob} {token:?}");
                assert_eq!(read_count, calls, "{raw}");
                let word = finish_evidenced(
                    p,
                    &Calibration {
                        min_prob: 0.85,
                        min_margin: 0.75,
                        digest: String::new(),
                        rule: CANON_RULE.into(),
                    },
                    outcome.0,
                    outcome.1,
                    number,
                    owned,
                )
                .unwrap();
                (word, read_count)
            };
            let (plain, a) = run(false);
            let (traced, b) = run(true);
            assert_eq!(a, b);
            assert_eq!(
                (
                    &plain.quad,
                    &plain.crop,
                    &plain.ranked,
                    plain.pick(),
                    plain.stray
                ),
                (
                    &traced.quad,
                    &traced.crop,
                    &traced.ranked,
                    traced.pick(),
                    traced.stray
                )
            );
            assert_eq!(
                plain.evidence.to_json(),
                without_trace(traced.evidence.to_json())
            );
            let trace = &traced.evidence.decisions;
            if raw == "9.Hobby" && calls == 1 {
                let e = event(trace, "cut_confidence");
                assert_eq!(e["status"], "skipped");
                assert_eq!(event(trace, "dotted_cut_word")["accepted"], narrowed);
                let m = event(trace, "dotted_matching");
                assert_eq!(
                    m["matching"]["text"],
                    if narrowed { "Hobby" } else { "9.Hobby" }
                );
                assert_eq!(
                    m["matching"]["reason"],
                    if narrowed {
                        "CleanedWholeWord"
                    } else {
                        "WholeWordWithoutCut"
                    }
                );
                assert_eq!(
                    m["adopted_cut_literal"],
                    if narrowed {
                        json!(after_raw)
                    } else {
                        Value::Null
                    }
                );
            }
            if raw == "x" {
                assert_eq!(
                    event(trace, "cut_read")["reason"],
                    if held {
                        "held_sequence_without_number"
                    } else {
                        "no_leading_number"
                    }
                );
            }
            if !can_read {
                assert_eq!(event(trace, "cut_read")["reason"], "no_recogniser");
            }
            if raw == "1 WALK" {
                assert_eq!(
                    event(trace, "trim_literal")["near_list_check"]["reason"],
                    "exact_whole_authoritative"
                );
            }
        }
    }

    #[test]
    fn orientation_trace_does_not_change_crops_reads_or_work_budget() {
        let photo = RgbImage::from_fn(100, 100, |x, y| Rgb([x as u8, y as u8, 30]));
        for (quad, scores, turned) in [
            (
                Quad([(10., 10.), (25., 10.), (25., 70.), (10., 70.)]),
                vec![0.4, 0.8],
                true,
            ),
            (
                Quad([(10., 10.), (25., 10.), (25., 70.), (10., 70.)]),
                vec![0.5, 0.5],
                false,
            ),
            (
                Quad([(10., 10.), (25., 10.), (25., 70.), (10., 70.)]),
                vec![0.6, 0.2],
                false,
            ),
            (
                Quad([(10., 10.), (70., 10.), (70., 25.), (10., 25.)]),
                vec![0.8],
                false,
            ),
        ] {
            let run = |enabled| {
                let mut classified = Vec::new();
                let mut recognized = Vec::new();
                let mut budget = crate::read_budget::Budget::default();
                let prepared = super::super::prepare_with(
                    &photo,
                    &quad,
                    |crop| {
                        classified.push(crop.clone());
                        Ok(vec![(index("air"), scores[classified.len() - 1])])
                    },
                    Some(|crop: &GrayImage| {
                        recognized.push(crop.clone());
                        Ok(LineRead {
                            text: "x".into(),
                            confidence: 0.535,
                        })
                    }),
                    0.15,
                    false,
                    enabled,
                    Some(&mut budget),
                )
                .unwrap();
                (prepared, classified, recognized, budget)
            };
            let (plain, a, b, before) = run(false);
            let (traced, c, d, after) = run(true);
            assert!(plain.decisions.is_none());
            assert_eq!(plain.crop, traced.crop);
            assert_eq!(plain.quad, traced.quad);
            assert_eq!(plain.ranked, traced.ranked);
            assert_eq!(plain.turned, turned);
            assert_eq!(traced.turned, turned);
            assert_eq!(a, c);
            assert_eq!(b, d);
            assert_eq!(before, after);
            assert_eq!(a.len(), scores.len());
            assert_eq!(b.len(), 1);
            assert_eq!(b[0], traced.crop);
            let half = event(&traced.decisions, "half_turn");
            assert_eq!(half["adopted"], turned);
            assert_eq!(half["absolute_angle_threshold"], 45.0);
            assert_eq!(
                half["status"],
                if scores.len() == 2 {
                    "evaluated"
                } else {
                    "skipped"
                }
            );
            let orientation = event(&traced.decisions, "reader_orientation");
            let axis = crate::split::frame_of(&crate::split::with_margin(&quad, 0.15)).angle;
            assert_eq!(orientation["level_rotation_degrees_ccw"], json!(-axis));
            assert_eq!(
                orientation["net_rotation_degrees_ccw"],
                json!(-axis + if turned { 180.0 } else { 0.0 })
            );
            assert_eq!(event(&traced.decisions, "whole_ocr")["literal"], "x");
            assert_eq!(
                event(&traced.decisions, "token_ocr")["reason"],
                "labels_disabled"
            );
        }
    }

    #[test]
    fn token_attempts_and_skips_are_recorded_without_extra_calls() {
        let mut photo = RgbImage::from_pixel(100, 40, Rgb([255; 3]));
        for left in [8, 40, 55, 70] {
            for x in left..left + 7 {
                for y in 10..25 {
                    photo.put_pixel(x, y, Rgb([0; 3]));
                }
            }
        }
        let quad = Quad([(0., 0.), (100., 0.), (100., 40.), (0., 40.)]);
        for labels in [false, true] {
            let mut results = Vec::new();
            for enabled in [false, true] {
                let mut calls = Vec::new();
                let prepared = super::super::prepare_with(
                    &photo,
                    &quad,
                    |_| Ok(vec![(index("air"), 0.4)]),
                    Some(|crop: &GrayImage| {
                        calls.push(crop.clone());
                        Ok(LineRead {
                            text: if calls.len() == 1 { "7.air" } else { "7." }.into(),
                            confidence: 0.7,
                        })
                    }),
                    0.0,
                    labels,
                    enabled,
                    None,
                )
                .unwrap();
                if enabled {
                    let e = event(&prepared.decisions, "token_ocr");
                    if labels {
                        assert_eq!(e["status"], "evaluated");
                        assert_eq!(e["literal"], "7.");
                        assert_eq!(e["cut_x"], json!(prepared.cut.unwrap()));
                        assert_eq!(calls.len(), 2);
                    } else {
                        assert_eq!(e["status"], "skipped");
                        assert_eq!(calls.len(), 1);
                    }
                }
                results.push(calls);
            }
            assert_eq!(results[0], results[1]);
        }
        let no_ocr = super::super::prepare_with(
            &photo,
            &quad,
            |_| Ok(vec![(index("air"), 0.4)]),
            None::<fn(&GrayImage) -> Result<LineRead, String>>,
            0.0,
            true,
            true,
            None,
        )
        .unwrap();
        for rule in ["whole_ocr", "leading_gap", "token_ocr"] {
            assert_eq!(event(&no_ocr.decisions, rule)["reason"], "no_recogniser");
        }
        let blank = super::super::prepare_with(
            &RgbImage::from_pixel(100, 40, Rgb([255; 3])),
            &quad,
            |_| Ok(vec![(index("air"), 0.4)]),
            Some(|_: &GrayImage| {
                Ok(LineRead {
                    text: "".into(),
                    confidence: 0.0,
                })
            }),
            0.0,
            true,
            true,
            None,
        )
        .unwrap();
        assert_eq!(event(&blank.decisions, "token_ocr")["reason"], "no_gap");
    }

    #[test]
    fn word_eligibility_records_the_real_distance_witness_and_short_circuit() {
        let ranks = vec![
            (index("air"), 0.47385657),
            (index("put"), 0.10707156),
            (index("fox"), 0.08432491),
        ];
        for raw in [
            None,
            Some(""),
            Some("12."),
            Some("x"),
            Some("air"),
            Some("zzzzzzzz"),
        ] {
            let selected = crate::hybrid::select(raw, None, &ranks, 0.85, 0.75).unwrap();
            let original = selected.text().is_none_or(|text| {
                crate::hybrid::near_list(text, &crate::hybrid::CURRENT)
                    || selected.verdict() == crate::words::Verdict::Accept
            });
            let mut trace = Some(DecisionTrace::default());
            assert_eq!(
                DecisionTrace::eligibility(&mut None, &selected, "initial"),
                original
            );
            assert_eq!(
                DecisionTrace::eligibility(&mut trace, &selected, "initial"),
                original
            );
            let e = event(&trace, "word_like")["evidence"].clone();
            assert_eq!(e["word_like"], original);
            if raw == Some("x") {
                assert_eq!(
                    e["near_list"]["witness"],
                    json!({"word":"box","distance":3.0})
                );
                assert_eq!(e["near_list"]["budget"], 3.0);
                assert_eq!(e["selection"]["word"], "fox");
                assert_eq!(e["selection"]["confirmation"], "Disagreement");
                assert_eq!(e["confirmation_check"]["reason"], "near_list_sufficient");
            }
        }
        for letter in 'a'..='z' {
            let selected =
                crate::hybrid::select(Some(&letter.to_string()), None, &ranks, 0.85, 0.75).unwrap();
            let (kept, event) = selected.word_like_traced(true);
            assert!(kept);
            let distance = event.unwrap()["near_list"]["witness"]["distance"]
                .as_f64()
                .unwrap();
            assert!(distance <= 3.0);
        }
    }

    #[test]
    fn disabled_collection_is_lazy_and_finish_keeps_explicit_exclusions() {
        DecisionTrace::push(&mut None, || panic!("disabled trace evaluated payload"));
        for excluded in [false, true] {
            let mut observed = Vec::new();
            for enabled in [false, true] {
                let p = super::super::prepare_with(
                    &RgbImage::new(100, 40),
                    &Quad([(0., 0.), (100., 0.), (100., 40.), (0., 40.)]),
                    |_| Ok(vec![(index("air"), 0.9)]),
                    Some(|_: &GrayImage| {
                        Ok(LineRead {
                            text: "air".into(),
                            confidence: 0.7,
                        })
                    }),
                    0.0,
                    false,
                    enabled,
                    None,
                )
                .unwrap();
                let w = super::super::finish(
                    p,
                    &Calibration {
                        min_prob: 0.85,
                        min_margin: 0.75,
                        digest: String::new(),
                        rule: String::new(),
                    },
                    false,
                    excluded,
                    None,
                )
                .unwrap();
                assert_eq!(w.stray, excluded);
                if enabled {
                    assert_eq!(event(&w.evidence.decisions, "finish")["kept"], !excluded);
                    if excluded {
                        assert_eq!(
                            event(&w.evidence.decisions, "word_like")["reason"],
                            "already_excluded"
                        );
                    }
                } else {
                    assert!(w.evidence.to_json().get("decisions").is_none());
                }
                let pick = w.pick();
                observed.push((w.crop, w.quad, w.ranked, pick, w.stray));
            }
            assert_eq!(observed[0], observed[1]);
        }
    }
}
