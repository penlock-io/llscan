//! Aggregate all label evidence before fitting ordinals. No classifier gate on dots.
use super::*;
use crate::numbering::dotted;
use serde_json::json;

/// Specific contradictory observations, using the same zero-based box IDs as fit.
#[cfg(test)]
pub(super) fn conflict_detail(observations: &[WordEvidence]) -> Option<String> {
    describe_conflicts(observations.iter().enumerate().filter(|(_, o)| o.conflict))
}

fn describe_conflicts<'a>(
    observations: impl Iterator<Item = (usize, &'a WordEvidence)>,
) -> Option<String> {
    let conflicts: Vec<_> = observations
        .map(|(k, o)| {
            let mut numbers: Vec<_> = o.labels.iter().filter_map(|l| l.ordinal).collect();
            numbers.sort_unstable();
            numbers.dedup();
            format!(
                "box {k}: {}",
                numbers
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
        .collect();
    (!conflicts.is_empty()).then(|| {
        format!(
            "conflicting or duplicate observed labels ({})",
            conflicts.join("; ")
        )
    })
}

impl Labelled {
    /// Fit only eligible boxes through the existing sequence threshold. Local
    /// conflicts remain diagnostics below that threshold, not a new repair veto.
    pub fn numbering(&self, columns: &[usize], rows: &[usize]) -> Numbering {
        let given: Vec<_> = self
            .evidence
            .iter()
            .map(|e| e.map(Evidence::label))
            .collect();
        let sequence = fit(&given, columns, rows);
        if !sequence.has_held_sequence() {
            return sequence;
        }
        // Single-valued duplicates already reach fit's own check. Only competing
        // values within an eligible word require a further veto of a Held result
        // (fit may otherwise interpolate a number for its Shaped label). Labels
        // outside both traversals cannot poison a sequence via diagnostic flags.
        let contradictions = self.observations.iter().enumerate().filter(|(i, o)| {
            if !columns.contains(i) && !rows.contains(i) {
                return false;
            }
            let mut values = o.labels.iter().filter_map(|l| l.ordinal);
            values
                .next()
                .is_some_and(|first| values.any(|n| n != first))
        });
        match describe_conflicts(contradictions) {
            Some(detail) => Numbering::Inconsistent { detail },
            None => sequence,
        }
    }
}

fn observed(quad: &Quad, literal: &str, origin: &str, label: Label, dotted: bool) -> LabelRead {
    LabelRead {
        corners: quad.0,
        literal: literal.into(),
        origin: origin.into(),
        ordinal: match label {
            Label::Exact(n) => Some(n),
            Label::Shaped => None,
        },
        dotted,
        ambiguous: false,
    }
}

/// The shared label geometry owns both old numeric tokens and mandatory dots.
pub fn label_evidence(boxes: &[Candidate]) -> Labelled {
    label_evidence_traced(boxes, false)
}

pub(super) fn label_evidence_traced(boxes: &[Candidate], trace: bool) -> Labelled {
    label_evidence_in(boxes, trace, WritingFrame::Local)
}

pub(super) fn label_evidence_in(
    boxes: &[Candidate],
    trace: bool,
    writing: WritingFrame,
) -> Labelled {
    let n = boxes.len();
    let mut out = Labelled {
        evidence: vec![None; n],
        label_box: vec![false; n],
        label_read: vec![None; n],
        observations: vec![
            WordEvidence {
                decisions: trace.then(DecisionTrace::default),
                ..Default::default()
            };
            n
        ],
    };
    let digit_shaped = |c: char| c.is_ascii_digit() || crate::numbering::SHAPED.contains(c);
    for (k, b) in boxes.iter().enumerate() {
        let read = b.read.unwrap_or("");
        let dot = dotted::token(read);
        let legacy = dot.is_none().then(|| label_of(read));
        let standalone = dot.or_else(|| legacy.flatten());
        let size = (standalone.is_some() && dot.is_none())
            .then(|| crate::numbering::label_shape_evidence(b.quad, trace, writing));
        let accepted = standalone.filter(|_| dot.is_some() || size.as_ref().is_some_and(|s| s.0));
        DecisionTrace::push(&mut out.observations[k].decisions, || {
            json!({
            "rule":"standalone_label", "status":"evaluated", "literal":read,
            "dotted_token":dot.map(|l| format!("{l:?}")),
            "legacy_shaped_characters":crate::numbering::SHAPED,
            "legacy_parse":{"status":if legacy.is_some() {"evaluated"} else {"skipped"},
                "reason":if dot.is_some() {Some("dotted_token_authoritative")} else {None},
                "label":legacy.flatten().map(|l| format!("{l:?}"))},
            "size_check":size.as_ref().and_then(|s| s.1.clone()).unwrap_or_else(|| json!({"status":"skipped",
                "reason":if standalone.is_none() {"no_label_parse"} else {"dotted_token_bypasses_size"}})),
            "label_box":accepted.is_some()})
        });
        if let Some(label) = accepted {
            out.label_box[k] = true;
            out.label_read[k] = Some(read.into());
            out.observations[k].labels.push(observed(
                b.quad,
                read,
                "standalone",
                label,
                dot.is_some(),
            ));
            DecisionTrace::push(&mut out.observations[k].decisions, || {
                json!({"rule":"prefix_label",
                "status":"skipped", "reason":"standalone_whole_authoritative"})
            });
            DecisionTrace::push(&mut out.observations[k].decisions, || {
                json!({"rule":"legacy_prefix_label",
                "status":"skipped", "reason":"standalone_whole_authoritative"})
            });
            continue; // Whole label OCR owns its ordinal, never its partial token.
        }
        out.label_read[k] = b.token.map(str::to_owned);
        let prefix = dotted::prefix(read);
        let token = b.token.and_then(dotted::token);
        let mandatory = prefix.is_some() || token.is_some();
        if let Some(p) = prefix {
            out.observations[k]
                .labels
                .push(observed(b.quad, read, "prefix", p.label(), true));
            out.evidence[k] = Some(Evidence::Prefix(p.label()));
            out.label_read[k] = Some(read[..p.consumed].trim().into());
        }
        // Non-dotted token can corroborate a whole dotted head (9 / 9.Hobby).
        let corroborated = b.token.filter(|t| {
            prefix.is_some_and(|p| !p.head.is_empty() && t.trim().eq_ignore_ascii_case(p.head))
        });
        let token_label =
            token.or_else(|| corroborated.map(|s| label_of(s).unwrap_or(Label::Shaped)));
        DecisionTrace::push(&mut out.observations[k].decisions, || {
            json!({"rule":"prefix_label", "status":"evaluated",
            "whole_literal":read, "token_literal":b.token,
            "dotted_prefix":prefix.map(|p| json!({"head":p.head,"remainder":p.remainder,"consumed_bytes":p.consumed,
                "label":format!("{:?}",p.label())})),
            "dotted_token":token.map(|l| format!("{l:?}")), "corroborated_token":corroborated,
            "token_label":token_label.map(|l| format!("{l:?}")), "mandatory":mandatory})
        });
        if let Some(label) = token_label {
            out.observations[k].labels.push(observed(
                b.quad,
                b.token.unwrap(),
                "token",
                label,
                token.is_some(),
            ));
            out.evidence[k] = Some(Evidence::Token(label));
            out.label_read[k] = b.token.map(str::to_owned);
        }
        let digit_head =
            (!mandatory && b.word_like).then(|| read.chars().next().is_some_and(digit_shaped));
        if digit_head == Some(true) {
            let legacy = b.token.and_then(label_of).map(Evidence::Token).or_else(|| {
                past_label(read).map(|rest| {
                    Evidence::Prefix(
                        label_of(&read[..read.len() - rest.len()]).unwrap_or(Label::Shaped),
                    )
                })
            });
            DecisionTrace::push(&mut out.observations[k].decisions, || {
                json!({"rule":"legacy_prefix_label", "status":"evaluated",
                "word_like":b.word_like, "first_character":read.chars().next(), "digit_shaped_head":true,
                "allowed_shaped_characters":crate::numbering::SHAPED, "ascii_digits_allowed":true,
                "token_literal":b.token, "evidence":legacy.map(|e| format!("{e:?}"))})
            });
            if let Some(e) = legacy {
                let literal = if e.by_ink() { b.token.unwrap() } else { read };
                out.observations[k].labels.push(observed(
                    b.quad,
                    literal,
                    if e.by_ink() { "token" } else { "prefix" },
                    e.label(),
                    false,
                ));
                out.evidence[k] = Some(e);
            }
        } else {
            DecisionTrace::push(&mut out.observations[k].decisions, || {
                json!({"rule":"legacy_prefix_label", "status":"skipped",
                "reason":if mandatory {"mandatory_dotted_evidence"} else if !b.word_like {"not_word_like"} else {"first_character_not_digit_shaped"},
                "word_like":b.word_like, "first_character":read.chars().next(), "digit_shaped_head":digit_head,
                "allowed_shaped_characters":crate::numbering::SHAPED, "ascii_digits_allowed":true})
            });
        }
    }
    for k in 0..n {
        if !out.label_box[k] {
            DecisionTrace::push(
                &mut out.observations[k].decisions,
                || json!({"rule":"label_owner", "status":"skipped", "reason":"not_standalone_label"}),
            );
            continue;
        }
        // Exclude label-shaped weak scraps, not plausible word-width targets
        // whose contaminated OCR or classifier cannot yet read a word.
        let mut owners: Vec<_> = boxes
            .iter()
            .enumerate()
            .filter_map(|(i, b)| {
                let size = (!out.label_box[i] && !b.word_like).then(|| crate::numbering::label_shape_evidence(b.quad, trace, writing));
                let eligible = !out.label_box[i] && (b.word_like || size.as_ref().is_some_and(|s| !s.0));
                let (x, geometry) = if eligible { crate::numbering::leading_label_evidence(
                    boxes[k].quad,
                    b.quad,
                    writing.angle_or(0.0),
                    writing.frame_of(boxes[k].quad).h * 1e-5,
                    trace,
                    writing,
                ) } else { (None, None) };
                DecisionTrace::push(&mut out.observations[k].decisions, || json!({"rule":"label_owner_candidate",
                    "status":if eligible {"evaluated"} else {"skipped"},
                    "reason":if out.label_box[i] {Some("target_is_label")} else if !eligible {Some("weak_label_sized_target")} else {None},
                    "target_quad":b.quad.0, "word_like":b.word_like,
                    "target_size_check":size.and_then(|s| s.1).unwrap_or_else(|| json!({"status":"skipped",
                        "reason":if out.label_box[i] {"target_is_label"} else {"word_like_sufficient"}})),
                    "geometry":geometry, "eligible_owner":x.is_some()}));
                x.map(|x| (i, x))
            })
            .collect();
        owners.sort_by(|a, b| a.1.total_cmp(&b.1));
        let Some(&(target, x)) = owners.first() else {
            DecisionTrace::push(
                &mut out.observations[k].decisions,
                || json!({"rule":"label_owner", "status":"evaluated", "reason":"no_eligible_owner", "assigned":false}),
            );
            continue;
        };
        let epsilon = writing.frame_of(boxes[k].quad).h * 1e-5;
        let tied = owners
            .get(1)
            .is_some_and(|&(_, other)| (other - x).abs() <= epsilon);
        DecisionTrace::push(&mut out.observations[k].decisions, || {
            json!({"rule":"label_owner", "status":"evaluated",
            "owner_quad":if tied {None} else {Some(boxes[target].quad.0)}, "assigned":!tied,
            "reason":if tied {"ambiguous_nearest_owner"} else {"unique_nearest_owner"},
            "nearest_x":x,"runner_up_x":owners.get(1).map(|o| o.1),"tie_epsilon":epsilon,
            "comparison":"abs(runner_up_x - nearest_x) <= tie_epsilon"})
        });
        if tied {
            out.observations[k].labels[0].ambiguous = true;
            continue;
        }
        let mut observation = out.observations[k].labels[0].clone();
        observation.origin = "beside".into();
        out.label_read[target] = Some(observation.literal.clone());
        out.observations[target].labels.push(observation);
        DecisionTrace::push(&mut out.observations[target].decisions, || {
            json!({"rule":"label_attachment",
            "status":"evaluated", "label_quad":boxes[k].quad.0, "literal":boxes[k].read,
            "reason":"unique_nearest_owner", "authorizes_cut":false})
        });
        // Beside labels protect their target, but never authorize a cut of it.
        if !out.observations[target]
            .labels
            .iter()
            .any(|l| l.dotted && l.origin != "beside")
        {
            out.evidence[target] = Some(Evidence::Beside(Label::Shaped));
        }
    }
    // Resolve independent observations, without guessing ordinals. Known +
    // unknown is not a conflict; two different known values are.
    let mut ordinals = vec![None; n];
    for (k, ordinal) in ordinals.iter_mut().enumerate() {
        if out.label_box[k] {
            continue;
        }
        let mut values: Vec<_> = out.observations[k]
            .labels
            .iter()
            .filter_map(|l| l.ordinal)
            .collect();
        values.sort_unstable();
        values.dedup();
        out.observations[k].conflict = values.len() > 1;
        *ordinal = (values.len() == 1).then(|| values[0]);
    }
    // Duplicate ordinals are explicit even below fit's MIN_RUN. Do not confuse
    // a standalone label and its attached copy with two word ordinals.
    for k in 0..n {
        if let Some(value) = ordinals[k] {
            if ordinals
                .iter()
                .enumerate()
                .any(|(j, n)| j != k && *n == Some(value))
            {
                out.observations[k].conflict = true;
            }
        }
        if let Some(e) = out.evidence[k] {
            // Keep single-valued duplicates exact: fit must still see them once
            // its anchor threshold is met. Only competing values within this
            // word have no unique ordinal and remain Shaped.
            let label = ordinals[k].map_or(Label::Shaped, Label::Exact);
            out.evidence[k] = Some(match e {
                Evidence::Token(_) => Evidence::Token(label),
                Evidence::Prefix(_) => Evidence::Prefix(label),
                Evidence::Beside(_) => Evidence::Beside(label),
                Evidence::Cell(_) => Evidence::Cell(label),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    fn quad(x: f32, y: f32, width: f32) -> Quad {
        Quad([(x, y), (x + width, y), (x + width, y + 30.), (x, y + 30.)])
    }
    fn candidate<'a>(quad: &'a Quad, read: &'a str, token: &'a str) -> Candidate<'a> {
        Candidate {
            quad,
            read: Some(read),
            token: Some(token),
            word_like: false,
        }
    }

    #[test]
    fn mandatory_labels_ignore_word_confidence_and_whole_read_first_character() {
        let q = quad(0., 0., 150.);
        for (read, token, ordinal) in [
            ("G.corn", "G", None),
            ("s.riot", "S.", None),
            (".5tate", "7.", Some(7)),
            ("1.dice", "?", Some(1)),
            ("", "8.", Some(8)),
        ] {
            let labelled = label_evidence(&[candidate(&q, read, token)]);
            assert!(!labelled.label_box[0], "{read}");
            assert!(labelled.evidence[0].is_some(), "{read}");
            let observations = &labelled.observations[0];
            assert!(observations.labels.iter().any(|l| l.dotted));
            assert!(!observations.conflict);
            assert_eq!(
                labelled.evidence[0].unwrap().label(),
                ordinal.map_or(Label::Shaped, Label::Exact)
            );
        }
    }

    #[test]
    fn standalone_whole_is_authoritative_and_attaches_to_a_weak_word() {
        let (label, word) = (quad(0., 0., 25.), quad(35., 0., 150.));
        for raw in ["H.", "."] {
            let l = label_evidence(&[candidate(&label, raw, "1"), candidate(&word, "???", "")]);
            assert_eq!(l.label_box, [true, false]);
            assert_eq!(l.evidence[1], Some(Evidence::Beside(Label::Shaped)));
            assert_eq!(l.observations[1].labels[0].literal, raw);
            assert_eq!(l.observations[1].labels[0].corners, label.0);
            assert!(l.observations[1].labels[0].ordinal.is_none());
        }
    }

    #[test]
    fn conflicts_and_duplicates_survive_below_sequence_threshold() {
        let a = quad(0., 0., 150.);
        let b = quad(300., 0., 150.);
        let l = label_evidence(&[candidate(&a, "1.dice", "2."), candidate(&b, "G.corn", "6.")]);
        assert!(l.observations[0].conflict);
        assert_eq!(l.observations[0].labels.len(), 2);
        assert!(!l.observations[1].conflict); // unknown G plus exact 6
        assert_eq!(
            conflict_detail(&l.observations).as_deref(),
            Some("conflicting or duplicate observed labels (box 0: 1, 2)")
        );
        assert_eq!(l.numbering(&[0, 1], &[0, 1]), Numbering::None);
        let l = label_evidence(&[candidate(&a, "1.dice", "1."), candidate(&b, "1.corn", "1.")]);
        assert!(l.observations.iter().all(|o| o.conflict));
        assert_eq!(
            conflict_detail(&l.observations).as_deref(),
            Some("conflicting or duplicate observed labels (box 0: 1; box 1: 1)")
        );
        let sequence = l.numbering(&[0, 1], &[0, 1]);
        assert_eq!(sequence, Numbering::None);
        assert!(sequence.permits_word_repair());
        assert_eq!(
            dotted::ListMode::from_evidence(true, &sequence),
            dotted::ListMode::Numbered
        );
    }

    fn run(values: &[(u32, u32)]) -> Labelled {
        let quads: Vec<_> = (0..values.len())
            .map(|i| quad(0., i as f32 * 60., 150.))
            .collect();
        let texts: Vec<_> = values
            .iter()
            .map(|(whole, token)| (format!("{whole}.word"), format!("{token}.")))
            .collect();
        let boxes: Vec<_> = texts
            .iter()
            .enumerate()
            .map(|(i, (whole, token))| Candidate {
                quad: &quads[i],
                read: Some(whole),
                token: Some(token),
                word_like: true,
            })
            .collect();
        label_evidence(&boxes)
    }

    #[test]
    fn sufficiently_anchored_duplicates_still_reach_the_existing_fit_check() {
        let mut values: Vec<_> = (1..=8).map(|i| (i, i)).collect();
        values[7] = (7, 7);
        let l = run(&values);
        assert!(l.observations[6].conflict && l.observations[7].conflict);
        assert_eq!(l.evidence[6].unwrap().label(), Label::Exact(7));
        assert_eq!(l.evidence[7].unwrap().label(), Label::Exact(7));
        let order: Vec<_> = (0..8).collect();
        assert_eq!(
            l.numbering(&order, &order),
            Numbering::Inconsistent {
                detail: "two boxes are numbered 7 (boxes 6 and 7)".into(),
            }
        );
    }

    #[test]
    fn competing_values_cannot_be_published_as_an_interpolated_held_number() {
        let mut values: Vec<_> = (1..=8).map(|i| (i, i)).collect();
        values.push((9, 10));
        let l = run(&values);
        let order: Vec<_> = (0..9).collect();
        let given: Vec<_> = l.evidence.iter().map(|e| e.map(Evidence::label)).collect();
        assert_eq!(given[8], Some(Label::Shaped));
        assert!(fit(&given, &order, &order).has_held_sequence());
        let result = l.numbering(&order, &order);
        assert_eq!(
            result,
            Numbering::Inconsistent {
                detail: "conflicting or duplicate observed labels (box 8: 9, 10)".into(),
            }
        );
        assert!(!result.permits_word_repair());
        // Seven unambiguous anchors plus a competing observation are not eight
        // exact anchors. The eligibility rule still owns that distinction.
        values.remove(7);
        let l = run(&values);
        let order: Vec<_> = (0..8).collect();
        assert_eq!(l.numbering(&order, &order), Numbering::None);
        assert!(l.observations[7].conflict);
    }

    #[test]
    fn out_of_region_conflicts_cannot_poison_an_eligible_sequence() {
        for extra in [(1, 1), (9, 10)] {
            let mut values: Vec<_> = (1..=8).map(|i| (i, i)).collect();
            values.push(extra);
            let l = run(&values);
            assert!(l.observations[8].conflict);
            let order: Vec<_> = (0..8).collect(); // box 8 is outside both orders
            assert!(l.numbering(&order, &order).has_held_sequence());
        }
    }

    #[test]
    fn weak_undotted_letter_tokens_remain_diagnostics_not_a_global_veto() {
        let a = quad(0., 0., 150.);
        let b = quad(0., 60., 150.);
        let mut a = candidate(&a, "secet", "1");
        a.word_like = true;
        let mut b = candidate(&b, "sutfer", "1");
        b.word_like = true;
        let l = label_evidence(&[a, b]);
        assert!(l.observations.iter().all(|o| o.conflict));
        assert!(
            l.observations
                .iter()
                .flat_map(|o| &o.labels)
                .all(|l| !l.dotted)
        );
        let sequence = l.numbering(&[0, 1], &[0, 1]);
        assert_eq!(sequence, Numbering::None);
        assert!(sequence.permits_word_repair());
    }

    #[test]
    fn multiple_labels_do_not_overwrite_and_equal_geometric_owners_are_explicit() {
        let a = quad(0., 0., 20.);
        let b = quad(22., 0., 20.);
        let w = quad(50., 0., 150.);
        let l = label_evidence(&[
            candidate(&a, "1.", ""),
            candidate(&b, "2.", ""),
            candidate(&w, "???", ""),
        ]);
        assert_eq!(l.observations[2].labels.len(), 2);
        assert!(l.observations[2].conflict);
        let l = label_evidence(&[
            candidate(&a, "H.", ""),
            candidate(&w, "???", ""),
            candidate(&w, "???", ""),
        ]);
        assert!(l.observations[0].labels[0].ambiguous);
        assert!(l.evidence.iter().all(Option::is_none));
    }

    #[test]
    fn ordinary_words_do_not_gain_mandatory_evidence() {
        let q = quad(0., 0., 150.);
        for read in ["word.", "can.cel", "5ILK", "word!"] {
            let l = label_evidence(&[candidate(&q, read, "")]);
            assert!(l.observations[0].labels.is_empty(), "{read}");
        }
    }
}
