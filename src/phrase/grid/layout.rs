//! Complete a numbered list's grid from the SAME observation groups used for
//! word ordering. Missing readable labels do not erase a word column.
use super::*;

/// Spatial identity is bounded by label scale AND neighbouring column pitch.
/// Member IDs can rule out a match, but never make a far-away guide compatible.
pub(super) fn guide_owner(
    guides: &[(ColumnBounds, serde_json::Value)],
    claimed: &std::collections::HashSet<usize>,
    members: &[usize],
    x: f32,
    y: f32,
    height: f32,
    pitch: f32,
) -> Option<usize> {
    guide_owner_across(guides, claimed, members, x, y, height, pitch, &|_| false)
}

/// As `guide_owner`, but `clear` may say that the paper between a guide's line
/// and these words holds no other writing.
///
/// A label is only as far from its word as the sheet prints it, and a sheet
/// that sets its numbers well clear of its ruled lines is not thereby a
/// different column: page-18's numbers end at x 352 and its words begin at
/// 477, 125px of blank paper with nothing in it, and the guide its six numbers
/// built could not reach them. What must not happen is a guide reaching across
/// a *neighbouring* column, and that is ink — the neighbour's own writing lies
/// in between — so emptiness is the thing to ask about, under the same cap of
/// just under half the column pitch.
pub(super) fn guide_owner_across(
    guides: &[(ColumnBounds, serde_json::Value)],
    claimed: &std::collections::HashSet<usize>,
    members: &[usize],
    x: f32,
    y: f32,
    height: f32,
    pitch: f32,
    clear: &dyn Fn(f32) -> bool,
) -> Option<usize> {
    guides
        .iter()
        .enumerate()
        .filter(|(i, (c, v))| {
            let gutter = v["label_gutter_width"].as_f64().unwrap_or(height as f64) as f32;
            let radius = (height + gutter).min(0.45 * pitch);
            let gap = (c.left(y) - x).abs();
            !claimed.contains(i)
                && (gap <= radius || (gap <= 0.45 * pitch && clear(c.left(y))))
                && v["word_layout_members"].as_array().is_none_or(|ids| {
                    ids.iter()
                        .any(|id| members.iter().any(|&m| id.as_u64() == Some(m as u64)))
                })
        })
        .min_by(|(_, (a, _)), (_, (b, _))| (a.left(y) - x).abs().total_cmp(&(b.left(y) - x).abs()))
        .map(|(i, _)| i)
}

// Legal space is not a predicted word length. Resolve this after all label/dot
// refinements so every crop owner and the overlay receive the same boundary.
pub(super) fn extend_right_domains(
    guides: &mut [(ColumnBounds, serde_json::Value)],
    photo: &RgbImage,
    writing: WritingFrame,
    all: &[Quad],
) {
    let boxes: Vec<_> = all
        .iter()
        .map(|q| Bounds::of(&writing.project(q)))
        .collect();
    let photo_right = Bounds::of(&writing.project(&Quad([
        (0., 0.),
        (photo.width() as f32, 0.),
        (photo.width() as f32, photo.height() as f32),
        (0., photo.height() as f32),
    ])))
    .r;
    for i in 0..guides.len() {
        let domain = guides[i].0;
        let next = guides
            .iter()
            .enumerate()
            .filter(|(j, (c, _))| {
                *j != i
                    && c.left((domain.top + domain.bottom) / 2.)
                        > domain.left((domain.top + domain.bottom) / 2.)
            })
            .min_by(|(_, (a, _)), (_, (b, _))| {
                a.left((domain.top + domain.bottom) / 2.)
                    .total_cmp(&b.left((domain.top + domain.bottom) / 2.))
            })
            .map(|(_, (c, v))| {
                (
                    c.intercept - v["label_gutter_width"].as_f64().unwrap_or(0.) as f32,
                    c.slope,
                )
            });
        let (c, v) = &mut guides[i];
        let before = c.right;
        // Interior edges follow the next measured label gutter, including its
        // taper. There is no parallel-edge constraint. The last column stops
        // at its widest detected row plus 20%, not at the table/photo edge.
        let members: Vec<_> = v["word_layout_members"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|id| id.as_u64().map(|i| i as usize))
            .filter(|&i| {
                boxes
                    .get(i)
                    .is_some_and(|b| b.y() >= c.top && b.y() <= c.bottom && b.r > c.left(b.y()))
            })
            .collect();
        let rows = detector_rows(&boxes, &members);
        let widest = rows
            .iter()
            .map(|row| {
                let l = row
                    .iter()
                    .map(|&i| boxes[i].l.max(c.left_limit(boxes[i])))
                    .reduce(f32::min)
                    .unwrap();
                let r = row.iter().map(|&i| boxes[i].r).reduce(f32::max).unwrap();
                (l, r, (r - l).max(0.))
            })
            .max_by(|a, b| a.2.total_cmp(&b.2));
        let row_limit = widest
            .map(|(_, r, width)| r + 0.2 * width)
            .unwrap_or(before);
        // Preserve all measured row ends even when a shorter row is offset.
        let row_limit = members
            .iter()
            .map(|&i| boxes[i].r)
            .fold(row_limit, f32::max);
        let (desired, slope) = next.unwrap_or((row_limit, 0.));
        // The image's projected envelope caps the guide. Actual photo-edge
        // clipping belongs to Grid::constrain at EACH WORD's height: using the
        // photo intersection at the column's top would truncate wider rows
        // farther down a rotated photograph.
        let photo_intercept = photo_right - (slope * c.top).max(slope * c.bottom);
        c.right_slope = slope;
        c.right = desired.min(photo_intercept);
        v["right_boundary"] = json!({"policy":if next.is_some(){"next_label_gutter"}else{"widest_row_plus_20_percent"},
            "previous_x":before,"intercept":c.right,"slope":c.right_slope,
            "widest_row":widest,"column_allowance_fraction":0.2,"detector_boxes_not_expanded":true});
        v["quad"] = json!(writing.unproject(&c.quad()).0);
        if let Some(rows) = v["rows"].as_array_mut() {
            for row in rows {
                let p: (f32, f32) = serde_json::from_value(row["line"][0].clone()).unwrap();
                let y = writing.project(&Quad([p; 4])).0[0].1;
                let q = writing.unproject(&Quad([
                    (c.left(y), y),
                    (c.right_at(y), y),
                    (c.right_at(y), y),
                    (c.left(y), y),
                ]));
                row["line"] = json!([q.0[0], q.0[1]]);
            }
        }
    }
}

// A split word can produce several nearby detector centres. They are not
// independent row observations when their vertical extents overlap within
// less than half the column's typical row spacing. Label-derived rows never
// enter this reconstruction path.
pub(super) fn detector_rows(boxes: &[Bounds], members: &[usize]) -> Vec<Vec<usize>> {
    let mut order = members.to_vec();
    order.sort_by(|&a, &b| boxes[a].y().total_cmp(&boxes[b].y()));
    let gaps: Vec<_> = order
        .windows(2)
        .map(|p| boxes[p[1]].y() - boxes[p[0]].y())
        .filter(|d| *d >= 1.)
        .collect();
    let pitch = if gaps.is_empty() { 0. } else { median(gaps) };
    let mut rows: Vec<Vec<usize>> = Vec::new();
    for i in order {
        if let Some(row) = rows.last_mut() {
            let top = row.iter().map(|&j| boxes[j].t).fold(boxes[i].t, f32::max);
            let bottom = row.iter().map(|&j| boxes[j].b).fold(boxes[i].b, f32::min);
            if top < bottom && boxes[i].y() - boxes[row[0]].y() < pitch / 2. {
                row.push(i);
                continue;
            }
        }
        rows.push(vec![i]);
    }
    rows
}

// A sparse label fit must not dictate a steep extrapolation when prefix gaps
// across the whole word track disagree. Use independent rows, not OCR scores.
fn full_track_fit(points: &[(f32, f32)], height: f32, span: f32) -> Option<(f32, f32, usize, f32)> {
    track_fit(points, height, span, 3)
}

fn track_fit(
    points: &[(f32, f32)],
    height: f32,
    span: f32,
    minimum: usize,
) -> Option<(f32, f32, usize, f32)> {
    let mut best: Option<Vec<(f32, f32)>> = None;
    let mut competing = false;
    for (i, &(y, x)) in points.iter().enumerate() {
        for &(yy, xx) in &points[i + 1..] {
            if (yy - y).abs() < height {
                continue;
            }
            let slope = (xx - x) / (yy - y);
            if slope.abs() > 0.3 {
                continue;
            }
            let intercept = x - slope * y;
            let mut inliers: Vec<_> = points
                .iter()
                .copied()
                .filter(|&(py, px)| (px - intercept - slope * py).abs() <= 0.25 * height)
                .collect();
            inliers.sort_by(|a, b| a.0.total_cmp(&b.0));
            inliers.dedup_by(|a, b| (a.0 - b.0).abs() < height * 0.5);
            if inliers.len() < minimum
                || inliers.len() * 2 <= points.len()
                || inliers.last()?.0 - inliers.first()?.0 < span
            {
                continue;
            }
            if best.as_ref().is_none_or(|b| inliers.len() > b.len()) {
                best = Some(inliers);
                competing = false;
            } else if minimum == 2 && best.as_ref().is_some_and(|b| b.len() == inliers.len()) {
                let (a, s) = regress(best.as_ref().unwrap())?;
                let (aa, ss) = regress(&inliers)?;
                if [inliers.first()?.0, inliers.last()?.0]
                    .iter()
                    .any(|&y| ((a + s * y) - (aa + ss * y)).abs() > 0.5 * height)
                {
                    competing = true;
                }
            }
        }
    }
    if competing {
        return None;
    }
    let best = best?;
    let (intercept, slope) = regress(&best)?;
    (slope.abs() <= 0.3).then_some((
        intercept,
        slope,
        best.len(),
        best.last()?.0 - best.first()?.0,
    ))
}

fn retreating_fit(column: ColumnBounds, a: f32, s: f32, height: f32) -> Option<(f32, f32)> {
    let changes = [column.top, column.bottom].map(|y| a + s * y - column.left(y));
    let advance = changes[0].max(changes[1]);
    let retreat = -changes[0].min(changes[1]);
    // This repair addresses destructive extrapolation, not new label removal.
    // Proposals may be word-internal gaps: they cannot justify advancing the
    // boundary. Small crossings of the old line are aligned left so no point
    // over the full shared extent cuts deeper than it did before.
    (advance <= 0.25 * height && retreat >= 0.25 * height).then_some((a - advance.max(0.), s))
}

pub(super) fn complete(
    guides: &mut Vec<(ColumnBounds, serde_json::Value)>,
    all: &[Quad],
    writing: WritingFrame,
    proposals: &[(usize, Quad, Frame, Bounds, f32)],
    anchored: bool,
) {
    let layout = crate::layout::PageLayout::with_writing(all, writing);
    let boxes: Vec<_> = all
        .iter()
        .map(|q| Bounds::of(&writing.project(q)))
        .collect();
    let supported_span = (!guides.is_empty()).then(|| {
        (
            guides
                .iter()
                .map(|(c, _)| c.top)
                .fold(f32::INFINITY, f32::min),
            guides
                .iter()
                .map(|(c, _)| c.bottom)
                .fold(f32::NEG_INFINITY, f32::max),
        )
    });
    let mut spans = Vec::new();
    let mut reconciliations = Vec::new();
    // A physical group owns at most one guide and a guide owns at most one
    // group. Word width is not a column-identity tolerance: wide words can
    // reach across a neighbouring label track.
    let mut claimed = std::collections::HashSet::new();
    let starts_by_group: Vec<_> = layout
        .columns
        .iter()
        .filter_map(|members| {
            let (words, _) = layout.dominant_word_block(members, 1.2)?;
            Some(median(words.iter().map(|&i| boxes[i].l).collect()))
        })
        .collect();
    for members in &layout.columns {
        let Some((words, height)) = layout.dominant_word_block(members, 1.2) else {
            continue;
        };
        let width = median(words.iter().map(|&i| boxes[i].w()).collect());
        if width < 2. * height {
            continue;
        } // not a track of standalone labels
        let top = words
            .iter()
            .map(|&i| boxes[i].t)
            .fold(f32::INFINITY, f32::min);
        let bottom = words
            .iter()
            .map(|&i| boxes[i].b)
            .fold(f32::NEG_INFINITY, f32::max);
        if supported_span.is_some_and(|(t, b)| bottom < t || top > b) {
            continue;
        }
        if bottom - top < 2. * height {
            continue;
        }
        let y = (top + bottom) / 2.;
        let starts = median(words.iter().map(|&i| boxes[i].l).collect());
        let mut endpoints: Vec<_> = proposals
            .iter()
            .filter(|p| words.contains(&p.0))
            .map(|p| {
                let b = from_crop(p.3, p.2, writing);
                let x = from_crop(band(p.3.r + p.4 / 2.), p.2, writing).l;
                (b.y(), x)
            })
            .collect();
        endpoints.retain(|(_, x)| *x >= starts - height && *x <= starts + 1.5 * height);
        let pitch = starts_by_group
            .iter()
            .map(|x| (x - starts).abs())
            .filter(|d| *d > height)
            .reduce(f32::min)
            .unwrap_or(f32::INFINITY);
        // Word-shaped observations only: the labels themselves stand between a
        // guide's line and its words by construction.
        let clear = |line: f32| {
            let (lo, hi) = (line.min(starts), line.max(starts));
            !boxes.iter().enumerate().any(|(i, b)| {
                !words.contains(&i)
                    && b.w() >= 2. * b.h()
                    && b.r > lo
                    && b.l < hi
                    && b.b > top
                    && b.t < bottom
            })
        };
        if let Some(index) =
            guide_owner_across(guides, &claimed, &words, starts, y, height, pitch, &clear)
        {
            let view = &mut guides[index].1;
            claimed.insert(index);
            spans.push((top, bottom));
            view["word_layout_members"] = json!(words);
            view["group_assignment"] = json!("unique_spatial_owner");
            let observed = view["observed_y_span"]
                .as_array()
                .map(|s| (s[1].as_f64().unwrap() - s[0].as_f64().unwrap()) as f32)
                .unwrap_or(bottom - top);
            if view["method"] != "dot_track_boundary" && observed < (bottom - top) * 0.5 {
                if let Some((a, s, count, span)) =
                    full_track_fit(&endpoints, height, (bottom - top) * 0.5)
                {
                    reconciliations.push((index, a, s, height, count, span));
                }
            }
            continue;
        }
        // Prefix components refine the estimate when their endpoints form a
        // coherent track. Otherwise the aligned box starts remain the explicit
        // estimate; neither absence nor a bad individual OCR read vetoes it.
        let (mut intercept, mut slope, mut method) = if endpoints.len() >= 2 {
            let x = median(endpoints.iter().map(|p| p.1).collect());
            endpoints.retain(|p| (p.1 - x).abs() <= 0.75 * height);
            if endpoints.len() >= 2 {
                let (a, s) = regress(&endpoints)
                    .filter(|(_, s)| s.abs() <= 0.3)
                    .unwrap_or((x, 0.));
                (a, s, "estimated_prefix_track")
            } else {
                (starts, 0., "estimated_word_starts")
            }
        } else {
            (starts, 0., "estimated_word_starts")
        };
        // With no independently established list, aligned word starts alone
        // are not evidence of labels. Bootstrap only from repeated substantial
        // prefix gaps; OCR need not read those prefixes successfully.
        //
        // A page that read no label at all is in that position however many
        // guides some other column found: a dot track is evidence of a track,
        // not of labels, and the authority it carries is its own. Without it
        // an unlabelled page draws a gutter through its words and cuts their
        // first letters away.
        let mut whitespace_evidence = serde_json::Value::Null;
        if guides.is_empty() || !anchored {
            let separated: Vec<_> = proposals
                .iter()
                .filter(|p| {
                    words.contains(&p.0)
                        && p.4 >= (0.35 * height).max(0.5 * p.3.w())
                        && p.3.w() <= 1.5 * p.3.h()
                        && p.2.w - p.3.r - p.4 >= (2. * p.3.w()).max(1.5 * height)
                })
                .map(|p| {
                    (
                        from_crop(p.3, p.2, writing).y(),
                        from_crop(band(p.3.r + p.4 / 2.), p.2, writing).l,
                    )
                })
                .collect();
            let Some((a, s, count, span)) = track_fit(&separated, height, 0.5 * (bottom - top), 2)
            else {
                continue;
            };
            intercept = a;
            slope = s;
            method = "spatial_whitespace_track";
            whitespace_evidence = json!({"points_y_x":separated,"independent_rows":count,"y_span":span,
                "minimum_gap_word_heights":0.35,"minimum_gap_prefix_widths":0.5,
                "minimum_body_prefix_widths":2.,"minimum_body_word_heights":1.5,
                "minimum_supports":2,"ocr_required":false});
        }
        spans.push((top, bottom));
        let right = words
            .iter()
            .map(|&i| boxes[i].r)
            .fold(f32::NEG_INFINITY, f32::max);
        let c = ColumnBounds {
            right_slope: 0.,
            intercept,
            slope,
            right,
            top,
            bottom,
        };
        claimed.insert(guides.len());
        guides.push((
            c,
            json!({"quad":writing.unproject(&c.quad()).0,
            "rows":[],"display_only":false,"supports":0,"estimated":true,
            "method":method,"word_layout_members":words,"label_gutter_width":height,
            "whitespace_evidence":whitespace_evidence,
            "observed_y_span":[top,bottom],"ordinal_inferred":false}),
        ));
    }
    if guides.is_empty() {
        return;
    }
    let top = guides
        .iter()
        .map(|(c, _)| c.top)
        .chain(spans.iter().map(|s| s.0))
        .fold(f32::INFINITY, f32::min);
    let bottom = guides
        .iter()
        .map(|(c, _)| c.bottom)
        .chain(spans.iter().map(|s| s.1))
        .fold(f32::NEG_INFINITY, f32::max);
    for (index, a, s, height, count, span) in reconciliations {
        let (column, view) = &mut guides[index];
        let domain = ColumnBounds {
            top,
            bottom,
            ..*column
        };
        let corrected = retreating_fit(domain, a, s, height);
        view["full_track_reconciliation"] = json!({
            "previous_intercept":column.intercept,"previous_slope":column.slope,
            "proposed_intercept":a,"proposed_slope":s,"prefix_supports":count,"prefix_y_span":span,
            "accepted":corrected.is_some(),"corrected_line":corrected,
            "reason":if corrected.is_some() { "retreating_full_track_repair" } else { "not_a_safe_retreat" }
        });
        if let Some((a, s)) = corrected {
            column.intercept = a;
            column.slope = s;
        }
    }
    guides.sort_by(|(a, _), (b, _)| {
        a.left((top + bottom) / 2.)
            .total_cmp(&b.left((top + bottom) / 2.))
    });
    for i in 0..guides.len() {
        let next = guides
            .get(i + 1)
            .map(|(c, v)| (*c, v["label_gutter_width"].as_f64().unwrap() as f32));
        let (c, v) = &mut guides[i];
        c.top = top;
        c.bottom = bottom;
        if let Some((n, w)) = next {
            c.right = c.right.min(n.left(top).min(n.left(bottom)) - w);
        }
        v["quad"] = json!(writing.unproject(&c.quad()).0);
        v["shared_list_y_span"] = json!([top, bottom]);
        v["display_only"] = json!(false);
        if let Some(rows) = v["rows"].as_array_mut() {
            for row in rows {
                let line: [(f32, f32); 2] = serde_json::from_value(row["line"].clone()).unwrap();
                let y =
                    Bounds::of(&writing.project(&Quad([line[0], line[1], line[1], line[0]]))).y();
                let q = writing.unproject(
                    &Bounds {
                        l: c.left(y),
                        r: c.right,
                        t: y,
                        b: y,
                    }
                    .quad(),
                );
                row["line"] = json!([q.0[0], q.0[1]]);
            }
        }
        if v["rows"].as_array().is_none_or(Vec::is_empty) {
            let members: Vec<usize> = v["word_layout_members"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|i| i.as_u64().unwrap() as usize)
                .collect();
            v["rows"] = json!(
                detector_rows(&boxes, &members).iter().map(|row| {
                        let top = row.iter().map(|&i|boxes[i].t).fold(f32::INFINITY,f32::min);
                        let bottom = row.iter().map(|&i|boxes[i].b).fold(f32::NEG_INFINITY,f32::max);
                        let y = (top+bottom)/2.;
                        let q = writing.unproject(
                            &Bounds {
                                l: c.left(y),
                                r: c.right,
                                t: y,
                                b: y,
                            }
                            .quad(),
                        );
                        json!({"ordinal":null,"line":[q.0[0],q.0[1]],"observed_label":false,
                            "source_observations":row,"method":"overlapping_detector_row",
                            "original_centres":row.iter().map(|&i|boxes[i].y()).collect::<Vec<_>>()})
                    })
                    .collect::<Vec<_>>()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owners_are_unique_spatial_and_independent_of_word_width() {
        let guide = |x: f32| {
            (
                ColumnBounds {
                    intercept: x,
                    slope: 0.,
                    right: x + 500.,
                    right_slope: 0.,
                    top: 0.,
                    bottom: 600.,
                },
                json!({"label_gutter_width":50.}),
            )
        };
        let mut guides = vec![guide(80.), guide(480.), guide(830.)];
        let mut claimed = std::collections::HashSet::new();
        for (group, start) in [100., 500., 850.].into_iter().enumerate() {
            let owner = guide_owner(&guides, &claimed, &[group], start, 300., 60., 250.).unwrap();
            assert_eq!(owner, group);
            claimed.insert(owner);
        }
        assert_eq!(
            guide_owner(&guides, &claimed, &[3], 1000., 300., 60., 150.),
            None
        );
        // Even forged membership cannot attach the far-away fourth column to
        // the third guide. Its 500px word width is irrelevant to identity.
        claimed.clear();
        guides[2].1["word_layout_members"] = json!([3]);
        assert_eq!(
            guide_owner(&guides, &claimed, &[3], 1000., 300., 60., 150.),
            None
        );
        guides.push(guide(980.));
        assert_eq!(
            guide_owner(&guides, &claimed, &[3], 1000., 300., 60., 150.),
            Some(3)
        );
    }

    /// page-18 prints its numbers 136px from where its words begin, further
    /// than a word height and a gutter, with 125px of blank paper between.
    /// Emptiness is what separates "my word, set further out" from "the next
    /// column's word": reaching into a neighbour means crossing its writing.
    #[test]
    fn a_guide_reaches_across_blank_paper_but_never_across_writing() {
        let guide = |x: f32| {
            (
                ColumnBounds {
                    intercept: x,
                    slope: 0.,
                    right: x + 500.,
                    right_slope: 0.,
                    top: 0.,
                    bottom: 600.,
                },
                json!({"label_gutter_width":59.}),
            )
        };
        let guides = vec![guide(343.)];
        let claimed = std::collections::HashSet::new();
        let (start, y, height, pitch) = (479., 300., 52., f32::INFINITY);
        // height + gutter is 111 and the gap is 136: too far on distance alone.
        assert_eq!(
            guide_owner(&guides, &claimed, &[0], start, y, height, pitch),
            None
        );
        // Blank paper in between, and the same guide owns the group.
        assert_eq!(
            guide_owner_across(&guides, &claimed, &[0], start, y, height, pitch, &|_| true),
            Some(0)
        );
        // Writing in between and it does not, however empty the rule's own
        // distance test would have left it.
        assert_eq!(
            guide_owner_across(&guides, &claimed, &[0], start, y, height, pitch, &|_| false),
            None
        );
    }

    #[test]
    fn a_clear_gap_still_stops_at_the_neighbouring_column() {
        let guides = vec![(
            ColumnBounds {
                intercept: 100.,
                slope: 0.,
                right: 600.,
                right_slope: 0.,
                top: 0.,
                bottom: 600.,
            },
            json!({"label_gutter_width":20.}),
        )];
        let claimed = std::collections::HashSet::new();
        // Columns 400 apart: the cap is 180, and 300 is beyond it even with
        // nothing in the way, so a wide word cannot claim the column next door.
        assert_eq!(
            guide_owner_across(&guides, &claimed, &[0], 400., 300., 40., 400., &|_| true),
            None
        );
        // Within the cap, emptiness carries it.
        assert_eq!(
            guide_owner_across(&guides, &claimed, &[0], 250., 300., 40., 400., &|_| true),
            Some(0)
        );
    }

    #[test]
    fn aligned_word_starts_alone_cannot_bootstrap_labels() {
        let all: Vec<_> = (0..12)
            .map(|i| {
                Bounds {
                    l: 50.,
                    r: 200.,
                    t: i as f32 * 50.,
                    b: i as f32 * 50. + 30.,
                }
                .quad()
            })
            .collect();
        let mut guides = Vec::new();
        complete(&mut guides, &all, WritingFrame::Local, &[], false);
        assert!(guides.is_empty());
    }

    /// A dot track is evidence of a track, not of labels. A page that read no
    /// label anywhere gets the same treatment as a page with nothing at all:
    /// aligned word starts do not draw a gutter, whatever some other column
    /// found. Otherwise an unlabelled page cuts the first letters off its own
    /// words — page-29 read `noLd` for uphold and `sTLe` for wrestle.
    #[test]
    fn a_page_that_read_no_label_does_not_borrow_another_columns_authority() {
        let column = |x: f32| {
            (0..4).map(move |i| {
                Bounds {
                    l: x,
                    r: x + 150.,
                    t: i as f32 * 60.,
                    b: i as f32 * 60. + 40.,
                }
                .quad()
            })
        };
        let all: Vec<_> = column(50.)
            .chain(column(500.))
            .chain(column(950.))
            .collect();
        let established = |anchored: bool| {
            let mut guides = vec![(
                ColumnBounds {
                    intercept: 930.,
                    slope: 0.,
                    right: 1150.,
                    right_slope: 0.,
                    top: 0.,
                    bottom: 220.,
                },
                json!({"label_gutter_width":40., "method":"dot_track_boundary", "estimated":false}),
            )];
            complete(&mut guides, &all, WritingFrame::Local, &[], anchored);
            guides
                .iter()
                .filter(|(_, view)| view["estimated"] == json!(true))
                .count()
        };
        // With a label read somewhere on the page, the other columns may be
        // estimated from it: page-14 does exactly that.
        assert_eq!(established(true), 2);
        // With none, the dot track keeps its own column and draws no others.
        assert_eq!(established(false), 0);
    }

    #[test]
    fn two_distributed_gaps_fit_but_competing_tracks_remain_unresolved() {
        assert!(track_fit(&[(0., 20.), (150., 25.)], 20., 100., 2).is_some());
        assert!(track_fit(&[(0., 20.), (2., 20.)], 20., 100., 2).is_none());
        assert!(track_fit(&[(0., 20.), (100., 20.), (200., 40.)], 20., 50., 2).is_none());
    }

    #[test]
    fn overlapping_fragments_share_a_row_but_disjoint_rows_do_not() {
        for angle in [0., 18.] {
            let make =
                |l, t, r, b| Quad([(l, t), (r, t), (r, b), (l, b)].map(|(x, y)| turn(x, y, angle)));
            let axis = make(0., 0., 400., 60.);
            let writing = WritingFrame::Page(
                crate::page_frame::PageAxis::from_detections(&[axis]).direction(false),
            );
            let quads = [
                make(50., 100., 300., 160.),
                make(200., 200., 270., 250.),
                make(80., 220., 210., 280.),
                make(50., 330., 300., 390.),
            ];
            let mut boxes: Vec<_> = quads
                .iter()
                .map(|q| Bounds::of(&writing.project(q)))
                .collect();
            assert_eq!(
                detector_rows(&boxes, &[0, 1, 2, 3]),
                vec![vec![0], vec![1, 2], vec![3]]
            );
            // Equally close centres are not enough: physical row bands must
            // overlap too. Do not merge genuinely separate short writing.
            boxes[1] = Bounds::of(&writing.project(&make(200., 220., 270., 235.)));
            boxes[2] = Bounds::of(&writing.project(&make(80., 245., 210., 260.)));
            assert_eq!(
                detector_rows(&boxes, &[0, 1, 2, 3]),
                vec![vec![0], vec![1], vec![2], vec![3]]
            );
        }
    }

    #[test]
    fn full_track_repair_never_advances_into_word_ink() {
        let column = ColumnBounds {
            right_slope: 0.,
            intercept: 20.,
            slope: 0.2,
            right: 200.,
            top: 0.,
            bottom: 120.,
        };
        let (a, s) = retreating_fit(column, 22., 0.05, 20.).unwrap();
        assert_eq!(a, 20.);
        for y in [0., 30., 60., 120.] {
            assert!(a + s * y <= column.left(y));
        }
        assert!(retreating_fit(column, 40., 0.2, 20.).is_none());
        assert!(retreating_fit(column, 21., 0.2, 20.).is_none());
    }

    #[test]
    fn full_track_rejects_a_word_internal_gap_and_requires_distributed_rows() {
        // A short, steep two-label fit must yield to four coherent endpoints
        // covering the full column; one gap inside a word is not another label.
        let points = [(0., 20.), (30., 22.), (60., 70.), (90., 24.), (120., 26.)];
        let (a, s, supports, span) = full_track_fit(&points, 20., 60.).unwrap();
        assert_eq!(supports, 4);
        assert_eq!(span, 120.);
        assert!((a - 20.).abs() < 1.);
        assert!(a + s * 120. < 27.);
        assert!(full_track_fit(&points[..2], 20., 60.).is_none());
        assert!(full_track_fit(&[(0., 20.), (1., 20.), (2., 20.)], 20., 60.).is_none());
        assert!(full_track_fit(&[(0., 20.), (30., 20.), (60., 20.)], 20., 100.).is_none());
    }
}
