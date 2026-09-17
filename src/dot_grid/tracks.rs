//! Model-free dot-track evidence. The native grid separately authorises cuts.
use crate::{layout::PageLayout, split::turn};
use image::GrayImage;
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[derive(Clone, Debug)]
struct Support {
    candidate: usize,
    row: usize,
    x: f32,
    y: f32,
    gap: Option<[f32; 2]>,
    raw: bool,
}

// A standalone narrow observation immediately before a word is independent
// label-location evidence. When present, dots belong to that region, not to
// threshold islands inside the separate word. No spelling or old fitted line
// participates. Merged-label words have no such region and retain the broad
// proposal search used for their attached/ruling-connected dots.
fn separate_labels(word: usize, boxes: &[[f32; 4]]) -> Vec<[f32; 4]> {
    let b = boxes[word];
    let h = b[3] - b[1];
    boxes
        .iter()
        .enumerate()
        .filter_map(|(i, &a)| {
            let ah = a[3] - a[1];
            let aw = a[2] - a[0];
            (i != word
                && ah >= 0.3 * h
                && ah <= 1.5 * h
                && aw <= 1.5 * ah
                && a[2] <= b[0] + 0.1 * h
                && a[2] >= b[0] - 1.5 * h
                && a[1] < b[3]
                && a[3] > b[1]
                && ((a[1] + a[3] - b[1] - b[3]) / 2.).abs() <= 0.65 * h)
                .then_some(a)
        })
        .collect()
}

fn in_label_region(x: f32, y: f32, h: f32, labels: &[[f32; 4]]) -> bool {
    labels.iter().any(|b| {
        x >= b[0] - 0.1 * h && x <= b[2] + 0.1 * h && y >= b[1] - 0.1 * h && y <= b[3] + 0.1 * h
    })
}

// Sample a row-body profile in writing coordinates. The dot and baseline lie
// below this band. An interval must contain measured whitespace followed by
// body ink, not just the approximate right edge of a heatmap window.
fn corridor(mask: &GrayImage, angle: f32, scale: f32, x: f32, y: f32, h: f32) -> Option<[f32; 2]> {
    let step = 1. / scale;
    let occupied = |at: f32| {
        let mut dark = 0;
        let mut valid = 0;
        let mut v = y - 0.65 * h;
        while v < y - 0.15 * h {
            let (px, py) = turn(at, v, angle);
            let (px, py) = ((px * scale).round() as i32, (py * scale).round() as i32);
            if px >= 0 && py >= 0 && px < mask.width() as i32 && py < mask.height() as i32 {
                valid += 1;
                dark += usize::from(mask.get_pixel(px as u32, py as u32)[0] == 0);
            }
            v += step;
        }
        valid > 0 && dark as f32 / valid as f32 >= 0.12
    };
    // A period without any preceding upper/body ink could be background or
    // isolated punctuation. Do not count it as an independently supported label.
    let preceding = (1..=(0.85 * h / step) as usize)
        .filter(|&i| occupied(x - i as f32 * step))
        .count();
    if (preceding as f32) * step < 0.08 * h {
        return None;
    }
    let mut quiet_start = None;
    let mut ink_start = None;
    let mut at = x + 0.08 * h;
    while at <= x + 1.2 * h {
        if occupied(at) {
            if let Some(start) = quiet_start {
                let ink = *ink_start.get_or_insert(at);
                if at - ink >= 0.04 * h {
                    return (ink - start >= 0.12 * h).then_some([start, ink]);
                }
            }
        } else if ink_start.is_some() {
            // A tiny isolated stroke is not the beginning of a stable word.
            quiet_start = Some(at);
            ink_start = None;
        } else {
            quiet_start.get_or_insert(at);
        }
        at += step;
    }
    None
}

fn fit(points: &[Support], h: f32, row_count: usize, span: f32) -> Vec<Value> {
    let mut hypotheses: Vec<(usize, f32, f32, Vec<usize>, f32)> = Vec::new();
    let mut seen = BTreeSet::new();
    let raw_rows: BTreeSet<_> = points.iter().filter(|p| p.raw).map(|p| p.row).collect();
    for (i, a) in points.iter().enumerate() {
        for b in &points[i + 1..] {
            // Detached compact components are stronger location evidence than
            // heatmap letter corners. When available, use them to seed the
            // line; attached-dot peaks complete it rather than pivoting it.
            if raw_rows.len() >= 2 && !(a.raw && b.raw) {
                continue;
            }
            if a.row == b.row || (a.y - b.y).abs() < h {
                continue;
            }
            let s = (a.x - b.x) / (a.y - b.y);
            if s.abs() > 0.3 {
                continue;
            }
            let intercept = a.x - s * a.y;
            let mut ranked: Vec<_> = points
                .iter()
                .enumerate()
                .map(|(i, p)| (i, (p.x - intercept - s * p.y).abs()))
                .filter(|p| p.1 <= 0.12 * h)
                .collect();
            ranked.sort_by(|a, b| {
                points[b.0]
                    .raw
                    .cmp(&points[a.0].raw)
                    .then_with(|| a.1.total_cmp(&b.1))
            });
            let mut rows = BTreeSet::new();
            let mut row_y: Vec<f32> = Vec::new();
            let mut ids: Vec<_> = ranked
                .into_iter()
                .filter_map(|(i, _)| {
                    if rows.contains(&points[i].row)
                        || row_y.iter().any(|&y| (y - points[i].y).abs() < 0.5 * h)
                    {
                        return None;
                    }
                    rows.insert(points[i].row);
                    row_y.push(points[i].y);
                    Some(i)
                })
                .collect();
            ids.sort_unstable();
            if ids.len() < 2 || ids.len() * 2 < row_count || !seen.insert(ids.clone()) {
                continue;
            }
            let lo = ids
                .iter()
                .map(|&i| points[i].y)
                .fold(f32::INFINITY, f32::min);
            let hi = ids
                .iter()
                .map(|&i| points[i].y)
                .fold(f32::NEG_INFINITY, f32::max);
            if hi - lo < 0.5 * span {
                continue;
            }
            let mean_y = ids.iter().map(|&i| points[i].y).sum::<f32>() / ids.len() as f32;
            let mean_x = ids.iter().map(|&i| points[i].x).sum::<f32>() / ids.len() as f32;
            let slope = ids
                .iter()
                .map(|&i| (points[i].y - mean_y) * (points[i].x - mean_x))
                .sum::<f32>()
                / ids
                    .iter()
                    .map(|&i| (points[i].y - mean_y).powi(2))
                    .sum::<f32>();
            let intercept = mean_x - slope * mean_y;
            if slope.abs() > 0.3
                || ids
                    .iter()
                    .any(|&i| (points[i].x - intercept - slope * points[i].y).abs() > 0.12 * h)
            {
                continue;
            }
            let score = ids.iter().map(|&i| if points[i].raw { 4 } else { 1 }).sum();
            let error = ids
                .iter()
                .map(|&i| (points[i].x - intercept - slope * points[i].y).powi(2))
                .sum::<f32>()
                / ids.len() as f32;
            hypotheses.push((score, intercept, slope, ids, error));
        }
    }
    hypotheses.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.4.total_cmp(&b.4)));
    // Retain distinct competitive tracks. Different choices of a peak on the
    // same dot are not independent alternatives; separate lines are.
    let mut kept: Vec<Value> = Vec::new();
    let best = hypotheses.first().map_or(0, |v| v.0);
    for (score, a, s, ids, error) in hypotheses {
        if (score as f32) < best as f32 * 0.85 {
            continue;
        }
        let lo = ids
            .iter()
            .map(|&i| points[i].y)
            .fold(f32::INFINITY, f32::min);
        let hi = ids
            .iter()
            .map(|&i| points[i].y)
            .fold(f32::NEG_INFINITY, f32::max);
        if kept.iter().any(|t| {
            let aa = t["intercept"].as_f64().unwrap() as f32;
            let ss = t["slope"].as_f64().unwrap() as f32;
            [lo, hi]
                .iter()
                .all(|&y| ((aa + ss * y) - (a + s * y)).abs() < 0.25 * h)
        }) {
            continue;
        }
        // A separator parallel to the track must be inside every measured gap.
        // Empty intersection means a line exists, but no safe shared corridor.
        let left = ids
            .iter()
            .filter_map(|&i| points[i].gap.map(|g| g[0] - s * points[i].y))
            .fold(f32::NEG_INFINITY, f32::max);
        let right = ids
            .iter()
            .filter_map(|&i| points[i].gap.map(|g| g[1] - s * points[i].y))
            .fold(f32::INFINITY, f32::min);
        let gap_count = ids.iter().filter(|&&i| points[i].gap.is_some()).count();
        kept.push(json!({"intercept":a,"slope":s,"support_count":ids.len(),"raw_support_count":ids.iter().filter(|&&i|points[i].raw).count(),"score":score,"rms_residual":error.sqrt(),"gap_count":gap_count,"observed_y_span":[lo,hi],
            "separator_intercept_interval":(gap_count>=2 && right-left >= 0.04*h).then_some([left,right]),
            "supports":ids.iter().map(|&i| {let p=&points[i];json!({"candidate":p.candidate,"observation":p.row,"centre":[p.x,p.y],"gap":p.gap})}).collect::<Vec<_>>() }));
    }
    kept
}

pub fn analyse(
    mask: &GrayImage,
    angle: f32,
    scale: f32,
    boxes: &[[f32; 4]],
    layout: &PageLayout,
    candidates: &[Value],
) -> Value {
    let mut columns = Vec::new();
    for (column, members) in layout.columns.iter().enumerate() {
        let Some((words, h)) = layout.dominant_word_block(members, 1.5) else {
            continue;
        };
        let top = words
            .iter()
            .map(|&i| boxes[i][1])
            .fold(f32::INFINITY, f32::min);
        let bottom = words
            .iter()
            .map(|&i| boxes[i][3])
            .fold(f32::NEG_INFINITY, f32::max);
        let mut points = Vec::new();
        let mut rejected = Vec::new();
        let labels: Vec<_> = words.iter().map(|&i| separate_labels(i, boxes)).collect();
        for c in candidates {
            let b: [f32; 4] = serde_json::from_value(c["bounds"].clone()).unwrap();
            let (x, y) = turn((b[0] + b[2]) / 2., (b[1] + b[3]) / 2., -angle);
            for (&i, label_regions) in words.iter().zip(&labels) {
                let b = boxes[i];
                let rh = b[3] - b[1];
                if x < b[0] - 1.5 * rh
                    || x > b[0] + 1.5 * rh
                    || y < b[1] - 0.3 * rh
                    || y > b[3] + 0.3 * rh
                {
                    continue;
                }
                let id = c["id"].as_u64().unwrap() as usize;
                if !label_regions.is_empty() && !in_label_region(x, y, rh, label_regions) {
                    rejected.push(json!({"candidate":id,"observation":i,"reason":"outside_separate_label_region"}));
                    continue;
                }
                if label_regions.is_empty() && y < b[1] + 0.4 * rh {
                    continue;
                }
                let gap = corridor(mask, angle, scale, x, y, rh);
                points.push(Support {
                    candidate: id,
                    row: i,
                    x,
                    y,
                    gap,
                    raw: c["origin"] == "raw_component",
                });
                if gap.is_none() {
                    rejected.push(json!({"candidate":id,"observation":i,"reason":"no_preceding_body_ink_or_no_following_body_gap"}));
                }
            }
        }
        let tracks = fit(&points, h, words.len(), bottom - top - h);
        let status = if tracks.len() > 1 {
            "ambiguous"
        } else if tracks.is_empty() {
            "unsupported"
        } else {
            "candidate_track"
        };
        columns.push(json!({"layout_column":column,"word_observations":words,"y_span":[top,bottom],"height":h,"status":status,"tracks":tracks,"track_candidates":points.len(),"gap_candidates":points.iter().filter(|p|p.gap.is_some()).count(),"separate_label_regions":words.iter().zip(&labels).map(|(&i,r)|json!({"observation":i,"regions":r})).collect::<Vec<_>>(),"rejections":rejected}));
    }
    json!({"authority_owner":"native_grid_reconciliation","corridors_diagnostic_only":true,"columns":columns})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn separate_label_observations_exclude_word_interior_islands() {
        let boxes = [
            [70., 20., 210., 80.],
            [14., 35., 35., 55.],
            [70., 110., 210., 170.],
            [14., 125., 35., 145.],
        ];
        let regions = separate_labels(0, &boxes);
        assert_eq!(regions, vec![boxes[1]]);
        assert!(in_label_region(30., 51., 60., &regions));
        assert!(!in_label_region(138.5, 77.5, 60., &regions));
        // A dot higher than the word's baseline is still in its label region.
        assert!(in_label_region(30., 40., 60., &regions));
        assert!(separate_labels(0, &[[10., 20., 210., 80.], boxes[1]]).is_empty());
    }
    fn dot(row: usize, x: f32, y: f32) -> Support {
        Support {
            candidate: row,
            row,
            x,
            y,
            gap: Some([x + 4., x + 12.]),
            raw: true,
        }
    }
    #[test]
    fn two_rows_fit_but_same_row_peaks_do_not() {
        assert_eq!(
            fit(&[dot(0, 20., 0.), dot(1, 22., 100.)], 40., 2, 100.).len(),
            1
        );
        assert!(fit(&[dot(0, 20., 0.), dot(0, 22., 100.)], 40., 2, 100.).is_empty());
        assert!(fit(&[dot(0, 20., 0.), dot(1, 21., 5.)], 40., 2, 100.).is_empty());
    }
    #[test]
    fn competitors_remain_visible_and_disjoint_gaps_have_no_separator() {
        let points = [
            dot(0, 20., 0.),
            dot(1, 20., 100.),
            dot(0, 50., 0.),
            dot(1, 50., 100.),
        ];
        // Two rows cannot distinguish parallel from crossing pairings. Keep
        // that ambiguity rather than silently choosing one of the lines.
        assert!(fit(&points, 40., 2, 100.).len() >= 2);
        let mut points = [dot(0, 20., 0.), dot(1, 20., 100.)];
        points[1].gap = Some([40., 50.]);
        assert!(fit(&points, 40., 2, 100.)[0]["separator_intercept_interval"].is_null());
    }
}
