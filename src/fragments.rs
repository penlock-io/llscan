//! Geometry-only ownership of tiny raw detections. No OCR, classifier, target
//! word count or spelling is an input. The returned layout is the one to carry
//! through filtering and reading; callers must not fit another layout.

use crate::detect::{Quad, intersects};
use crate::layout::PageLayout;
use crate::page_frame::WritingFrame;
use crate::sources::RawSource;
use crate::split::{Frame, turn};

/// A split observation, or the whole-raw placeholder for an empty split.
/// References the source ledger, not a mutable final-word index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Observation {
    /// Index in the raw-source ledger (which stays in detector order).
    pub source: usize,
    /// Stable part ID; none identifies an empty source's raw placeholder.
    pub part: Option<usize>,
}

impl Observation {
    /// Resolves the original geometry from the ledger, without a second copy.
    pub fn quad(self, sources: &[RawSource]) -> &Quad {
        let source = &sources[self.source];
        match self.part {
            Some(id) => {
                &source
                    .parts()
                    .iter()
                    .find(|p| p.part == id)
                    .expect("stable part ID")
                    .quad
            }
            None => &source.quad,
        }
    }

    /// The original or recovered reading, absent for a suppressed fragment.
    pub fn word_index(self, sources: &[RawSource]) -> Option<usize> {
        let raw = &sources[self.source];
        match self.part {
            Some(id) => raw
                .parts()
                .iter()
                .find(|p| p.part == id)
                .and_then(|p| p.word_index),
            None => match raw.outcome {
                crate::sources::Outcome::Rescued { word_index, .. } => Some(word_index),
                _ => None,
            },
        }
    }
}

/// Independent local writing scale, measured from other raw sources once each.
#[derive(Clone, Debug, PartialEq)]
pub struct Scale {
    /// Median short-side height in photo pixels.
    pub height: f32,
    /// Supporting raw-source ledger indices, excluding the candidate itself.
    pub supports: Vec<usize>,
}

/// Why geometry cannot establish a reliable local word scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoScale {
    /// No observation belongs to a column.
    NoColumn,
    /// The source's parts span multiple existing columns.
    MultipleColumns,
    /// Fewer than three other elongated raw sources in this column.
    TooFewSupports,
    /// Supporting writing axes disagree by more than fifteen degrees.
    ConflictingAxes,
    /// The candidate has no finite, positive-area frame.
    InvalidFrame,
}

/// Why an established tiny source has no automatic owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unowned {
    /// The geometric owner has no selected/readable observation.
    OwnerUnavailable,
    /// No same-column parent meets the containment/proximity rules.
    NoOwner,
    /// More than one observation could own the ink; distance is not a tie-break.
    Ambiguous,
    /// The fragment or proposed read footprint intersects another observation.
    OtherObservation,
}

/// An ownership proposal, before any parent/union reading is available.
#[derive(Clone, Debug, PartialEq)]
pub enum Ownership {
    /// Lower ink wholly inside the parent's original split geometry.
    Duplicate(Observation),
    /// Overlapping lower ink extends beyond the parent. Preserve it separately;
    /// do not claim the parent's old crop/read contained this entire fragment.
    Descender(Observation),
    /// A detached right ending needs one bounded union read, not classification
    /// of the tiny piece. This is a proposal, never automatic read acceptance.
    Ending {
        /// The geometric parent, still identified through the source ledger.
        owner: Observation,
        /// Full parent observation plus the full raw fragment, in photo pixels.
        union: Quad,
        /// The actual rounded reader canvas with its configured margin.
        footprint: Quad,
    },
    /// Retain source geometry/reason, without guessing a standalone word.
    Unowned(Unowned),
}

/// One geometry decision per accepted raw source.
#[derive(Clone, Debug, PartialEq)]
pub enum Decision {
    /// Missing scale never authorizes merging or suppressing nonempty parts.
    NoScale(NoScale),
    /// A genuine-sized source, including short words and failed whole-word splits.
    Word(Scale),
    /// A tiny token plausibly labels a word to its right. Leave it to numbering.
    PossibleLabel(Scale),
    /// Too small to be a standalone word, regardless of model confidence.
    Tiny {
        /// The independent local scale that establishes tiny size.
        scale: Scale,
        /// Unique ownership or an explicit refusal; no reading is fabricated.
        ownership: Ownership,
    },
}

impl Decision {
    /// Stable geometry outcome for diagnostics and mobile provenance.
    pub fn reason(&self) -> &'static str {
        match self {
            Self::NoScale(NoScale::NoColumn) => "no_column",
            Self::NoScale(NoScale::MultipleColumns) => "multiple_columns",
            Self::NoScale(NoScale::TooFewSupports) => "too_few_supports",
            Self::NoScale(NoScale::ConflictingAxes) => "conflicting_axes",
            Self::NoScale(NoScale::InvalidFrame) => "invalid_frame",
            Self::Word(_) => "word",
            Self::PossibleLabel(_) => "possible_label",
            Self::Tiny { ownership, .. } => match ownership {
                Ownership::Duplicate(_) => "duplicate",
                Ownership::Descender(_) => "owned_descender",
                Ownership::Ending { .. } => "detached_ending",
                Ownership::Unowned(Unowned::OwnerUnavailable) => "owner_unavailable",
                Ownership::Unowned(Unowned::NoOwner) => "no_owner",
                Ownership::Unowned(Unowned::Ambiguous) => "ambiguous_owner",
                Ownership::Unowned(Unowned::OtherObservation) => "other_observation",
            },
        }
    }

    /// The measured local scale, when enough independent support exists.
    pub fn scale(&self) -> Option<&Scale> {
        match self {
            Self::NoScale(_) => None,
            Self::Word(s) | Self::PossibleLabel(s) | Self::Tiny { scale: s, .. } => Some(s),
        }
    }

    /// Stable source/part reference for a uniquely owned fragment.
    pub fn owner(&self) -> Option<Observation> {
        match self {
            Self::Tiny {
                ownership:
                    Ownership::Duplicate(o)
                    | Ownership::Descender(o)
                    | Ownership::Ending { owner: o, .. },
                ..
            } => Some(*o),
            _ => None,
        }
    }
}

/// Geometry and decisions to carry into reading. This planner does not mutate
/// the ledger, remove parts, invoke models or apply an unverified union read.
pub struct Plan {
    /// Existing PageLayout fitted once to parts plus empty raw placeholders.
    pub layout: PageLayout,
    /// Ledger references indexed by that layout's observations.
    pub observations: Vec<Observation>,
    /// Decisions in raw-source ledger order, not traversal order.
    pub decisions: Vec<Decision>,
}

/// Plans ownership using the approved dimensionless constants. A part-rich raw
/// source contributes only one scale sample; an empty split remains geometry.
pub fn plan(sources: &[RawSource], margin: f32) -> Plan {
    plan_with_writing(sources, margin, WritingFrame::Local)
}

pub(crate) fn plan_with_writing(sources: &[RawSource], margin: f32, writing: WritingFrame) -> Plan {
    assert!(margin.is_finite() && margin >= 0.0);
    let observations: Vec<_> = sources
        .iter()
        .enumerate()
        .flat_map(|(source, raw)| {
            if raw.parts().is_empty() {
                vec![Observation { source, part: None }]
            } else {
                raw.parts()
                    .iter()
                    .map(|p| Observation {
                        source,
                        part: Some(p.part),
                    })
                    .collect()
            }
        })
        .collect();
    let quads: Vec<_> = observations
        .iter()
        .map(|o| o.quad(sources).clone())
        .collect();
    let layout = PageLayout::with_writing(&quads, writing);
    let frames: Vec<_> = sources.iter().map(|s| writing.frame_of(&s.quad)).collect();
    let observation_frames: Vec<_> = quads.iter().map(|q| writing.frame_of(q)).collect();
    let mut columns = vec![Vec::new(); sources.len()];
    for (column, members) in layout.columns.iter().enumerate() {
        for &i in members {
            let memberships = &mut columns[observations[i].source];
            if !memberships.contains(&column) {
                memberships.push(column);
            }
        }
    }
    let decisions = (0..sources.len())
        .map(|source| {
            let scale = match scale_for(source, &frames, &columns) {
                Ok(scale) => scale,
                Err(reason) => return Decision::NoScale(reason),
            };
            let h = scale.height;
            if frames[source].w > 0.5 * h {
                return Decision::Word(scale);
            }
            let column = columns[source][0];
            let plausible_parent = |i: usize| {
                let other = observations[i].source;
                let f = frames[other];
                let p = observation_frames[i];
                other != source
                    && f.w >= h
                    && f.h >= 0.5 * h
                    && p.w >= h
                    && p.h >= 0.5 * h
                    && scale
                        .supports
                        .iter()
                        .all(|&s| same_axis(p.angle, frames[s].angle))
            };
            let owners: Vec<_> = layout.columns[column]
                .iter()
                .copied()
                .filter(|&i| plausible_parent(i))
                .collect();
            if (0..observations.len())
                .filter(|&i| plausible_parent(i))
                .any(|i| {
                    crate::numbering::leading_label(
                        &sources[source].quad,
                        &quads[i],
                        observation_frames[i].angle,
                        h * 1e-5,
                        writing,
                    )
                    .is_some()
                })
            {
                return Decision::PossibleLabel(scale);
            }
            let mut proposals = Vec::new();
            for i in owners {
                if let Some(ownership) = ownership(
                    observations[i],
                    observation_frames[i],
                    &sources[source].quad,
                    h,
                    margin,
                    writing,
                ) {
                    proposals.push((i, ownership));
                }
            }
            // Decide ambiguity before collision refusal, never pick a nearer owner.
            let ownership = match proposals.as_slice() {
                [] => Ownership::Unowned(Unowned::NoOwner),
                [(owner, proposal)] => {
                    let footprint = match proposal {
                        Ownership::Ending { footprint, .. } => footprint,
                        _ => &sources[source].quad,
                    };
                    if observations.iter().enumerate().any(|(i, o)| {
                        o.source != source && i != *owner && intersects(footprint, &quads[i])
                    }) {
                        Ownership::Unowned(Unowned::OtherObservation)
                    } else {
                        proposal.clone()
                    }
                }
                _ => Ownership::Unowned(Unowned::Ambiguous),
            };
            Decision::Tiny { scale, ownership }
        })
        .collect();
    Plan {
        layout,
        observations,
        decisions,
    }
}

fn scale_for(source: usize, frames: &[Frame], columns: &[Vec<usize>]) -> Result<Scale, NoScale> {
    let f = frames[source];
    if !valid_frame(f) {
        return Err(NoScale::InvalidFrame);
    }
    if columns[source].is_empty() {
        return Err(NoScale::NoColumn);
    }
    let [column] = columns[source].as_slice() else {
        return Err(NoScale::MultipleColumns);
    };
    let supports: Vec<_> = frames
        .iter()
        .enumerate()
        .filter_map(|(i, &f)| {
            (i != source && columns[i].contains(column) && valid_frame(f) && f.w >= 2.0 * f.h)
                .then_some(i)
        })
        .collect();
    if supports.len() < 3 {
        return Err(NoScale::TooFewSupports);
    }
    if supports.iter().enumerate().any(|(i, &a)| {
        supports[i + 1..]
            .iter()
            .any(|&b| !same_axis(frames[a].angle, frames[b].angle))
    }) {
        return Err(NoScale::ConflictingAxes);
    }
    let mut heights: Vec<_> = supports.iter().map(|&s| frames[s].h).collect();
    heights.sort_by(f32::total_cmp);
    let n = heights.len();
    Ok(Scale {
        height: (heights[(n - 1) / 2] + heights[n / 2]) / 2.0,
        supports,
    })
}

fn same_axis(a: f32, b: f32) -> bool {
    ((a - b + 90.0).rem_euclid(180.0) - 90.0).abs() <= 15.0
}

fn valid_frame(f: Frame) -> bool {
    [f.cx, f.cy, f.w, f.h, f.angle]
        .iter()
        .all(|v| v.is_finite())
        && f.w > 0.0
        && f.h > 0.0
}

fn bounds_in(frame: Frame, q: &Quad) -> [f32; 4] {
    let points =
        q.0.map(|(x, y)| turn(x - frame.cx, y - frame.cy, -frame.angle));
    [
        points.iter().map(|p| p.0).fold(f32::INFINITY, f32::min),
        points.iter().map(|p| p.1).fold(f32::INFINITY, f32::min),
        points.iter().map(|p| p.0).fold(f32::NEG_INFINITY, f32::max),
        points.iter().map(|p| p.1).fold(f32::NEG_INFINITY, f32::max),
    ]
}

fn quad_in(frame: Frame, [l, t, r, b]: [f32; 4]) -> Quad {
    Quad([(l, t), (r, t), (r, b), (l, b)].map(|(x, y)| {
        let (dx, dy) = turn(x, y, frame.angle);
        (frame.cx + dx, frame.cy + dy)
    }))
}

fn ownership(
    owner: Observation,
    parent: Frame,
    fragment: &Quad,
    h: f32,
    margin: f32,
    writing: WritingFrame,
) -> Option<Ownership> {
    let [l, t, r, b] = bounds_in(parent, fragment);
    let (left, right, top, bottom) = (
        -parent.w / 2.0,
        parent.w / 2.0,
        -parent.h / 2.0,
        parent.h / 2.0,
    );
    // Only floating-point transform tolerance; all policy distances scale by H.
    let e = h * 1e-5;
    if l >= left - e && r <= right + e && (t + b) / 2.0 > 0.0 {
        if t >= top - e && b <= bottom + e {
            return Some(Ownership::Duplicate(owner));
        }
        if t <= bottom + e && b >= bottom - e && b <= bottom + 0.5 * h + e {
            return Some(Ownership::Descender(owner));
        }
    }
    if l >= right - e && l - right <= 0.35 * h + e && t >= top - e && b <= bottom + e {
        let union = quad_in(parent, [left, top.min(t), right.max(r), bottom.max(b)]);
        let f = writing.crop_frame(&union, margin);
        let footprint = quad_in(
            f,
            [
                -f.w.round() / 2.0,
                -f.h.round() / 2.0,
                f.w.round() / 2.0,
                f.h.round() / 2.0,
            ],
        );
        return Some(Ownership::Ending {
            owner,
            union,
            footprint,
        });
    }
    None
}

#[cfg(test)]
mod tests;
