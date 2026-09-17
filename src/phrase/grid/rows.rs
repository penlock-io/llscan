//! Reconcile sparse numbered-label rows with the observed word/dot rows.
//! A terminal pair's local pitch must not override distinct rows farther away.
use super::*;

/// Where a guide's own numbered rows put its list, with half a row's pitch
/// of tolerance at each end. None when the guide carries no such rows.
fn label_row_span(view: &serde_json::Value, writing: WritingFrame) -> Option<(f32, f32)> {
    let rows = view["rows"].as_array()?;
    if rows.len() < 2 {
        return None;
    }
    let ys: Vec<f32> = rows
        .iter()
        .filter_map(|r| {
            let line: [(f32, f32); 2] = serde_json::from_value(r["line"].clone()).ok()?;
            Some(Bounds::of(&writing.project(&Quad([line[0], line[1], line[1], line[0]]))).y())
        })
        .collect();
    if ys.len() != rows.len() {
        return None;
    }
    let pitch = median(ys.windows(2).map(|p| p[1] - p[0]).collect());
    Some((ys[0] - 0.5 * pitch, ys[ys.len() - 1] + 0.5 * pitch))
}

pub(super) fn reconcile(
    guides: &mut [(ColumnBounds, serde_json::Value)],
    all: &[Quad],
    writing: WritingFrame,
    trace: &mut Option<DecisionTrace>,
) {
    if guides.is_empty() {
        return;
    }
    let boxes: Vec<_> = all
        .iter()
        .map(|q| Bounds::of(&writing.project(q)))
        .collect();
    // Before partitioning, one detector rectangle may bridge two columns.
    // Its row exists in BOTH domains. Likewise an isolated first row must not
    // disappear just because dominant_word_block chose the lower four rows.
    // Geometry owns these observations; membership from clustering is a seed,
    // not a veto on spatially present word ink.
    // Numbers that establish rows 1..N say where the list is, and a heading or
    // a footer is not a cell of it however close the sheet prints them:
    // page-18 sets its title two line-heights above row 1 and a second section
    // a line-height below row 12, and the block a guide's span comes from
    // never breaks for either. The span is every guide's rows together, not
    // each guide's own: page-12 numbers four columns of six, and a column
    // bounded by its own rows keeps a quarter of a 24-word list.
    let list = guides
        .iter()
        .filter_map(|(_, view)| label_row_span(view, writing))
        .reduce(|(t, b), (t2, b2)| (t.min(t2), b.max(b2)));
    for (column, view) in guides.iter_mut() {
        let mut members: Vec<usize> =
            serde_json::from_value(view["word_layout_members"].clone()).unwrap_or_default();
        if let Some((top, bottom)) = list {
            members.retain(|&i| boxes[i].y() >= top && boxes[i].y() <= bottom);
            view["label_row_span"] = json!([top, bottom]);
        }
        let heights: Vec<_> = members.iter().map(|&i| boxes[i].h()).collect();
        if heights.is_empty() {
            continue;
        }
        let height = median(heights);
        let mut added = Vec::new();
        for (i, b) in boxes.iter().enumerate() {
            let overlap = b.r.min(column.right_at(b.y())) - b.l.max(column.left(b.y()));
            if !members.contains(&i)
                && b.y() >= list.map_or(column.top, |(t, _)| t)
                && b.y() <= list.map_or(column.bottom, |(_, b)| b)
                && b.h() >= 0.5 * height
                && b.h() <= 2. * height
                && overlap >= 1.2 * b.h()
            {
                members.push(i);
                added.push(i);
            }
        }
        view["spatial_row_additions"] = json!(added);
        view["word_layout_members"] = json!(members);
    }
    let mut observed: Vec<Vec<serde_json::Value>> = guides
        .iter()
        .map(|(column, view)| {
            let members: Vec<_> = view["word_layout_members"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_u64().map(|i| i as usize))
                .filter(|&i| i < boxes.len())
                .collect();
            let groups = layout::detector_rows(&boxes, &members);
            let mut rows: Vec<_> = groups
                .iter()
                .map(|g| {
                    let top = g.iter().map(|&i| boxes[i].t).reduce(f32::min).unwrap();
                    let bottom = g.iter().map(|&i| boxes[i].b).reduce(f32::max).unwrap();
                    json!({"y":(top+bottom)/2., "source_observations":g, "dots":[]})
                })
                .collect();
            if rows.len() < 2 {
                return rows;
            }
            let ys: Vec<_> = rows
                .iter()
                .map(|r| r["y"].as_f64().unwrap() as f32)
                .collect();
            let pitch = median(ys.windows(2).map(|p| p[1] - p[0]).collect());
            // Dots locate rows without requiring the adjacent digit to be detected
            // or readable. Use only isolated marks belonging to the accepted track.
            let dots: Vec<_> = view["dot_reconciliation"]["measured_dot_edges"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|d| {
                    view["dot_reconciliation"]["supports"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .any(|s| {
                            s["candidate"] == d["candidate"] && s["isolation"]["isolated"] == true
                        })
                })
                .filter_map(|d| {
                    let [l, t, r, b] =
                        serde_json::from_value::<[f32; 4]>(d["photo_bounds"].clone()).ok()?;
                    let y = Bounds::of(&writing.project(&Bounds { l, t, r, b }.quad())).y();
                    Some((d["candidate"].clone(), y))
                })
                .collect();
            let nearest = |y: f32| {
                (0..ys.len())
                    .min_by(|&a, &b| (ys[a] - y).abs().total_cmp(&(ys[b] - y).abs()))
                    .unwrap()
            };
            let offsets: Vec<_> = dots
                .iter()
                .filter_map(|(_, y)| {
                    let delta = y - ys[nearest(*y)];
                    (delta.abs() < pitch * 0.45).then_some(delta)
                })
                .collect();
            if !offsets.is_empty() {
                // Dot centres usually sit below word centres. Estimate that offset
                // from observed rows, rather than treating the dot as a word centre.
                let offset = median(offsets);
                for (id, dot_y) in dots {
                    let y = dot_y - offset;
                    let i = nearest(y);
                    if (ys[i] - y).abs() < pitch * 0.45 {
                        rows[i]["dots"].as_array_mut().unwrap().push(id);
                    } else if y >= column.top && y <= column.bottom {
                        rows.push(json!({"y":y,"source_observations":[],"dots":[id]}));
                    }
                }
            }
            rows.sort_by(|a, b| {
                a["y"]
                    .as_f64()
                    .unwrap()
                    .total_cmp(&b["y"].as_f64().unwrap())
            });
            rows
        })
        .collect();
    let original = observed.clone();
    let selected = select_layout(&observed);
    if let Some(ref ranges) = selected {
        for (rows, range) in observed.iter_mut().zip(ranges) {
            *rows = rows[range.clone()].to_vec();
        }
        // Use a shared list span. A heading excluded by the layout must not
        // become ink in its first cell merely because the old span included it.
        let top = observed
            .iter()
            .map(|rows| y(&rows[0]) - (y(&rows[1]) - y(&rows[0])) / 2.)
            .reduce(f32::min)
            .unwrap();
        let bottom = observed
            .iter()
            .map(|rows| {
                let n = rows.len();
                y(&rows[n - 1]) + (y(&rows[n - 1]) - y(&rows[n - 2])) / 2.
            })
            .reduce(f32::max)
            .unwrap();
        for (bounds, view) in guides.iter_mut() {
            bounds.top = bounds.top.max(top);
            bounds.bottom = bounds.bottom.min(bottom);
            view["shared_list_y_span"] = json!([bounds.top, bounds.bottom]);
        }
    }
    let counts: Vec<_> = observed.iter().map(Vec::len).collect();
    let total: usize = counts.iter().sum();
    let accepted = selected.is_some();
    for (column, ((bounds, view), rows)) in guides.iter_mut().zip(observed).enumerate() {
        let previous = view["rows"].clone();
        let diagnostic = json!({"rule":"grid_row_reconciliation","status":"evaluated",
            "accepted":accepted,"column":column,"observed_rows":rows,
            "column_counts":counts,"total_cells":total,"allowed_totals":[12,24],
            "previous_rows":previous,"all_observed_rows":original[column],"extra_ocr_calls":0,
            "reason":if accepted {"observed_word_and_dot_rows"} else {"layout_count_unresolved"}});
        DecisionTrace::push(trace, || diagnostic.clone());
        view["row_fit"] = diagnostic;
        if !accepted {
            // The caller must disable global enforcement for this unresolved
            // layout. The old rows remain diagnostic, never fallback cells.
            view["rows"] = json!([]);
            continue;
        }
        view["rows"] = json!(rows.iter().enumerate().map(|(row, observation)| {
            let y = observation["y"].as_f64().unwrap() as f32;
            let q = writing.unproject(&Quad([(bounds.left(y),y),(bounds.right,y),
                                             (bounds.right,y),(bounds.left(y),y)]));
            json!({"line":[q.0[0],q.0[1]],"ordinal":column*counts[column]+row+1,
                "ordinal_inferred":true,"observed_label":false,
                "method":"observed_word_and_dot_rows",
                "source_observations":observation["source_observations"],"dots":observation["dots"]})
        }).collect::<Vec<_>>());
    }
}

fn y(row: &serde_json::Value) -> f32 {
    row["y"].as_f64().unwrap() as f32
}

/// Fit complete column-major layouts, preserving every dot-supported row.
/// Compare contiguous row windows by spacing smoothness, not word recognition.
/// A heading can be outside a list; an internal observed row cannot be skipped.
fn select_layout(rows: &[Vec<serde_json::Value>]) -> Option<Vec<std::ops::Range<usize>>> {
    for total in [24, 12] {
        if total % rows.len() != 0 {
            continue;
        }
        // At most one peripheral heading per column may be outside the list.
        // Never turn a nearly complete 24-word layout into twelve by silently
        // discarding half its rows to satisfy the count.
        if rows.iter().any(|r| r.len() > total / rows.len() + 1) {
            continue;
        }
        let n = total / rows.len();
        if n < 2 || rows.iter().any(|r| r.len() < n) {
            continue;
        }
        let windows: Option<Vec<_>> = rows
            .iter()
            .map(|column| {
                (0..=column.len() - n)
                    .filter(|&start| {
                        column.iter().enumerate().all(|(i, r)| {
                            (start..start + n).contains(&i)
                                || r["dots"].as_array().unwrap().is_empty()
                        })
                    })
                    .map(|start| {
                        let gaps: Vec<_> = column[start..start + n]
                            .windows(2)
                            .map(|p| y(&p[1]) - y(&p[0]))
                            .collect();
                        let pitch = median(gaps.clone()).max(1.);
                        let cost = gaps
                            .windows(2)
                            .map(|p| ((p[1] - p[0]) / pitch).powi(2))
                            .sum::<f32>();
                        (start, cost)
                    })
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(start, _)| start..start + n)
            })
            .collect();
        if windows.is_some() {
            return windows;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(y: f32, ordinal: u32) -> serde_json::Value {
        json!({"line":[[10., y],[40., y]],"ordinal":ordinal,"observed_label":false})
    }

    /// page-18 prints a title two line-heights above row 1 and a second
    /// section a line-height below row 12, and the word block a guide's span
    /// comes from breaks for neither. The rows its numbers establish say where
    /// the list is, and half a pitch either side is all the tolerance they need.
    #[test]
    fn a_guides_numbered_rows_bound_its_list() {
        let view = json!({"rows":(1..=12).map(|i| row(441. + 55.6 * (i - 1) as f32, i)).collect::<Vec<_>>()});
        let (top, bottom) = label_row_span(&view, WritingFrame::Local).unwrap();
        assert!((top - (441. - 27.8)).abs() < 1., "{top}");
        assert!((bottom - (1052.6 + 27.8)).abs() < 1., "{bottom}");
        // A title at 250 and a footer at 1140 are outside it; every list row is in.
        assert!(250. < top && 1140. > bottom);
        for i in 0..12 {
            let y = 441. + 55.6 * i as f32;
            assert!(y >= top && y <= bottom, "row {i} at {y}");
        }
    }

    #[test]
    fn a_guide_without_numbered_rows_bounds_nothing() {
        assert!(label_row_span(&json!({}), WritingFrame::Local).is_none());
        assert!(label_row_span(&json!({"rows":[]}), WritingFrame::Local).is_none());
        // One row cannot give a pitch, so it cannot give a span either.
        assert!(label_row_span(&json!({"rows":[row(100., 1)]}), WritingFrame::Local).is_none());
    }

    /// page-12 numbers one 24-word list across four columns. Each column's
    /// rows vouch for a quarter of it, so a list bounded by one guide's rows
    /// keeps a quarter; the span is every guide's rows together.
    #[test]
    fn a_list_numbered_across_columns_is_bounded_by_all_of_its_rows() {
        let top_half = json!({"rows":(1..=6).map(|i| row(100. + 50. * (i - 1) as f32, i)).collect::<Vec<_>>()});
        let bottom_half = json!({"rows":(7..=12).map(|i| row(400. + 50. * (i - 7) as f32, i)).collect::<Vec<_>>()});
        let alone = label_row_span(&top_half, WritingFrame::Local).unwrap();
        assert!(alone.1 < 400., "one guide's rows stop at its own last row");
        let union = [&top_half, &bottom_half]
            .into_iter()
            .filter_map(|v| label_row_span(v, WritingFrame::Local))
            .reduce(|(t, b), (t2, b2)| (t.min(t2), b.max(b2)))
            .unwrap();
        assert!(union.0 <= alone.0 && union.1 >= 625., "{union:?}");
    }

    #[test]
    fn nearly_twenty_four_rows_cannot_be_discarded_to_make_twelve() {
        let rows = |n| {
            (0..n)
                .map(|i| json!({"y":i as f32*50.,"dots":[]}))
                .collect::<Vec<_>>()
        };
        assert!(select_layout(&[rows(11), rows(12)]).is_none());
        assert!(select_layout(&[rows(12), rows(12)]).is_some());
    }

    #[test]
    fn a_spanning_detector_box_supplies_a_row_to_both_columns() {
        let mut all = Vec::new();
        let mut guides = Vec::new();
        for column in 0..2 {
            let mut ids = Vec::new();
            for row in 0..6 {
                if row == 1 {
                    continue;
                }
                ids.push(all.len());
                all.push(
                    Bounds {
                        l: column as f32 * 200. + 40.,
                        r: column as f32 * 200. + 160.,
                        t: row as f32 * 50.,
                        b: row as f32 * 50. + 30.,
                    }
                    .quad(),
                );
            }
            let bounds = ColumnBounds {
                intercept: column as f32 * 200. + 30.,
                slope: 0.,
                right: column as f32 * 200. + 190.,
                right_slope: 0.,
                top: 0.,
                bottom: 300.,
            };
            guides.push((bounds, json!({"word_layout_members":ids,"rows":[]})));
        }
        let spanning = all.len();
        all.push(
            Bounds {
                l: 40.,
                r: 360.,
                t: 50.,
                b: 80.,
            }
            .quad(),
        );
        reconcile(&mut guides, &all, WritingFrame::Local, &mut None);
        for (_, v) in &guides {
            assert_eq!(v["row_fit"]["accepted"], true);
            assert_eq!(v["rows"].as_array().unwrap().len(), 6);
            assert_eq!(v["rows"][1]["source_observations"], json!([spanning]));
        }
    }

    #[test]
    fn local_terminal_pitch_cannot_merge_two_observed_rows() {
        let ys = [
            240., 370., 477., 598., 710., 830., 950., 1070., 1210., 1350., 1470., 1620.,
        ];
        let mut words: Vec<_> = ys
            .iter()
            .map(|&y| {
                Bounds {
                    l: 100.,
                    r: 300.,
                    t: y - 35.,
                    b: y + 35.,
                }
                .quad()
            })
            .collect();
        // A detached unread ending belongs to the same ninth row.
        words.push(
            Bounds {
                l: 305.,
                r: 330.,
                t: 1180.,
                b: 1220.,
            }
            .quad(),
        );
        let c = ColumnBounds {
            right_slope: 0.,
            intercept: 80.,
            slope: 0.,
            right: 400.,
            top: 190.,
            bottom: 1670.,
        };
        let previous: Vec<_> = (2..=12)
            .map(|n| {
                let y = 1350. + (n as f32 - 10.) * 136.;
                json!({"ordinal":n,"line":[[80.,y],[400.,y]]})
            })
            .collect();
        let mut guides = vec![(
            c,
            json!({"rows":previous,"word_layout_members":(0..words.len()).collect::<Vec<_>>()}),
        )];
        reconcile(&mut guides, &words, WritingFrame::Local, &mut None);
        assert_eq!(guides[0].1["row_fit"]["accepted"], true);
        let rows = guides[0].1["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 12);
        assert_eq!(rows[2]["line"][0][1], 477.);
        assert_eq!(rows[3]["line"][0][1], 598.);
        assert_eq!(rows[8]["source_observations"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn twelve_is_total_not_per_column_and_partial_lists_are_not_stretched() {
        for (columns, height, accepted) in [
            (2, 6, true),
            (2, 12, true),
            (3, 4, true),
            (1, 8, false),
            (1, 11, false),
        ] {
            let mut words = Vec::new();
            let mut guides = Vec::new();
            for col in 0..columns {
                let first = words.len();
                let x = col as f32 * 300.;
                for row in 0..height {
                    let y = row as f32 * 100. + 50.;
                    words.push(
                        Bounds {
                            l: x + 40.,
                            r: x + 240.,
                            t: y - 25.,
                            b: y + 25.,
                        }
                        .quad(),
                    );
                }
                let c = ColumnBounds {
                    right_slope: 0.,
                    intercept: x + 20.,
                    slope: 0.,
                    right: x + 280.,
                    top: 0.,
                    bottom: height as f32 * 100.,
                };
                let old: Vec<_> = (0..height)
                    .map(|row| {
                        let y = row as f32 * 100. + 50.;
                        json!({"ordinal":row+1,"line":[[x+20.,y],[x+280.,y]]})
                    })
                    .collect();
                guides.push((c,json!({"rows":old,"word_layout_members":(first..words.len()).collect::<Vec<_>>()})));
            }
            reconcile(&mut guides, &words, WritingFrame::Local, &mut None);
            assert_eq!(guides[0].1["row_fit"]["accepted"], accepted);
            if accepted {
                assert_eq!(
                    guides
                        .iter()
                        .map(|(_, v)| v["rows"].as_array().unwrap().len())
                        .sum::<usize>(),
                    columns * height
                );
            } else {
                assert!(
                    guides
                        .iter()
                        .all(|(_, v)| v["rows"].as_array().unwrap().is_empty()),
                    "an invalid count must not keep enforcing the old rows"
                );
            }
        }
    }

    #[test]
    fn a_heading_is_not_a_seventh_row_and_supported_rows_cannot_be_dropped() {
        let row = |y, dot| json!({"y":y,"dots":if dot {vec![1]} else {vec![]}});
        let left = vec![
            row(-150., false),
            row(0., true),
            row(100., true),
            row(195., true),
            row(290., true),
            row(380., true),
            row(470., true),
        ];
        let right = vec![
            row(-10., true),
            row(80., true),
            row(170., true),
            row(260., true),
            row(345., true),
            row(450., true),
        ];
        assert_eq!(
            select_layout(&[left.clone(), right.clone()]),
            Some(vec![1..7, 0..6])
        );
        let mut supported = left;
        supported[0]["dots"] = json!([2]);
        assert_eq!(select_layout(&[supported, right]), None);
    }
}
