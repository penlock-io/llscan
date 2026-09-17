//! Default ordinal dominance comes from layout dimensions, never checksums.
use super::*;

/// Counts of retained geometric groups in the shared writing frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrderSupport {
    /// Number of native column groups among retained word observations.
    pub columns: usize,
    /// Number of native row groups among retained word observations.
    pub rows: usize,
}

impl OrderSupport {
    /// The longer dimension in cells, not pixels: words are wider than tall.
    pub fn preferred(&self) -> Option<InitialOrder> {
        use std::cmp::Ordering;
        match self.rows.cmp(&self.columns) {
            Ordering::Greater => Some(InitialOrder::Columns),
            Ordering::Less => Some(InitialOrder::Rows),
            Ordering::Equal => None,
        }
    }

    /// Record the counts and default policy independently of checksum results.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({"preferred":self.preferred().map(InitialOrder::as_str),
            "columns":self.columns,"rows":self.rows,
            "one_group_sensitive":self.columns.abs_diff(self.rows) <= 1,
            "source":"retained_box_groups","policy":"longest_dimension_first"})
    }

    pub(super) fn measure(quads: &[Quad], writing: WritingFrame) -> Self {
        let layout = crate::layout::PageLayout::with_writing(quads, writing);
        Self {
            columns: layout.columns.len(),
            rows: layout.rows.len(),
        }
    }
}

// Deliberately no checksum or word-confidence parameters: neither is layout.
pub(super) fn decide(
    numbered: bool,
    same: bool,
    support: &OrderSupport,
) -> (InitialOrder, bool, &'static str) {
    if numbered {
        (InitialOrder::Numbers, false, "held_numbers")
    } else if same {
        (InitialOrder::Columns, false, "same_retained_traversal")
    } else if let Some(order) = support.preferred() {
        // Preserve the user's default even near a tie, but do not present it
        // as settled when one missed/extra group could erase the preference.
        let sensitive = support.columns.abs_diff(support.rows) <= 1;
        (
            order,
            sensitive,
            if sensitive {
                "longest_dimension_first_near_tie"
            } else {
                "longest_dimension_first"
            },
        )
    } else {
        (
            InitialOrder::Columns,
            true,
            "equal_dimensions_unverified_layout",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rect(x: f32, y: f32, w: f32, h: f32) -> Quad {
        Quad([(x, y), (x + w, y), (x + w, y + h), (x, y + h)])
    }

    #[test]
    fn longest_dimension_counts_cells_not_pixels_in_shared_writing_frame() {
        for scale in [0.2, 1., 8.] {
            for angle in [0., 17., 90., 180., 270.] {
                let writing = WritingFrame::Page(crate::page_frame::test_direction(angle));
                for (columns, rows, expected) in [
                    (2, 6, Some(InitialOrder::Columns)),
                    (6, 2, Some(InitialOrder::Rows)),
                    (1, 12, Some(InitialOrder::Columns)),
                    (12, 1, Some(InitialOrder::Rows)),
                    (2, 2, None),
                    (3, 4, Some(InitialOrder::Columns)),
                    (4, 3, Some(InitialOrder::Rows)),
                ] {
                    // Very wide words make even 2x6 physically wider than tall.
                    let quads: Vec<_> = (0..rows)
                        .flat_map(|row| {
                            (0..columns).map(move |col| {
                                writing.unproject(&rect(
                                    (71. + col as f32 * 250.) * scale,
                                    (43. + row as f32 * 25.) * scale,
                                    200. * scale,
                                    20. * scale,
                                ))
                            })
                        })
                        .collect();
                    let support = OrderSupport::measure(&quads, writing);
                    assert_eq!(support, OrderSupport { columns, rows });
                    assert_eq!(support.preferred(), expected);
                }
            }
        }
        assert_eq!(
            OrderSupport::measure(&[], WritingFrame::Local).preferred(),
            None
        );
    }

    #[test]
    fn labels_override_dimension_and_ties_never_claim_order_confirmation() {
        for (columns, rows) in [
            (2, 6),
            (6, 2),
            (2, 2),
            (0, 0),
            (3, 4),
            (4, 3),
            (2, 4),
            (4, 2),
        ] {
            let support = OrderSupport { columns, rows };
            assert_eq!(
                decide(true, false, &support),
                (InitialOrder::Numbers, false, "held_numbers")
            );
            assert!(!decide(false, true, &support).1);
            let (order, review, _) = decide(false, false, &support);
            assert_eq!(review, columns.abs_diff(rows) <= 1);
            assert_eq!(order, support.preferred().unwrap_or(InitialOrder::Columns));
        }
    }
}
