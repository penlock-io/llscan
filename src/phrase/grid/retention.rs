//! A labeled cell is evidence that word ink exists, not that OCR is correct.
use super::*;
use crate::phrase::evidence::CellSupport;

pub(super) fn word_ink_pixels(photo: &RgbImage, q: &Quad, writing: WritingFrame) -> usize {
    let frame = writing.crop_frame(q, 0.);
    let crop = level_crop_in(photo, frame);
    let mut ink = source_ink(photo, q, frame, &crop);
    strip_rules(&mut ink);
    ruling::strip_context_rules(photo, frame, &mut ink);
    ink.iter().flatten().filter(|v| **v).count()
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Cell {
    pub(super) bounds: Bounds,
    ordinal: Option<u32>,
}

impl Grid {
    pub(super) fn install_cells(
        &mut self,
        guides: &[(ColumnBounds, serde_json::Value)],
        anchors: &[Anchor],
        writing: WritingFrame,
    ) {
        self.row_support = guides
            .iter()
            .enumerate()
            .map(|(column, (c, view))| {
                let rows = &self.row_centres[column];
                if rows.len() < 2 {
                    return vec![None; rows.len()];
                }
                let pitch = median(rows.windows(2).map(|r| r[1] - r[0]).collect());
                let width = view["label_gutter_width"].as_f64().unwrap_or(0.) as f32;
                let mut supports = Vec::new();
                let mut offsets = Vec::new();
                let mut observe = |b: Bounds, ordinal: Option<u32>| {
                    let x = (b.l + b.r) / 2.;
                    if x < c.left(b.y()) - width || x > c.left(b.y()) {
                        return;
                    }
                    let row = (0..rows.len())
                        .min_by(|&a, &d| {
                            (rows[a] - b.y()).abs().total_cmp(&(rows[d] - b.y()).abs())
                        })
                        .unwrap();
                    if (rows[row] - b.y()).abs() > 0.5 * pitch {
                        return;
                    }
                    supports.push(row);
                    if let Some(n) = ordinal {
                        offsets.push(n as i64 - row as i64);
                    }
                };
                for a in anchors {
                    observe(a.bounds, a.ordinal);
                }
                // Only isolated measured marks from the accepted dot track, not
                // arbitrary detector centres or proposed/rejected dot candidates.
                if let Some(marks) = view["dot_reconciliation"]["measured_dot_edges"].as_array() {
                    for mark in marks {
                        if let Ok([l, t, r, b]) =
                            serde_json::from_value::<[f32; 4]>(mark["photo_bounds"].clone())
                        {
                            observe(
                                Bounds::of(&writing.project(&Bounds { l, t, r, b }.quad())),
                                None,
                            );
                        }
                    }
                }
                let complete = view["row_fit"]["accepted"] == true;
                let (lo, hi) = if complete {
                    (0, rows.len() - 1)
                } else if let (Some(&lo), Some(&hi)) =
                    (supports.iter().min(), supports.iter().max())
                {
                    (lo, hi)
                } else {
                    return vec![None; rows.len()];
                };
                let offset = offsets
                    .first()
                    .copied()
                    .filter(|n| offsets.iter().all(|m| m == n));
                rows.iter()
                    .enumerate()
                    .map(|(row, _)| {
                        if row < lo || row > hi {
                            return None;
                        }
                        let t = if row == 0 {
                            c.top
                        } else {
                            (rows[row - 1] + rows[row]) / 2.
                        };
                        let b = if row + 1 == rows.len() {
                            c.bottom
                        } else {
                            (rows[row] + rows[row + 1]) / 2.
                        };
                        let ordinal = if complete {
                            view["rows"][row]["ordinal"]
                                .as_u64()
                                .and_then(|n| u32::try_from(n).ok())
                        } else {
                            offset
                                .and_then(|n| u32::try_from(n + row as i64).ok())
                                .filter(|n| *n > 0)
                        };
                        Some(Cell {
                            bounds: Bounds {
                                l: c.left(t).max(c.left(b)),
                                t,
                                r: c.right_at(t).min(c.right_at(b)),
                                b,
                            },
                            ordinal,
                        })
                    })
                    .collect()
            })
            .collect();
    }

    pub(in crate::phrase) fn word_cell(
        &self,
        photo: &RgbImage,
        q: &Quad,
        writing: WritingFrame,
    ) -> Option<CellSupport> {
        if self.exclusion_read(q, writing, None).is_some() {
            return None;
        }
        let b = Bounds::of(&writing.project(q));
        let cx = (b.l + b.r) / 2.;
        let compact = self.compact_word(q);
        // An exact local prefix owns its word independently of grid fitting.
        let local = self
            .limits
            .iter()
            .find(|l| l.applies(b) && l.label.as_ref().is_some_and(|a| a.ordinal.is_some()));
        let cell = self
            .row_support
            .iter()
            .enumerate()
            .find_map(|(column, rows)| {
                rows.iter().enumerate().find_map(|(row, cell)| {
                    let cell = cell.as_ref()?;
                    (cx >= cell.bounds.l
                        && cx < cell.bounds.r
                        && b.y() >= cell.bounds.t
                        && b.y() < cell.bounds.b)
                        .then_some((column, row, *cell))
                })
            });
        if local.is_none() && cell.is_none() && !compact {
            return None;
        }
        // No OCR/classifier eligibility enters this mask check. Blank cells
        // create nothing, and ruling lines alone are not word-side ink.
        let ink_pixels = word_ink_pixels(photo, q, writing);
        if ink_pixels == 0 {
            return None;
        }
        Some(CellSupport {
            column: cell.map(|v| v.0),
            row: cell.map(|v| v.1),
            ordinal: local
                .and_then(|l| l.label.as_ref()?.ordinal)
                .or(cell.and_then(|v| v.2.ordinal)),
            quad: writing
                .unproject(
                    &cell
                        .map_or_else(|| local.map_or(b, |l| l.word), |v| v.2.bounds)
                        .quad(),
                )
                .0,
            ink_pixels,
            basis: if local.is_some() {
                "confirmed_numeric_prefix_and_word_ink"
            } else if cell.is_some() {
                "label_track_cell_and_word_ink"
            } else {
                "compact_row_fragments_and_word_ink"
            },
        })
    }
}
