//! Label-cut geometry and crop-owned evidence, with actual decision diagnostics.
use super::*;
use serde_json::json;

/// A dotted label owns its detached punctuation as well as its digit strokes.
/// Propose just one cut, before any token OCR: discard reading-margin pixels
/// from the gap mask, then pass small detached marks following the first span.
/// A multi-character head additionally needs a separated low dot delimiting
/// the whole prefix; a gap between its digits is not a word boundary.
/// The actual readers still see the original pixels, including their margins.
pub(super) fn dotted_offset(
    crop: &GrayImage,
    content: crate::split::Frame,
    whole: &str,
    trace: &mut Option<DecisionTrace>,
) -> Option<u32> {
    let mut ink = binarise(crop);
    for (y, row) in ink.iter_mut().enumerate() {
        for (x, value) in row.iter_mut().enumerate() {
            if (x as f32 + 0.5 - crop.width() as f32 / 2.).abs() > content.w / 2.
                || (y as f32 + 0.5 - crop.height() as f32 / 2.).abs() > content.h / 2.
            {
                *value = false;
            }
        }
    }
    strip_rules(&mut ink);
    let spans = split_columns(&ink, TRIM);
    let small = |&(x0, x1): &(usize, usize)| {
        let rows: Vec<_> = ink
            .iter()
            .enumerate()
            .filter(|(_, row)| row[x0..x1].iter().any(|&v| v))
            .map(|(y, _)| y)
            .collect();
        let height = rows
            .last()
            .zip(rows.first())
            .map_or(0, |(last, first)| last - first + 1);
        ((x1 - x0) as f32) < crate::split::SMALL_INK_RATIO * content.h
            && (height as f32) < crate::split::SMALL_INK_RATIO * content.h
    };
    let word_span = spans
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, span)| !small(span));
    let initial_candidate = word_span.map(|(_, &(x, _))| x as u32);
    let mut candidate = initial_candidate;
    let multi_character_head =
        crate::numbering::dotted::prefix(whole).is_some_and(|p| p.head.chars().count() > 1);
    let content_left = (crop.width() as f32 - content.w) / 2.;
    // TRIM may join a digit to its dot, hiding the label terminator inside a
    // span. Inspect every non-overlapping component gap before applying its
    // median-gap grouping; zero here means no grouping, not a tuned threshold.
    let components = if multi_character_head {
        split_columns(
            &ink,
            Split {
                factor: 0.,
                gap: 0.,
                ..TRIM
            },
        )
    } else {
        Vec::new()
    };
    let delimiter = components.iter().enumerate().skip(1).find(|(i, span)| {
        if !small(span) {
            return false;
        }
        let (x0, x1) = **span;
        if x1 as f32 - content_left >= crate::numbering::LABEL_ASPECT_LIMIT * content.h
            || (x0 - components[i - 1].1) as f32 <= TRIM.gap * content.h
        {
            return false;
        }
        let rows: Vec<_> = ink
            .iter()
            .enumerate()
            .filter(|(_, row)| row[x0..x1].iter().any(|&v| v))
            .map(|(y, _)| y)
            .collect();
        // A label's full stop is below the line centre. An i-dot above a
        // word is not evidence to consume that word's initial strokes.
        rows.first()
            .zip(rows.last())
            .is_some_and(|(first, last)| first + last + 1 > crop.height() as usize)
    });
    if multi_character_head && candidate.is_some() {
        // The whole read already says there is more than one label character.
        // Without a separated low dot we cannot certify a gap between digits
        // as label-free. Refuse rather than ask suffix OCR to hide the residue.
        candidate = delimiter.and_then(|(dot_index, &(_, dot_end))| {
            if candidate.unwrap() as usize >= dot_end {
                candidate
            } else {
                components[dot_index + 1..]
                    .iter()
                    .find(|s| !small(s))
                    .map(|s| s.0 as u32)
            }
        });
    }
    let cut = candidate.filter(|&x| {
        let width = x as f32 - content_left;
        width > 0.
            && width < crate::numbering::LABEL_ASPECT_LIMIT * content.h
            && x + 2 < crop.width()
    });
    DecisionTrace::push(trace, || {
        json!({
            "rule":"dotted_label_gap", "status":"evaluated",
            "content_width":content.w, "content_height":content.h,
            "crop_size":[crop.width(), crop.height()], "margin_ink_excluded":true,
            "spans":spans, "initial_word_span":word_span.map(|(i, _)| i),
            "initial_candidate_cut_x":initial_candidate,
            "component_spans":components, "label_delimiter_span":delimiter.map(|(i, _)| i),
            "multi_character_head":multi_character_head,
            "delimiter_min_gap":TRIM.gap * content.h,
            "delimiter_position":"below_line_centre",
            "small_mark_ratio":crate::split::SMALL_INK_RATIO,
            "label_aspect_limit":crate::numbering::LABEL_ASPECT_LIMIT,
            "candidate_cut_x":candidate, "cut_x":cut,
            "reason":if cut.is_some() && multi_character_head {"after_multi_character_label_delimiter"}
                else if cut.is_some() {"after_label_and_detached_marks"}
                else if initial_candidate.is_some() && multi_character_head && delimiter.is_none() {"unresolved_multi_character_label"}
                else if candidate.is_some() {"outside_label_extent"} else {"no_word_span"},
        })
    });
    cut
}

fn skipped(p: &mut Prepared, reason: &str) {
    DecisionTrace::push(
        &mut p.decisions,
        || json!({"rule":"cut_read", "status":"skipped", "reason":reason}),
    );
}

fn confidence_accepts(p: &mut Prepared, after: &Past) -> bool {
    let before_top = p.ranked.first().map_or(0.0, |r| r.1);
    let after_top = after.ranked.first().map_or(0.0, |r| r.1);
    let accepted = after_top + CUT_TOLERANCE >= before_top;
    DecisionTrace::push(&mut p.decisions, || {
        json!({"rule":"cut_confidence", "status":"evaluated",
        "before_top":before_top, "after_top":after_top, "tolerance":CUT_TOLERANCE,
        "comparison":"after_top + tolerance >= before_top", "accepted":accepted})
    });
    accepted
}

/// A corroborated dotted label plus a literal whole word in the suffix is
/// stronger cut evidence than classifier confidence on a label-bearing crop.
/// Exact words already present in the whole or cleaned remainder must survive.
fn dotted_accepts(p: &mut Prepared, whole: &str, after: &Past) -> bool {
    let literal = after.read.text.trim();
    let suffix = literal
        .strip_prefix('.')
        .map(str::trim_start)
        .unwrap_or(literal);
    let usable = !suffix.is_empty()
        && after.read.confidence.is_finite()
        && (0. ..=1.).contains(&after.read.confidence);
    let word = usable.then(|| Word::from_bip39(suffix)).flatten();
    let normalized = normalise(whole);
    let original_word = Word::from_bip39(&normalized);
    let remainder_word =
        crate::numbering::dotted::prefix(whole).and_then(|p| Word::from_bip39(p.remainder.trim()));
    let preserved = [original_word, remainder_word]
        .into_iter()
        .flatten()
        .all(|w| Some(w) == word);
    let decision = if !usable {
        Some((false, "unusable_word_only_read"))
    } else if !preserved {
        Some((false, "cut_changes_existing_exact_word"))
    } else if word.is_some() {
        Some((true, "corroborated_label_and_exact_word"))
    } else {
        None
    };
    DecisionTrace::push(&mut p.decisions, || {
        json!({
            "rule":"dotted_cut_word", "status":"evaluated", "whole_literal":whole,
            "cut_literal":after.read.text, "word_witness":word.map(Word::as_str),
            "whole_word":original_word.map(Word::as_str), "remainder_word":remainder_word.map(Word::as_str),
            "accepted":decision.map(|d| d.0),
            "reason":decision.map_or("no_exact_word_use_existing_confidence_rule", |d| d.1),
        })
    });
    if let Some((accepted, _)) = decision {
        DecisionTrace::push(&mut p.decisions, || {
            json!({"rule":"cut_confidence",
            "status":"skipped", "reason":"literal_word_guard_decided"})
        });
        accepted
    } else {
        confidence_accepts(p, after)
    }
}

/// `read_past` owns the same crop/classifier/OCR work as before. The closure is
/// also the fake-reader seam: tracing must never invoke it to fill a history gap.
#[allow(clippy::too_many_arguments)]
pub(super) fn apply(
    p: &mut Prepared,
    owned: &mut WordEvidence,
    number: Option<u32>,
    held: bool,
    label_box: bool,
    evidence: Option<Evidence>,
    can_read: bool,
    margin: f32,
    mut read_past: impl FnMut(&mut Prepared, u32) -> Result<Past, String>,
) -> Result<(bool, bool), String> {
    let (mut narrowed_box, mut trim_stray) = (false, false);
    let (Some(read), true) = (p.read.clone(), can_read) else {
        skipped(
            p,
            if !can_read {
                "no_recogniser"
            } else {
                "no_whole_read"
            },
        );
        return Ok((false, false));
    };
    let dotted = owned
        .labels
        .iter()
        .any(|l| l.dotted && l.origin != "beside");
    DecisionTrace::push(&mut p.decisions, || {
        json!({"rule":"cut_branch", "status":"evaluated",
        "standalone_label":label_box, "own_dotted_evidence":dotted, "held_number":number,
        "held_sequence":held, "whole_literal":read.text, "token_literal":p.token.as_ref().map(|t| &t.text),
        "gap_cut_x":p.cut})
    });
    if label_box {
        skipped(p, "standalone_label");
    } else if dotted {
        // A whole prefix alone does not authorize a geometric cut.
        let corroborated = p
            .cut
            .map(|_| evidence::dotted_cut(&read.text, p.token.as_ref().map(|t| t.text.as_str())));
        DecisionTrace::push(&mut p.decisions, || {
            json!({"rule":"dotted_cut_corroboration",
            "status":if corroborated.is_some() {"evaluated"} else {"skipped"},
            "reason":if p.cut.is_none() {Some("no_gap")} else {None}, "accepted":corroborated})
        });
        if let Some(cut) = p.cut.filter(|_| corroborated == Some(true)) {
            let after = read_past(p, cut)?;
            if dotted_accepts(p, &read.text, &after) {
                owned.original = Some(p.adopt(cut, after, margin));
                narrowed_box = true;
            }
        } else {
            skipped(
                p,
                if p.cut.is_none() {
                    "no_gap"
                } else {
                    "token_does_not_corroborate_dot"
                },
            );
        }
        owned.matching = evidence::dotted_matching_traced(
            &read.text,
            narrowed_box.then(|| p.read.as_ref().unwrap().text.as_str()),
            &mut p.decisions,
        );
    } else if number.is_some() {
        // A held run allows an ink-supported cut, but still retains the
        // classifier-confidence veto. Otherwise strip only the matching text.
        let by_ink = evidence.is_some_and(Evidence::by_ink);
        DecisionTrace::push(&mut p.decisions, || {
            json!({"rule":"numbered_cut_evidence", "status":"evaluated",
            "evidence":evidence.map(|e| format!("{e:?}")), "by_ink":by_ink, "gap_cut_x":p.cut})
        });
        if let (true, Some(cut)) = (by_ink, p.cut) {
            let after = read_past(p, cut)?;
            if confidence_accepts(p, &after) {
                owned.original = Some(p.adopt(cut, after, margin));
                narrowed_box = true;
            }
        } else {
            skipped(
                p,
                if !by_ink {
                    "no_ink_label_evidence"
                } else {
                    "no_gap"
                },
            );
        }
        let rest = (!narrowed_box).then(|| past_label(&read.text));
        DecisionTrace::push(&mut p.decisions, || {
            json!({"rule":"numbered_matching", "status":if narrowed_box {"skipped"} else {"evaluated"},
            "reason":if narrowed_box {Some("cut_adopted")} else {None}, "whole_literal":read.text,
            "remainder":rest.flatten(), "removed_prefix_bytes":rest.flatten().map(|r| read.text.len()-r.len())})
        });
        if let Some(rest) = rest.flatten() {
            owned.matching = Some(MatchingRead {
                text: rest.to_owned(),
                original: false,
                removed_prefix_bytes: read.text.len() - rest.len(),
            });
        }
    } else if !held && leading_number(&read.text) {
        // No trusted run: the crop's own literal evidence decides.
        if let Some(cut) = p.cut {
            let after = read_past(p, cut)?;
            let decision =
                trim_decision_traced(&read.text, Some(&after.read.text), &mut p.decisions);
            if decision.narrowed {
                let original = decision.raw != after.read.text;
                owned.original = Some(p.adopt(cut, after, margin));
                narrowed_box = true;
                owned.matching = Some(MatchingRead {
                    text: decision.raw,
                    original,
                    removed_prefix_bytes: 0,
                });
            }
            trim_stray = decision.stray;
        } else {
            skipped(p, "no_gap");
        }
    } else {
        skipped(
            p,
            if held {
                "held_sequence_without_number"
            } else {
                "no_leading_number"
            },
        );
    }
    DecisionTrace::push(&mut p.decisions, || {
        json!({"rule":"cut_result", "status":"evaluated",
        "narrowed":narrowed_box, "trim_stray":trim_stray})
    });
    Ok((narrowed_box, trim_stray))
}

fn near(raw: &str, site: &str, trace: &mut Option<DecisionTrace>) -> bool {
    let result = crate::hybrid::near_list_evidence(raw, &CURRENT);
    DecisionTrace::push(trace, || {
        json!({"rule":"trim_near_list", "status":"evaluated", "site":site,
        "literal":raw, "normalized":result.normalized, "within_budget":result.within_budget,
        "witness":result.witness.map(|(w,d)| json!({"word":w.as_str(),"distance":d})),
        "budget":CURRENT.budget, "weights":{"substitute":CURRENT.substitute,"delete":CURRENT.delete,
            "insert":CURRENT.insert,"first":CURRENT.first}})
    });
    result.within_budget
}

pub(super) fn trim_decision_traced(
    raw: &str,
    raw2: Option<&str>,
    trace: &mut Option<DecisionTrace>,
) -> Trim {
    let whole = |stray| Trim {
        narrowed: false,
        raw: raw.to_owned(),
        stray,
    };
    let Some(raw2) = raw2 else {
        DecisionTrace::push(
            trace,
            || json!({"rule":"trim_literal", "status":"skipped", "reason":"no_cut_read"}),
        );
        return whole(false);
    };
    let read = normalise(raw);
    let exact = Word::from_bip39(&read).is_some();
    let same = exact.then(|| normalise(raw2) == read);
    DecisionTrace::push(trace, || {
        json!({"rule":"trim_whole_word", "status":"evaluated",
        "whole_literal":raw, "cut_literal":raw2, "normalized_whole":read,
        "exact_word":exact, "same_word_check":{"status":if exact {"evaluated"} else {"skipped"},
            "reason":if exact {None} else {Some("whole_not_exact")}, "equal":same}})
    });
    if let Some(same) = same {
        // A complete word in the original (e.g. 1 WALK) must survive the cut.
        DecisionTrace::push(trace, || {
            json!({"rule":"trim_literal", "status":"evaluated",
            "reason":"preserve_exact_whole_word", "narrowed":same, "stray":false,
            "near_list_check":{"status":"skipped","reason":"exact_whole_authoritative"}})
        });
        return if same {
            Trim {
                narrowed: true,
                raw: raw.to_owned(),
                stray: false,
            }
        } else {
            whole(false)
        };
    }
    let leading = leading_number(raw2);
    if leading {
        DecisionTrace::push(trace, || {
            json!({"rule":"trim_near_list", "site":"cut", "status":"skipped",
            "reason":"cut_still_has_leading_number", "literal":raw2})
        });
    }
    if !leading && near(raw2, "cut", trace) {
        DecisionTrace::push(trace, || {
            json!({"rule":"trim_literal", "status":"evaluated",
            "reason":"cut_is_near_word_without_leading_number", "narrowed":true,"stray":false,
            "whole_near_list_check":{"status":"skipped","reason":"cut_accepted"}})
        });
        return Trim {
            narrowed: true,
            raw: raw2.to_owned(),
            stray: false,
        };
    }
    let stray = !near(raw, "whole", trace);
    DecisionTrace::push(trace, || {
        json!({"rule":"trim_literal", "status":"evaluated",
        "reason":"cut_rejected_keep_depends_on_whole", "narrowed":false, "stray":stray})
    });
    whole(stray)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gap_proposal_stops_on_a_real_stroke_and_rejects_only_marks_or_wide_prefixes() {
        let content = crate::split::Frame {
            cx: 0.,
            cy: 0.,
            w: 200.,
            h: 40.,
            angle: 0.,
        };
        let crop = |word_x: u32, word_h: u32| {
            GrayImage::from_fn(212, 52, |x, y| {
                let label = (12..22).contains(&x) && (10..42).contains(&y);
                let dot = (30..33).contains(&x) && (37..40).contains(&y);
                let stroke = (word_x..word_x + 5).contains(&x) && (10..10 + word_h).contains(&y);
                image::Luma([if label || dot || stroke { 20 } else { 235 }])
            })
        };
        // A thin, full-height first letter is not punctuation.
        assert_eq!(
            dotted_offset(&crop(60, 32), content, "1.word", &mut None),
            Some(60)
        );
        // A label and detached marks alone supply no word-body candidate.
        assert_eq!(
            dotted_offset(&crop(60, 5), content, "1.word", &mut None),
            None
        );
        // A candidate farther than the existing label aspect bound is refused,
        // not tried and followed by more OCR until a word appears.
        assert_eq!(
            dotted_offset(&crop(160, 32), content, "1.word", &mut None),
            None
        );
        assert_eq!(
            dotted_offset(&GrayImage::new(212, 52), content, "1.word", &mut None),
            None
        );
    }

    #[test]
    fn two_glyph_label_needs_a_low_separated_delimiter_before_the_word() {
        let content = crate::split::Frame {
            cx: 0.,
            cy: 0.,
            w: 280.,
            h: 50.,
            angle: 0.,
        };
        let crop = |dot_y: Option<u32>, word_x: Option<u32>, merged: bool| {
            GrayImage::from_fn(295, 65, |x, y| {
                let first =
                    (10..if merged { 60 } else { 20 }).contains(&x) && (10..55).contains(&y);
                let second = (35..60).contains(&x) && (10..55).contains(&y);
                let dot =
                    dot_y.is_some_and(|dy| (75..80).contains(&x) && (dy..dy + 5).contains(&y));
                let word =
                    word_x.is_some_and(|wx| (wx..wx + 40).contains(&x) && (10..55).contains(&y));
                image::Luma([if first || second || dot || word {
                    20
                } else {
                    235
                }])
            })
        };
        for head in ["12.word", "Hl.word"] {
            assert_eq!(
                dotted_offset(&crop(Some(47), Some(100), false), content, head, &mut None),
                Some(100)
            );
            assert_eq!(
                dotted_offset(&crop(Some(47), Some(100), true), content, head, &mut None),
                Some(100)
            );
            // A high dot, an absent delimiter, a label alone or a word beyond
            // the existing label extent cannot license an internal digit cut.
            for c in [
                crop(Some(12), Some(100), false),
                crop(None, Some(100), false),
                crop(Some(47), None, false),
                crop(Some(47), Some(230), false),
            ] {
                assert_eq!(dotted_offset(&c, content, head, &mut None), None);
            }
        }
    }

    fn prepared(whole: &str, token: Option<&str>) -> Prepared {
        let quad = Quad([(0., 0.), (160., 0.), (160., 40.), (0., 40.)]);
        Prepared {
            cell: None,
            writing: WritingFrame::Page(
                PageAxis::from_detections(&[quad.clone()]).direction(false),
            ),
            quad,
            crop: GrayImage::new(172, 52),
            ranked: vec![(Word::from_bip39("history").unwrap().index() as usize, 0.99)],
            turned: false,
            read: Some(LineRead {
                text: whole.into(),
                confidence: 0.9,
            }),
            cut: Some(40),
            token: token.map(|t| LineRead {
                text: t.into(),
                confidence: 0.9,
            }),
            decisions: Some(DecisionTrace::default()),
        }
    }

    #[test]
    fn cell_ordinal_does_not_authorise_cutting_a_word_internal_gap() {
        for (whole, token) in [("Spoon", "sp"), ("churn", "Ch"), ("artwork", "art")] {
            let mut p = prepared(whole, Some(token));
            let before = p.quad.clone();
            let mut owned = WordEvidence::default();
            let result = apply(
                &mut p,
                &mut owned,
                Some(8),
                true,
                false,
                Some(Evidence::Cell(Label::Exact(8))),
                true,
                0.15,
                |_, _| panic!("cell numbering is not prefix ink evidence"),
            )
            .unwrap();
            assert_eq!(result, (false, false));
            assert_eq!(p.quad, before);
            assert_eq!(p.read.as_ref().unwrap().text, whole);
            assert!(owned.matching.is_none());
        }
    }

    #[test]
    fn corroborated_literal_cuts_preserve_exact_words_and_read_at_most_once() {
        for (whole, token, suffix, confidence, accepted, calls) in [
            ("12.5Tory", Some("12"), "STORy", 0.918, true, 1),
            ("B.BANANA", Some("8"), "BANANA", 0.9, true, 1),
            ("B.BANANA", Some("8"), "band", 0.9, false, 1),
            ("9.Hobby", Some("9."), ".Hobby", 0.9, true, 1),
            ("9.Hobby", Some("9."), "hobby", f32::NAN, false, 1),
            ("12.5Tory", Some("12"), "", 0.9, false, 1),
            ("co.rn", Some("c"), "corn", 0.9, false, 0),
            ("co.rn", Some("co"), "run", 0.9, false, 1),
            ("ab.use", Some("ab"), "use", 0.9, false, 1),
            ("a.gain", Some("a"), "gain", 0.9, false, 1),
            ("B.BANANA", Some(""), "banana", 0.9, false, 0),
            ("4.word", None, "word", 0.9, false, 0),
        ] {
            let mut p = prepared(whole, token);
            let initial = p.crop.clone();
            let mut labelled = label_evidence(&[Candidate {
                quad: &p.quad,
                read: Some(whole),
                token,
                word_like: true,
            }]);
            let mut owned = labelled.observations.remove(0);
            let labels = owned.to_json()["labels"].clone();
            let mut reads = 0;
            let (narrowed, stray) = apply(
                &mut p,
                &mut owned,
                None,
                false,
                labelled.label_box[0],
                labelled.evidence[0],
                true,
                CROP_MARGIN,
                |p, cut| {
                    reads += 1;
                    Ok(Past {
                        crop: image::imageops::crop_imm(
                            &p.crop,
                            cut,
                            0,
                            p.crop.width() - cut,
                            p.crop.height(),
                        )
                        .to_image(),
                        ranked: vec![(Word::from_bip39("story").unwrap().index() as usize, 0.01)],
                        read: LineRead {
                            text: suffix.into(),
                            confidence,
                        },
                    })
                },
            )
            .unwrap();
            assert_eq!(
                (narrowed, stray, reads),
                (accepted, false, calls),
                "{whole} / {token:?} / {suffix}"
            );
            assert_eq!(
                owned.to_json()["labels"],
                labels,
                "cut role must not rewrite ordinal observations"
            );
            if accepted {
                assert_eq!(owned.original.as_ref().unwrap().raw.text, whole);
                assert_eq!(owned.original.as_ref().unwrap().crop, initial);
                assert_eq!(p.read.as_ref().unwrap().text, suffix);
                let word = finish_evidenced(
                    p,
                    &Calibration {
                        min_prob: 0.85,
                        min_margin: 0.75,
                        digest: String::new(),
                        rule: CANON_RULE.into(),
                    },
                    narrowed,
                    false,
                    None,
                    owned,
                )
                .unwrap();
                assert_eq!(
                    Word::from_index(word.pick().unwrap() as u16)
                        .unwrap()
                        .as_str(),
                    normalise(suffix)
                );
            } else {
                assert!(owned.original.is_none());
                assert_eq!(p.crop, initial);
                assert_eq!(p.read.as_ref().unwrap().text, whole);
            }
        }
    }
}
