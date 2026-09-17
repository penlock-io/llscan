//! Discover label tracks from the raster, independently of OCR-derived guides.
use super::*;
use crate::{dot_grid, layout::PageLayout};
use image::buffer::ConvertBuffer;
use image::imageops::FilterType;
use serde_json::Value;

// Test the actual vertical clamp used by the shared crop owner, not merely
// the ideal sloping line. A clean row-body interval alone is insufficient.
fn crosses_ink(
    ink: &[Vec<bool>],
    frame: Frame,
    column: ColumnBounds,
    writing: WritingFrame,
    word: Bounds,
) -> bool {
    let x = column.left_limit(word);
    let band = 0.02 * word.h();
    let h = ink.len();
    let w = ink.first().map_or(0, Vec::len);
    ink.iter().enumerate().any(|(y, row)| {
        row.iter().enumerate().any(|(xx, &dark)| {
            if !dark {
                return false;
            }
            let (dx, dy) = turn(
                xx as f32 + 0.5 - w as f32 / 2.,
                y as f32 + 0.5 - h as f32 / 2.,
                frame.angle,
            );
            let px = writing
                .project(&Quad([(frame.cx + dx, frame.cy + dy); 4]))
                .0[0]
                .0;
            (px - x).abs() <= band
        })
    })
}

fn boundary_reason(c: ColumnBounds, isolated: usize) -> &'static str {
    if isolated < 2 {
        "insufficient_isolated_marks"
    } else if !c.intercept.is_finite()
        || !c.slope.is_finite()
        || c.right <= c.left(c.top).max(c.left(c.bottom))
    {
        "invalid_column_geometry"
    } else {
        "applied"
    }
}

fn compact_prefix(mark: Bounds, word: Bounds, height: f32) -> bool {
    let gap = word.l - mark.r;
    mark.h() >= 0.25 * height
        && mark.h() <= height
        && mark.w() >= 0.12 * height
        && mark.w() <= 1.5 * mark.h()
        && gap >= 0.12 * height
        && gap <= 1.5 * height
        && mark.t < word.b
        && mark.b > word.t
}

// The centre track locates labels; their measured edges determine the crop line.
// Raw connected-component bounds measure ink extent; a compact-peak window
// only locates a response and must not masquerade as the extent of a glyph.
fn word_edge(
    track: &Value,
    candidates: &[Value],
    angle: f32,
    _scale: f32,
) -> (f32, f32, Vec<Value>) {
    let mut slope = track["slope"].as_f64().unwrap() as f32;
    let centre = track["intercept"].as_f64().unwrap() as f32;
    let measured: Vec<_> = track["supports"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| {
            let id = s["candidate"].as_u64()?;
            let c = candidates
                .iter()
                .find(|c| c["id"].as_u64() == Some(id) && c["origin"] == "raw_component")?;
            let [l, t, r, b]: [f32; 4] = serde_json::from_value(c["bounds"].clone()).ok()?;
            let corners = [(l, t), (r, t), (r, b), (l, b)].map(|(x, y)| turn(x, y, -angle));
            Some((id, [l, t, r, b], corners))
        })
        .collect();
    // Detection may be supported by approximate peaks, but only measured
    // glyph edges determine the boundary's direction. In particular, letter
    // corners must not pivot a line extrapolated from two actual dots.
    if measured.len() >= 2 {
        let points: Vec<_> = measured
            .iter()
            .map(|(_, _, q)| {
                let b = Bounds::of(&Quad(*q));
                (b.r, b.y())
            })
            .collect();
        let mx = points.iter().map(|p| p.0).sum::<f32>() / points.len() as f32;
        let my = points.iter().map(|p| p.1).sum::<f32>() / points.len() as f32;
        slope = points.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum::<f32>()
            / points.iter().map(|p| (p.1 - my).powi(2)).sum::<f32>();
    }
    let edges: Vec<_> = measured
        .iter()
        .map(|(id, b, q)| {
            let intercept = q
                .iter()
                .map(|(x, y)| x - slope * y)
                .fold(f32::NEG_INFINITY, f32::max);
            json!({"candidate":id,"photo_bounds":b,"right_edge_intercept":intercept})
        })
        .collect();
    let boundary = if edges.len() >= 2 {
        // Component maxima are already exclusive pixel bounds. An additional
        // raster-pixel guard double-expands them into neighbouring word ink.
        edges
            .iter()
            .map(|e| e["right_edge_intercept"].as_f64().unwrap() as f32)
            .fold(f32::NEG_INFINITY, f32::max)
    } else {
        centre
    };
    (boundary, slope, edges)
}

fn gutter_growth(track: &Value, intercept: f32, slope: f32, span: [f32; 2]) -> f32 {
    span.into_iter()
        .map(|y| {
            intercept - track["intercept"].as_f64().unwrap() as f32
                + (slope - track["slope"].as_f64().unwrap() as f32) * y
        })
        .fold(0., f32::max)
}

pub(super) fn reconcile(
    photo: &RgbImage,
    all: &[Quad],
    writing: WritingFrame,
    guides: &mut Vec<(ColumnBounds, Value)>,
    decisions: &mut Option<DecisionTrace>,
    anchored: bool,
) {
    if !writing.shared() {
        return;
    }
    let _time = crate::timing::span("phrase.label_grid.dot_heatmap");
    let angle = writing.angle_or(0.);
    // Raster probes rotate about photo (0,0); the native writing frame also
    // translates to its anchor centre. Keep both coordinate systems explicit.
    let projected: Vec<_> = all
        .iter()
        .map(|q| Quad(q.0.map(|(x, y)| turn(x, y, -angle))))
        .collect();
    let boxes: Vec<_> = all
        .iter()
        .map(|q| Bounds::of(&writing.project(q)))
        .collect();
    let origin = writing.unproject(&Quad([(0., 0.); 4])).0[0];
    let (ox, oy) = turn(origin.0, origin.1, -angle);
    let mut hs: Vec<_> = boxes
        .iter()
        .filter(|b| b.w() >= 1.5 * b.h())
        .map(|b| b.h())
        .collect();
    if hs.len() < 2 {
        return;
    }
    hs.sort_by(f32::total_cmp);
    let height = hs[hs.len() / 2];
    let scale = (1800. / photo.width().max(photo.height()) as f32).min(1.);
    // No tiny-text/background heatmap sweep. Bounds the search resolution and
    // keeps this pass within the scale for which its raster rules were tested.
    if !(12. ..=400.).contains(&(height * scale)) {
        return;
    }
    let original_gray: GrayImage = photo.convert();
    let gray = image::imageops::resize(
        &original_gray,
        (photo.width() as f32 * scale).round() as u32,
        (photo.height() as f32 * scale).round() as u32,
        FilterType::Triangle,
    );
    let (mask, _, blobs) = dot_grid::dots(&gray, height * scale);
    let candidates: Vec<_> = blobs
        .iter()
        .enumerate()
        .map(|(id, b)| json!({"id":id,"bounds":b.bounds.map(|v|v as f32/scale),"origin":b.origin}))
        .collect();
    let rects: Vec<_> = projected
        .iter()
        .map(|q| {
            let b = Bounds::of(q);
            [b.l, b.t, b.r, b.b]
        })
        .collect();
    let layout = PageLayout::new(&projected);
    let fitted = dot_grid::tracks::analyse(&mask, angle, scale, &rects, &layout, &candidates);
    let mut applied = false;
    let mut claimed = std::collections::HashSet::new();
    for column in fitted["columns"].as_array().unwrap() {
        let members: Vec<usize> =
            serde_json::from_value(column["word_observations"].clone()).unwrap();
        let tracks = column["tracks"].as_array().unwrap();
        if tracks.len() != 1 {
            DecisionTrace::push(decisions, || {
                json!({
                    "rule":"grid_dot_reconciliation","status":"evaluated",
                    "reason":if tracks.is_empty() {"no_dot_track"} else {"ambiguous_dot_tracks"},
                    "word_layout_members":members,"candidate_analysis":column,
                    "boundary_policy":"retain_label_grid_without_unique_dot_track","extra_ocr_calls":0
                })
            });
            continue;
        }
        let track = &tracks[0];
        let supports: Vec<_> = track["supports"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                let mut p = p.clone();
                p["isolation"] = dot_grid::isolation::assess(
                    &mask,
                    angle,
                    scale,
                    p["centre"][0].as_f64().unwrap() as f32,
                    p["centre"][1].as_f64().unwrap() as f32,
                    column["height"].as_f64().unwrap() as f32,
                );
                p
            })
            .collect();
        let isolated = supports
            .iter()
            .filter(|p| p["isolation"]["isolated"] == true)
            .count();
        // The fitted direction and measured dot edges define the boundary. Gap availability,
        // distance from an old estimate, and undifferentiated ink crossings
        // are diagnostics, not authority to keep the old line instead.
        let (edge_intercept, slope, measured_edges) = word_edge(track, &candidates, angle, scale);
        let top = members
            .iter()
            .map(|&i| boxes[i].t)
            .reduce(f32::min)
            .unwrap();
        let bottom = members
            .iter()
            .map(|&i| boxes[i].b)
            .reduce(f32::max)
            .unwrap();
        let h = column["height"].as_f64().unwrap() as f32;
        let proposed = ColumnBounds {
            intercept: edge_intercept + slope * oy - ox,
            slope,
            top,
            bottom,
            right: members
                .iter()
                .map(|&i| boxes[i].r)
                .reduce(f32::max)
                .unwrap(),
            right_slope: 0.,
        };
        let middle = (top + bottom) / 2.;
        // Match locations, not just IDs. An old guide for a different physical
        // column must never lend its trailing edge to this dot track.
        let starts = median(members.iter().map(|&i| boxes[i].l).collect());
        let pitch = fitted["columns"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|other| {
                let ids: Vec<usize> =
                    serde_json::from_value(other["word_observations"].clone()).ok()?;
                let x = median(ids.iter().map(|&i| boxes[i].l).collect());
                let distance = (x - starts).abs();
                (distance > h).then_some(distance)
            })
            .reduce(f32::min)
            .unwrap_or(f32::INFINITY);
        let index = layout::guide_owner(
            guides,
            &claimed,
            &members,
            proposed.left(middle),
            middle,
            h,
            pitch,
        );
        let old = index.map(|i| guides[i].0).unwrap_or(proposed);
        let c = ColumnBounds {
            top: old.top.min(top),
            bottom: old.bottom.max(bottom),
            ..proposed
        };
        let full_members = members.clone();
        let label_supports = supports
            .iter()
            .filter(|s| {
                s["isolation"]["isolated"] == true
                    && (!s["gap"].is_null()
                        || column["separate_label_regions"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|r| {
                                r["observation"] == s["observation"]
                                    && r["regions"].as_array().is_some_and(|v| !v.is_empty())
                            }))
            })
            .count();
        // Small printed numbers can be isolated as whole components rather
        // than as separate periods. They need label-sized ink BEFORE the word
        // and a substantial gap. Repeated speckles outside an unnumbered list
        // are not enough, however straight their track may be.
        let compact_prefixes = supports
            .iter()
            .filter(|s| s["isolation"]["isolated"] == true)
            .filter_map(|s| {
                let c = candidates
                    .iter()
                    .find(|c| c["id"] == s["candidate"] && c["origin"] == "raw_component")?;
                let [l, t, r, b]: [f32; 4] = serde_json::from_value(c["bounds"].clone()).ok()?;
                let mark = Bounds::of(&Quad(
                    [(l, t), (r, t), (r, b), (l, b)].map(|(x, y)| turn(x, y, -angle)),
                ));
                let i = s["observation"].as_u64()? as usize;
                let word = Bounds::of(&projected[i]);
                compact_prefix(mark, word, h).then_some(i)
            })
            .collect::<std::collections::HashSet<_>>();
        // A dot track may establish a label column the recogniser could not
        // read, but only on a page that read a number somewhere: the marks
        // themselves cannot say whether they are labels. An i-dot is a small
        // isolated compact mark in exactly the way a list dot is, and on a page
        // with no numbers at all two of them are enough to draw a gutter
        // through the writing they sit in.
        let reason = if index.is_none() && !anchored {
            "isolated_marks_without_any_label_read_on_the_page"
        } else if index.is_none() && label_supports < 2 && compact_prefixes.len() < 2 {
            "isolated_marks_without_repeated_label_separation"
        } else {
            boundary_reason(c, isolated)
        };
        let mut crossing = Vec::new();
        if reason == "applied" && decisions.is_some() {
            for &i in &full_members {
                let q = &all[i];
                let b = boxes[i];
                let f = writing.crop_frame(q, 0.);
                let crop = level_crop_in(photo, f);
                let mut ink = source_ink(photo, q, f, &crop);
                strip_rules(&mut ink);
                if crosses_ink(&ink, f, c, writing, b) {
                    crossing.push(i);
                }
            }
        }
        let diagnostic = json!({"rule":"grid_dot_reconciliation","status":"evaluated","reason":reason,
            "word_layout_members":full_members,"track":track,"supports":supports,"isolated_marks":isolated,
            "label_separation_supports":label_supports,"bootstrap_without_ocr":index.is_none(),
            "compact_leading_component_rows":compact_prefixes.iter().copied().collect::<std::collections::BTreeSet<_>>(),
            "previous_quad":writing.unproject(&old.quad()).0,"proposed_quad":writing.unproject(&c.quad()).0,
            "crossing_observations":crossing,"ink_crossings_diagnostic_only":true,"boundary_policy":if measured_edges.len()>=2 {"measured_dot_edge_fit"}else{"centre_track_without_measured_edges"},
            "word_edge_intercept":edge_intercept,"word_edge_slope":slope,"measured_dot_edges":measured_edges,"raster_guard_px":0.,"edge_convention":"exclusive_component_pixel_bounds",
            "track_frame":"photo_origin_rotated_by_negative_writing_angle","native_origin_in_track_frame":[ox,oy],"geometry_preserved_independently_of_dot_identity":true,"extra_ocr_calls":0});
        DecisionTrace::push(decisions, || diagnostic.clone());
        if reason != "applied" {
            continue;
        }
        let index = index.unwrap_or_else(|| {
            let i = guides.len();
            guides.push((
                c,
                json!({"rows":[],"label_gutter_width":h,
                "observed_y_span":[top,bottom],"supports":isolated,"estimated":false,
                "display_only":false,"ordinal_inferred":false}),
            ));
            i
        });
        claimed.insert(index);
        let (boundary, v) = &mut guides[index];
        v["word_layout_members"] = json!(members);
        v["group_assignment"] = json!("unique_spatial_dot_owner");
        v["dot_reconciliation"] = diagnostic;
        if reason == "applied" {
            *boundary = c;
            // Extending exclusion past the dot's centre grows its gutter;
            // it must not translate the entire gutter and expose old label
            // ink to the preceding word column.
            // A refined direction can change clearance with height. Cover
            // the old label region over the FULL column, not just one row.
            let growth = gutter_growth(track, edge_intercept, slope, [c.top + oy, c.bottom + oy]);
            v["label_gutter_width"] =
                json!(v["label_gutter_width"].as_f64().unwrap() + growth as f64);
            v["method"] = json!("dot_track_boundary");
            applied = true;
        }
    }
    if !applied {
        return;
    }
    guides.sort_by(|(a, _), (b, _)| {
        a.left((a.top + a.bottom) / 2.)
            .total_cmp(&b.left((b.top + b.bottom) / 2.))
    });
    refresh_trailing_limits(guides, writing);
}

fn refresh_trailing_limits(guides: &mut [(ColumnBounds, Value)], writing: WritingFrame) {
    // Keep neighbouring label gutters excluded after a leading line moves.
    // This is the same grid consumed by the common crop owner and the report.
    for i in 0..guides.len() {
        let next = guides
            .get(i + 1)
            .map(|(c, v)| (*c, v["label_gutter_width"].as_f64().unwrap() as f32));
        let (c, v) = &mut guides[i];
        if let Some((next, w)) = next {
            // Recompute from the current neighbour, not a minimum with the
            // obsolete pre-dot boundary. Otherwise a moved label line leaves
            // an invisible old clamp which cuts off cross-column words.
            c.right = next.left(c.top).min(next.left(c.bottom)) - w;
        }
        v["quad"] = json!(writing.unproject(&c.quad()).0);
        if let Some(rows) = v["rows"].as_array_mut() {
            for row in rows {
                let line: [(f32, f32); 2] = serde_json::from_value(row["line"].clone()).unwrap();
                let y = writing.project(&Quad([line[0]; 4])).0[0].1;
                let q = writing.unproject(&Quad([
                    (c.left(y), y),
                    (c.right, y),
                    (c.right, y),
                    (c.left(y), y),
                ]));
                row["line"] = json!([q.0[0], q.0[1]]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compact_numbers_need_a_gap_before_the_word_not_just_alignment() {
        let word = Bounds {
            l: 260.,
            r: 540.,
            t: 240.,
            b: 330.,
        };
        let mark = Bounds {
            l: 200.,
            r: 223.,
            t: 273.,
            b: 299.,
        };
        assert!(compact_prefix(mark, word, 80.));
        assert!(!compact_prefix(
            Bounds {
                l: mark.l,
                r: mark.l + 5.,
                t: mark.t,
                b: mark.t + 5.
            },
            word,
            80.
        ));
        assert!(!compact_prefix(
            Bounds {
                l: 260.,
                r: 283.,
                ..mark
            },
            word,
            80.
        ));
        assert!(!compact_prefix(
            Bounds {
                t: 100.,
                b: 126.,
                ..mark
            },
            word,
            80.
        ));
        assert!(!compact_prefix(mark, Bounds { l: 227., ..word }, 80.));
    }
    #[test]
    fn gutter_growth_covers_old_label_region_when_direction_changes() {
        let track = json!({"intercept":80.,"slope":0.02});
        for slope in [-0.1, 0., 0.1] {
            let growth = gutter_growth(&track, 90., slope, [100., 600.]);
            for y in [100., 300., 600.] {
                assert!(90. + slope * y - (20. + growth) <= 80. + 0.02 * y - 20. + 0.001);
            }
        }
    }
    #[test]
    fn boundary_clears_raw_dot_extents_not_centres_or_peak_windows() {
        for angle in [0., 30.] {
            let candidates = vec![
                json!({"id":0,"origin":"raw_component","bounds":[100.,100.,110.,110.]}),
                json!({"id":1,"origin":"raw_component","bounds":[100.,200.,114.,215.]}),
                json!({"id":2,"origin":"compact_dark_peak","bounds":[100.,300.,200.,400.]}),
            ];
            let slope = -(angle as f32).to_radians().tan();
            let track = json!({"slope":slope,"intercept":0.,"supports":[{"candidate":0},{"candidate":1},{"candidate":2}]});
            let (boundary, slope, edges) = word_edge(&track, &candidates, angle, 0.5);
            let mut tilted = track.clone();
            tilted["slope"] = json!(0.25);
            tilted["intercept"] = json!(300.);
            let corrected = word_edge(&tilted, &candidates, angle, 0.5);
            assert_eq!((boundary, slope), (corrected.0, corrected.1));
            assert_eq!(edges.len(), 2);
            for c in &candidates[..2] {
                let [l, t, r, b]: [f32; 4] = serde_json::from_value(c["bounds"].clone()).unwrap();
                for (x, y) in [(l, t), (r, t), (r, b), (l, b)] {
                    let (x, y) = turn(x, y, -angle);
                    assert!(boundary + slope * y >= x - 0.001);
                }
            }
            assert!(boundary < 150.); // the peak window at x=200 never steers the edge
        }
        let track = json!({"slope":0.,"intercept":105.,"supports":[{"candidate":0}]});
        assert_eq!(
            word_edge(
                &track,
                &[json!({"id":0,"origin":"compact_dark_peak","bounds":[100.,100.,140.,140.]})],
                0.,
                1.
            )
            .0,
            105.
        );
    }
    #[test]
    fn rejects_nonfinite_or_width_inverting_track_geometry() {
        let c = ColumnBounds {
            right_slope: 0.,
            intercept: 10.,
            slope: 0.,
            right: 100.,
            top: 0.,
            bottom: 200.,
        };
        assert_eq!(boundary_reason(c, 2), "applied");
        for c in [
            ColumnBounds {
                intercept: f32::NAN,
                ..c
            },
            ColumnBounds {
                slope: f32::INFINITY,
                ..c
            },
            ColumnBounds { slope: 1., ..c },
        ] {
            assert_eq!(boundary_reason(c, 2), "invalid_column_geometry");
        }
    }
    #[test]
    fn moved_neighbour_replaces_stale_trailing_limit() {
        let first = ColumnBounds {
            right_slope: 0.,
            intercept: 20.,
            slope: 0.,
            right: 150.,
            top: 0.,
            bottom: 200.,
        };
        let next = ColumnBounds {
            right_slope: 0.,
            intercept: 250.,
            slope: 0.1,
            right: 400.,
            top: 0.,
            bottom: 200.,
        };
        let mut guides = vec![
            (first, json!({"label_gutter_width":30.,"rows":[]})),
            (next, json!({"label_gutter_width":40.,"rows":[]})),
        ];
        refresh_trailing_limits(&mut guides, WritingFrame::Local);
        assert_eq!(guides[0].0.right, 210.);
        assert_eq!(guides[0].1["quad"][1][0], 210.);
        assert_eq!(guides[1].0.right, 400.);
        // Dot-edge clearance grows the label region to the right. Its old
        // left edge remains illegal, including for preceding-column pieces.
        guides[1].0.intercept += 12.;
        guides[1].1["label_gutter_width"] = json!(52.);
        refresh_trailing_limits(&mut guides, WritingFrame::Local);
        assert_eq!(guides[0].0.right, 210.);
    }
    #[test]
    fn fitted_boundary_replaces_far_estimate_even_when_ink_crosses() {
        let mut photo = RgbImage::from_pixel(600, 650, image::Rgb([230; 3]));
        let mut all = Vec::new();
        for y in [100, 200, 300, 400] {
            all.push(
                Bounds {
                    l: 140.,
                    r: 320.,
                    t: y as f32,
                    b: y as f32 + 60.,
                }
                .quad(),
            );
            for (l, r, t, b) in [
                (145, 155, y + 12, y + 50),
                (164, 172, y + 48, y + 56),
                (200, 300, y + 10, y + 50),
            ] {
                for yy in t..b {
                    for x in l..r {
                        photo.put_pixel(x, yy, image::Rgb([20; 3]));
                    }
                }
            }
        }
        let writing =
            WritingFrame::Page(crate::page_frame::PageAxis::from_detections(&all).direction(false));
        let origin = writing.unproject(&Quad([(0., 0.); 4])).0[0];
        let old = ColumnBounds {
            right_slope: 0.,
            intercept: 250. - origin.0,
            slope: 0.,
            right: 320. - origin.0,
            top: 100. - origin.1,
            bottom: 460. - origin.1,
        };
        let mut guides = vec![(
            old,
            json!({"word_layout_members":[0,1,2,3],"label_gutter_width":60.,"rows":[]}),
        )];
        let mut trace = Some(DecisionTrace::default());
        reconcile(&photo, &all, writing, &mut guides, &mut trace, true);
        assert_eq!(guides[0].1["dot_reconciliation"]["reason"], "applied");
        assert_eq!(guides[0].1["dot_reconciliation"]["status"], "evaluated");
        let cut = guides[0].0.intercept + origin.0;
        assert!((172. ..177.).contains(&cut), "{cut}");
        // Ink crossing the fitted line is diagnostic only; it cannot restore
        // an old boundary or exempt a word from the column clamp.
        for y in 103..108 {
            for x in 165..216 {
                photo.put_pixel(x, y, image::Rgb([20; 3]));
            }
        }
        let mut guarded = vec![(
            old,
            json!({"word_layout_members":[0,1,2,3],"label_gutter_width":60.,"rows":[]}),
        )];
        let mut trace = Some(DecisionTrace::default());
        reconcile(&photo, &all, writing, &mut guarded, &mut trace, true);
        assert_eq!(guarded[0].1["dot_reconciliation"]["reason"], "applied");
        assert!(
            !guarded[0].1["dot_reconciliation"]["crossing_observations"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert!((guarded[0].0.intercept - guides[0].0.intercept).abs() < 2.);
        // A page that read a number somewhere can still find its gutter from
        // the geometry: the dots need not be readable themselves.
        let mut empty = Vec::new();
        reconcile(&photo, &all, writing, &mut empty, &mut None, true);
        assert_eq!(empty.len(), 1);
        assert_eq!(empty[0].1["method"], "dot_track_boundary");
        // A page that read no number at all may not bootstrap one from marks.
        // An i-dot is isolated and compact in exactly the way a list dot is,
        // and page-29 drew a gutter through its own words from two of them.
        let mut unlabelled = Vec::new();
        reconcile(&photo, &all, writing, &mut unlabelled, &mut None, false);
        assert!(unlabelled.is_empty());
    }
    #[test]
    fn clearance_uses_translated_rotated_page_coordinates() {
        let origin = (350., 250.);
        let angle = 35.;
        let q = Quad(
            [(-100., -50.), (100., -50.), (100., 50.), (-100., 50.)].map(|(x, y)| {
                let (x, y) = turn(x, y, angle);
                (x + origin.0, y + origin.1)
            }),
        );
        let writing =
            WritingFrame::Page(crate::page_frame::PageAxis::from_detections(&[q]).direction(false));
        let f = Frame {
            cx: 350.,
            cy: 250.,
            w: 100.,
            h: 100.,
            angle: writing.angle_or(0.),
        };
        let b = Bounds {
            l: -50.,
            r: 50.,
            t: -50.,
            b: 50.,
        };
        let c = ColumnBounds {
            right_slope: 0.,
            intercept: -30.,
            slope: 0.,
            right: 50.,
            top: -50.,
            bottom: 50.,
        };
        let mut ink = vec![vec![false; 100]; 100];
        ink[0][20] = true;
        assert!(crosses_ink(&ink, f, c, writing, b));
    }
    #[test]
    fn clearance_checks_top_and_bottom_ink_and_actual_clamp() {
        let f = Frame {
            cx: 50.,
            cy: 50.,
            w: 100.,
            h: 100.,
            angle: 0.,
        };
        let c = ColumnBounds {
            right_slope: 0.,
            intercept: 10.,
            slope: 0.1,
            right: 100.,
            top: 0.,
            bottom: 100.,
        };
        let b = Bounds {
            l: 0.,
            r: 100.,
            t: 0.,
            b: 100.,
        };
        let mut ink = vec![vec![false; 100]; 100];
        assert!(!crosses_ink(&ink, f, c, WritingFrame::Local, b));
        ink[0][20] = true;
        assert!(crosses_ink(&ink, f, c, WritingFrame::Local, b));
        ink[0][20] = false;
        ink[99][20] = true;
        assert!(crosses_ink(&ink, f, c, WritingFrame::Local, b));
    }
}
