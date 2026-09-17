//! The region a phrase occupies: its words sit in rows and columns
//! within reach of one another, and a few boxes far from all of them
//! — a keyboard's key caps under the page, a label on the desk — are
//! not the phrase's, however well they read. The words are clustered
//! by reach, the largest cluster is the phrase, and a small cluster
//! apart from it is left out, to be put back by hand.

use crate::detect::Quad;

use crate::layout::PageLayout;
pub use crate::layout::{COLUMN_REACH, ROW_REACH};

/// The largest component that may be excluded; it must also have fewer
/// than one third of the largest component's observations.
pub const APART_AT_MOST: usize = 3;

/// Compatibility entry point; the page pipeline shares an existing layout.
pub fn apart(quads: &[Quad], eligible: &[bool]) -> Vec<bool> {
    apart_in(&PageLayout::new(quads), eligible)
}

/// Region exclusion using the stage's shared geometric evidence.
pub fn apart_in(layout: &PageLayout, eligible: &[bool]) -> Vec<bool> {
    apart_in_observed(layout, eligible, |_, _| {})
}

pub(crate) fn apart_in_observed(
    layout: &PageLayout,
    eligible: &[bool],
    mut record: impl FnMut(usize, &dyn Fn() -> serde_json::Value),
) -> Vec<bool> {
    let (groups, height) = layout.reach_groups_with_scale(eligible);
    let main = groups.iter().map(Vec::len).max().unwrap_or(0);
    let mut out = vec![false; eligible.len()];
    for (i, &enabled) in eligible.iter().enumerate() {
        if !enabled {
            record(
                i,
                &|| serde_json::json!({"rule":"region_exclusion", "status":"skipped", "reason":"ineligible_for_region", "apart":false}),
            );
        }
    }
    for group in groups {
        let apart = group.len() <= APART_AT_MOST && 3 * group.len() < main;
        for &i in &group {
            out[i] = apart;
            record(i, &|| {
                serde_json::json!({"rule":"region_exclusion", "status":"evaluated",
                "bounds_coordinate_frame":if layout.writing.shared() {"page_writing_frame"} else {"canonical_photo"},
                "writing_angle_degrees":layout.writing.angle_or(0.0),
                "height_scale":height, "horizontal_padding":height.map(|h| COLUMN_REACH*h),
                "vertical_padding":height.map(|h| ROW_REACH*h),
                "reach_rule":"strict overlap of height-padded axis-aligned bounds; transitive connectivity",
                "group_bounds":group.iter().map(|&j| layout.observation_bounds(j)).collect::<Vec<_>>(),
                "group_size":group.len(), "largest_group_size":main, "max_excluded_size":APART_AT_MOST,
                "size_multiplier":3, "comparison":"group_size <= max_excluded_size and size_multiplier * group_size < largest_group_size",
                "apart":apart})
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(x: f32, y: f32) -> Quad {
        Quad([
            (x, y),
            (x + 400.0, y),
            (x + 400.0, y + 200.0),
            (x, y + 200.0),
        ])
    }

    /// Two columns of six words, rows 250 apart, columns 900 apart.
    fn phrase() -> Vec<Quad> {
        (0..12)
            .map(|i| {
                word(
                    if i < 6 { 900.0 } else { 1800.0 },
                    1000.0 + 250.0 * (i % 6) as f32,
                )
            })
            .collect()
    }

    #[test]
    fn key_caps_far_above_the_words_stand_apart_and_the_words_do_not() {
        let mut quads = phrase();
        quads.push(word(1200.0, 20.0));
        quads.push(word(1650.0, 160.0));
        let eligible = vec![true; quads.len()];
        let out = apart(&quads, &eligible);
        assert_eq!(out.iter().filter(|&&a| a).count(), 2);
        assert!(out[12] && out[13]);
    }

    #[test]
    fn a_phrase_split_in_two_far_columns_or_nine_and_three_stays_whole() {
        let mut quads = phrase();
        for q in &mut quads[6..] {
            for p in &mut q.0 {
                p.0 += 1500.0;
            }
        }
        assert!(apart(&quads, &[true; 12]).iter().all(|&a| !a));
        let mut quads = phrase();
        for q in &mut quads[9..] {
            for p in &mut q.0 {
                p.1 += 800.0;
            }
        }
        assert!(apart(&quads, &[true; 12]).iter().all(|&a| !a));
    }

    #[test]
    fn a_box_that_is_no_word_takes_no_part_and_a_lone_stray_leaves() {
        let mut quads = phrase();
        quads.push(word(900.0, 4000.0));
        quads.push(word(1800.0, 4000.0));
        let mut eligible = vec![true; quads.len()];
        eligible[13] = false;
        let out = apart(&quads, &eligible);
        assert!(out[12]);
        assert!(!out[13]);
        assert!(out[..12].iter().all(|&a| !a));
        assert!(apart(&quads[..1], &[true]).iter().all(|&a| !a));
    }
}
