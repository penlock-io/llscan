//! Bounded E3 word expansion and the explicit historical E1/E2 studies.
//! Selection follows the contracts in reports/word-extent*contract.md.
//! No detector, label policy, layout fitting, spelling gate or model of its own.

use crate::detect::{Quad, intersects};
use crate::layout::PageLayout;
#[cfg(test)]
use crate::numbering::Numbering;
use crate::page_frame::WritingFrame;
use crate::phrase::history;
use crate::phrase::{CROP_MARGIN, PageScan, Reader, WordBox};
use crate::progress::{RegionState, Stage};
use crate::read_budget::{Budget, pixels_ceil};
#[cfg(test)]
use crate::read_budget::{MAX_READ_PIXELS, MAX_READS, MAX_ROI_PIXELS, MAX_ROIS};
use crate::recogniser::Recogniser;
use crate::split::{Frame, binarise, level_crop_in, turn};
#[cfg(test)]
use crate::split::{frame_of, level_crop, with_margin};
use crate::words::Verdict;
use image::RgbImage;
use serde_json::{Value, json};

mod ink_witness;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Arm {
    #[default]
    E1,
    #[cfg(any(test, feature = "extent-experiment"))]
    E2,
    E3,
}
impl Arm {
    fn label(self) -> &'static str {
        match self {
            Self::E1 => "E1",
            #[cfg(any(test, feature = "extent-experiment"))]
            Self::E2 => "E2",
            Self::E3 => "E3",
        }
    }

    fn uses_ink_floor(self) -> bool {
        !matches!(self, Self::E1)
    }
}

/// Detailed numeric evidence is optional; the ink-ownership decision is not.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Audit {
    /// Keep only geometry, trials, work and restoration provenance.
    #[default]
    None,
    /// Retain every intersecting component for an explicit diagnostic export.
    Components,
}

/// The ordinary page representation plus experiment-only provenance/choices.
#[derive(Debug)]
pub struct Experiment {
    /// Original observations and selected expansions in the shared traversals.
    pub scan: PageScan,
    /// Numeric evidence and restorable alternatives; not a production FFI field.
    pub report: Report,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Source {
    pub detector: usize,
    pub part: Option<usize>,
    pub quad: Quad,
    pub column_rank: usize,
    pub row_rank: usize,
    pub word_index: usize,
    pub labelled: bool,
}

#[derive(Debug, PartialEq)]
struct Trial {
    column: usize,
    row: usize,
    sources: Vec<usize>,
    parents: Vec<usize>,
    reason: &'static str,
    geometry: Option<Geometry>,
    read: Option<Read>,
    selected: Option<usize>,
    original_strays: Vec<bool>,
    active: bool,
}

/// Geometry and restoration provenance, with optional audit evidence.
/// No pixel buffers are retained here; a lean scan does not build report JSON.
#[derive(Debug, Default, PartialEq)]
pub struct Report {
    pub(crate) arm: Arm,
    audit: Audit,
    pub(crate) sources: Vec<Source>,
    pub(crate) budget: Budget,
    trials: Vec<Trial>,
}

impl Report {
    pub(crate) fn new(arm: Arm, audit: Audit) -> Self {
        Self {
            arm,
            audit,
            ..Self::default()
        }
    }

    /// Number of detailed component witnesses actually retained, not estimated bytes.
    pub fn retained_component_witnesses(&self) -> usize {
        self.trials
            .iter()
            .filter_map(|t| t.geometry.as_ref())
            .filter_map(|g| g.witness.as_ref())
            .map(|w| w.len())
            .sum()
    }

    /// Bytes in this typed value and its owned buffer capacities. Excludes
    /// allocator overhead, shared static strings and the separate PageScan.
    /// Unlike serialized length this counts metadata actually retained by lean.
    pub fn retained_bytes(&self) -> usize {
        use std::mem::size_of;
        size_of::<Self>()
            + self.sources.capacity() * size_of::<Source>()
            + self.trials.capacity() * size_of::<Trial>()
            + self
                .trials
                .iter()
                .map(|t| {
                    (t.sources.capacity() + t.parents.capacity()) * size_of::<usize>()
                        + t.original_strays.capacity() * size_of::<bool>()
                        + t.read
                            .as_ref()
                            .and_then(|r| r.raw.as_ref())
                            .map_or(0, String::capacity)
                        + t.geometry.as_ref().map_or(0, |g| {
                            g.supports.capacity() * size_of::<usize>()
                                + g.prior_footprints.capacity() * size_of::<Quad>()
                                + g.components.capacity() * size_of::<[u32; 4]>()
                                + g.witness
                                    .as_ref()
                                    .map_or(0, ink_witness::Witness::buffer_bytes)
                        })
                })
                .sum::<usize>()
    }

    /// Original page indices replaced by this expanded word, if any.
    pub fn expanded_from(&self, word: usize) -> Option<&[usize]> {
        self.trials
            .iter()
            .find(|t| t.selected == Some(word))
            .map(|t| t.parents.as_slice())
    }

    /// Explicit diagnostic export, never called by the lean mobile scan.
    /// Lean reports omit component evidence; audit exports keep the historical shape.
    pub fn to_json(&self) -> Value {
        // Export happens after the scan has returned, so account for it
        // separately from the inclusive extent computation span.
        let _export = self
            .arm
            .uses_ink_floor()
            .then(|| crate::timing::span("phrase.extent.export"));
        json!({"arm": self.arm.label(), "budget": self.budget.json(),
            "sources": self.sources.iter().map(|s| json!({
                "detector": s.detector, "part": s.part, "quad": s.quad.0,
                "column_rank": s.column_rank, "row_rank": s.row_rank,
                "word_index": s.word_index, "labelled": s.labelled,
            })).collect::<Vec<_>>(),
            "trials": self.trials.iter().map(|t| {
                let mut trial = json!({
                "column": t.column, "row": t.row, "sources": t.sources,
                "parents": t.parents, "reason": t.reason,
                "geometry": t.geometry.as_ref().map(Geometry::json), "read": t.read.as_ref().map(Read::json),
                "selected": t.selected, "active": t.active,
                "original_strays": t.original_strays,
                });
                if self.arm.uses_ink_floor() && self.audit == Audit::Components {
                    trial["ink_witness"] = t.geometry.as_ref()
                        .and_then(|g| g.witness.as_ref()).map(ink_witness::Witness::json)
                        .unwrap_or_else(|| json!({
                            "status": if t.geometry.is_none() { "not_applicable" } else { "unavailable" },
                            "reason": t.reason, "coverage": null, "components": null,
                        }));
                }
                trial
            }).collect::<Vec<_>>()})
    }

    /// Switches a selected expansion and its complete original interpretation.
    /// Indices/ranks and original crops are unchanged; strays restore losslessly.
    pub fn choose_expansion(
        &mut self,
        scan: &mut PageScan,
        trial: usize,
        active: bool,
    ) -> Result<(), String> {
        let t = self.trials.get_mut(trial).ok_or("unknown extent trial")?;
        let selected = t
            .selected
            .ok_or("extent trial did not select an expansion")?;
        if selected >= scan.words.len() || t.parents.iter().any(|&p| p >= scan.words.len()) {
            return Err("extent report does not match page".into());
        }
        let mut affected = t.parents.clone();
        affected.push(selected);
        history::record(scan, &affected, |_| {
            json!({"rule":"restore_expansion","status":"evaluated",
            "links":history::links(&t.parents,Some(selected)),"active":active,"previous_active":t.active,
            "original_strays":t.original_strays,"reason":"explicit_choose_expansion"})
        });
        scan.words[selected].set_stray_recorded(
            !active,
            "extent_choice",
            "explicit_choose_expansion",
        );
        for (&p, &stray) in t.parents.iter().zip(&t.original_strays) {
            scan.words[p].set_stray_recorded(
                if active { true } else { stray },
                "extent_choice",
                if active {
                    "replacement_activated"
                } else {
                    "original_keep_state_restored"
                },
            );
        }
        t.active = active;
        history::terminal(scan, &[], "explicit_extent_choice");
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Rect([f32; 4]);

impl Rect {
    fn width(self) -> f32 {
        self.0[2] - self.0[0]
    }
    fn height(self) -> f32 {
        self.0[3] - self.0[1]
    }
    fn grow(self, m: f32) -> Self {
        let [l, t, r, b] = self.0;
        Self([l - m, t - m, r + m, b + m])
    }
    fn include(&mut self, (x, y): (f32, f32)) {
        self.0[0] = self.0[0].min(x);
        self.0[1] = self.0[1].min(y);
        self.0[2] = self.0[2].max(x);
        self.0[3] = self.0[3].max(y);
    }
    fn quad(self, f: Frame) -> Quad {
        let [l, t, r, b] = self.0;
        Quad([(l, t), (r, t), (r, b), (l, b)].map(|p| photo_point(f, p)))
    }
}

fn local(f: Frame, (x, y): (f32, f32)) -> (f32, f32) {
    turn(x - f.cx, y - f.cy, -f.angle)
}
fn photo_point(f: Frame, (x, y): (f32, f32)) -> (f32, f32) {
    let (dx, dy) = turn(x, y, f.angle);
    (f.cx + dx, f.cy + dy)
}

fn valid_frame(f: Frame) -> bool {
    [f.cx, f.cy, f.w, f.h, f.angle]
        .iter()
        .all(|n| n.is_finite())
        && f.w > 0.
        && f.h > 0.
}
fn same_axis(a: f32, b: f32) -> bool {
    ((a - b + 90.).rem_euclid(180.) - 90.).abs() <= 10.
}
fn contains(q: &Quad, p: (f32, f32)) -> bool {
    let mut pos = false;
    let mut neg = false;
    for i in 0..4 {
        let (a, b) = (q.0[i], q.0[(i + 1) % 4]);
        let c = (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0);
        pos |= c > 1e-4;
        neg |= c < -1e-4;
    }
    !(pos && neg)
}
fn outside_distance(q: &Quad, p: (f32, f32)) -> f32 {
    if contains(q, p) {
        return 0.;
    }
    (0..4)
        .map(|i| {
            let (a, b) = (q.0[i], q.0[(i + 1) % 4]);
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let t = (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / (dx * dx + dy * dy)).clamp(0., 1.);
            (p.0 - a.0 - t * dx).hypot(p.1 - a.1 - t * dy)
        })
        .fold(f32::INFINITY, f32::min)
}
#[derive(Debug, Clone, PartialEq)]
struct Raster {
    writing: WritingFrame,
    quad: Quad,
    frame: Frame,
    w: u32,
    h: u32,
}
impl Raster {
    #[cfg(test)]
    fn new(quad: Quad) -> Result<Self, &'static str> {
        Self::in_frame(quad, WritingFrame::Local)
    }

    fn in_frame(quad: Quad, writing: WritingFrame) -> Result<Self, &'static str> {
        let frame = writing.frame_of(&quad);
        pixels_ceil(frame)?;
        if frame.w <= frame.h || frame.w > u32::MAX as f32 || frame.h > u32::MAX as f32 {
            return Err("writing_axis");
        }
        Ok(Self {
            writing,
            quad,
            frame,
            w: frame.w.round().max(1.) as u32,
            h: frame.h.round().max(1.) as u32,
        })
    }
    // Exactly split::level_crop's rounded canvas and pixel-centre transform.
    fn point(&self, x: f32, y: f32) -> (f32, f32) {
        photo_point(self.frame, (x - self.w as f32 / 2., y - self.h as f32 / 2.))
    }
    fn footprint(&self) -> Quad {
        Quad(
            [
                (0., 0.),
                (self.w as f32, 0.),
                (self.w as f32, self.h as f32),
                (0., self.h as f32),
            ]
            .map(|(x, y)| self.point(x, y)),
        )
    }
    fn json(&self) -> Value {
        let f = self.frame;
        json!({"quad":self.quad.0,"width":self.w,"height":self.h,
            "frame":{"cx":f.cx,"cy":f.cy,"w":f.w,"h":f.h,"angle":f.angle},
            "pixel_origin":self.point(0.5,0.5),"u":turn(1.,0.,f.angle),"v":turn(0.,1.,f.angle)})
    }
}

#[derive(Debug, PartialEq)]
struct Geometry {
    raster: Raster,
    supports: Vec<usize>,
    scale: f32,
    base: Quad,
    prior_footprints: Vec<Quad>,
    components: Vec<[u32; 4]>,
    candidate: Option<Quad>,
    footprint: Option<Quad>,
    witness: Option<ink_witness::Witness>,
}
impl Geometry {
    fn json(&self) -> Value {
        json!({"roi":self.raster.json(),"supports":self.supports,"scale":self.scale,
            "base":self.base.0,"prior_footprints":self.prior_footprints.iter().map(|q| q.0).collect::<Vec<_>>(),
            "seeded_component_bounds":self.components,
            "candidate":self.candidate.as_ref().map(|q| q.0),
            "read_footprint":self.footprint.as_ref().map(|q| q.0)})
    }
}

fn protected(s: &Source, scan: &PageScan) -> bool {
    let w = &scan.words[s.word_index];
    s.labelled
        || w.geometry_owned()
        || crate::repair::owns_lower_fragment(scan, s.word_index)
        || w.apart
        || w.narrowed
        || w.joined_from.is_some()
        || !w.expanded_from.is_empty()
        || scan.words.iter().any(|w| {
            w.joined_from.is_some_and(|p| p.contains(&s.word_index))
                || w.expanded_from.iter().any(|p| p.word_index == s.word_index)
        })
}

#[cfg(test)]
fn seeds(layout: &PageLayout, sources: &[Source], scan: &PageScan) -> Vec<Trial> {
    seeds_observed(layout, sources, scan, |_, _, _| {})
}

fn seeds_observed(
    layout: &PageLayout,
    sources: &[Source],
    scan: &PageScan,
    mut record: impl FnMut(&Trial, &[(usize, Vec<usize>)], bool),
) -> Vec<Trial> {
    let mut trials = Vec::new();
    for (column, members) in layout.columns.iter().enumerate() {
        let rows: Vec<_> = layout
            .rows
            .iter()
            .enumerate()
            .filter_map(|(row, indices)| {
                let p: Vec<_> = indices
                    .iter()
                    .copied()
                    .filter(|i| members.contains(i))
                    .collect();
                (!p.is_empty()).then_some((row, p))
            })
            .collect();
        let structure = rows.len() >= 3
            && rows.iter().all(|(_, p)| p.len() <= 2)
            && rows.iter().filter(|(_, p)| p.len() == 2).count() <= 1;
        for (row, indices) in &rows {
            let parents: Vec<_> = indices.iter().map(|&i| sources[i].word_index).collect();
            let reason = if !structure {
                "row_structure"
            } else if !scan.numbering.permits_word_repair() {
                "numbering"
            } else if members.iter().any(|&i| sources[i].labelled) {
                "column_label"
            } else if indices.iter().any(|&i| protected(&sources[i], scan)) {
                "protected"
            } else if rows
                .iter()
                .filter(|(r, p)| r != row && p.len() == 1)
                .count()
                < 2
            {
                "supports"
            } else if indices.len() == 2
                && parents
                    .iter()
                    .any(|&p| scan.words[p].verdict() == Verdict::Accept)
            {
                "parent_accepted"
            } else if indices.len() == 2
                && !scan.join_trials.iter().any(|t| {
                    t.read.is_some()
                        && t.reason != "joined"
                        && t.parents.iter().all(|p| parents.contains(p))
                })
            {
                "pair_unread"
            } else {
                "candidate"
            };
            let trial = Trial {
                column,
                row: *row,
                sources: indices.clone(),
                parents,
                reason,
                geometry: None,
                read: None,
                selected: None,
                original_strays: Vec::new(),
                active: false,
            };
            record(&trial, &rows, structure);
            trials.push(trial);
        }
    }
    trials.sort_by_key(|t| {
        (
            t.column,
            t.row,
            t.sources.iter().copied().min().unwrap_or(usize::MAX),
        )
    });
    trials
}

fn geometry_seed(
    t: &Trial,
    layout: &PageLayout,
    sources: &[Source],
    scan: &PageScan,
) -> Result<Geometry, &'static str> {
    let first = *t
        .sources
        .iter()
        .min_by_key(|&&i| sources[i].column_rank)
        .ok_or("no_observation")?;
    let writing = layout.writing;
    let frame = writing.frame_of(&sources[first].quad);
    if !valid_frame(frame) || frame.w <= frame.h {
        return Err("writing_axis");
    }
    for &i in &t.sources {
        let f = writing.frame_of(&sources[i].quad);
        if !valid_frame(f) || f.w <= f.h || !same_axis(f.angle, frame.angle) {
            return Err("writing_axis");
        }
    }
    let mut heights = Vec::new();
    let mut supports = Vec::new();
    for (row, indices) in layout.rows.iter().enumerate() {
        if row == t.row {
            continue;
        }
        let p: Vec<_> = indices
            .iter()
            .copied()
            .filter(|i| layout.columns[t.column].contains(i))
            .collect();
        if let [i] = p.as_slice() {
            let f = writing.frame_of(&sources[*i].quad);
            if !protected(&sources[*i], scan)
                && valid_frame(f)
                && f.w > f.h
                && same_axis(f.angle, frame.angle)
            {
                heights.push(f.h);
                supports.push(*i);
            }
        }
    }
    if heights.len() < 2 {
        return Err("supports");
    }
    heights.sort_by(f32::total_cmp);
    let n = heights.len();
    let scale = (heights[(n - 1) / 2] + heights[n / 2]) / 2.;
    let p = local(frame, sources[first].quad.0[0]);
    let mut bounds = Rect([p.0, p.1, p.0, p.1]);
    for &i in &t.sources {
        for p in sources[i].quad.0 {
            bounds.include(local(frame, p));
        }
    }
    // Preserve the existing width/height repair guard in the supplied direction.
    if bounds.width() <= bounds.height() {
        return Err("writing_axis");
    }
    let base = bounds.quad(frame);
    let raster = Raster::in_frame(bounds.grow(0.5 * scale).quad(frame), writing)?;
    let mut prior_footprints: Vec<_> = t
        .parents
        .iter()
        .map(|&i| {
            // Actual level_crop footprint, including its integer rounding.
            let f = writing.crop_frame(&scan.words[i].quad, CROP_MARGIN);
            Rect([
                -f.w.round() / 2.,
                -f.h.round() / 2.,
                f.w.round() / 2.,
                f.h.round() / 2.,
            ])
            .quad(f)
        })
        .collect();
    if let [a, b] = t.parents.as_slice() {
        let q = crate::joins::union_in(&scan.words[*a].quad, &scan.words[*b].quad, writing);
        let f = writing.crop_frame(&q, CROP_MARGIN);
        prior_footprints.push(
            Rect([
                -f.w.round() / 2.,
                -f.h.round() / 2.,
                f.w.round() / 2.,
                f.h.round() / 2.,
            ])
            .quad(f),
        );
    }
    Ok(Geometry {
        raster,
        supports,
        scale,
        base,
        prior_footprints,
        components: Vec::new(),
        candidate: None,
        footprint: None,
        witness: None,
    })
}

#[derive(Debug)]
struct Component {
    bounds: [u32; 4],
    pixels: u32,
    seeded: bool,
    border: bool,
    novel: bool,
}

fn continue_ink_for(
    arm: Arm,
    audit: Audit,
    g: &mut Geometry,
    ink: &[Vec<bool>],
    parents: &[&Quad],
    others: &[&Quad],
    selected: &[Quad],
) -> &'static str {
    let r = &g.raster;
    let (w, h) = (r.w as usize, r.h as usize);
    assert_eq!(ink.len(), h);
    assert!(ink.iter().all(|row| row.len() == w));
    // Each pixel is enqueued once. No morphology or threshold retry.
    let mut labels = vec![0u32; w * h];
    let mut components = Vec::<Component>::new();
    let mut pending = Vec::new();
    for y in 0..h {
        for x in 0..w {
            if !ink[y][x] || labels[y * w + x] != 0 {
                continue;
            }
            let id = components.len() as u32 + 1;
            let mut c = Component {
                bounds: [x as u32, y as u32, x as u32 + 1, y as u32 + 1],
                pixels: 0,
                seeded: false,
                border: false,
                novel: false,
            };
            labels[y * w + x] = id;
            pending.push((x, y));
            while let Some((x, y)) = pending.pop() {
                c.pixels += 1;
                let p = r.point(x as f32 + 0.5, y as f32 + 0.5);
                c.seeded |= parents.iter().any(|q| contains(q, p));
                c.novel |= g
                    .prior_footprints
                    .iter()
                    .all(|q| outside_distance(q, p) >= 2.);
                c.border |= x == 0 || y == 0 || x + 1 == w || y + 1 == h;
                c.bounds[0] = c.bounds[0].min(x as u32);
                c.bounds[1] = c.bounds[1].min(y as u32);
                c.bounds[2] = c.bounds[2].max(x as u32 + 1);
                c.bounds[3] = c.bounds[3].max(y as u32 + 1);
                for ny in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                    for nx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                        if ink[ny][nx] && labels[ny * w + nx] == 0 {
                            labels[ny * w + nx] = id;
                            pending.push((nx, ny));
                        }
                    }
                }
            }
            components.push(c);
        }
    }
    let seeded: Vec<_> = components.iter().filter(|c| c.seeded).collect();
    g.components = seeded.iter().map(|c| c.bounds).collect();
    if seeded.iter().any(|c| c.border) {
        return "continuation_at_border";
    }
    if !seeded.iter().any(|c| c.novel) {
        return "no_new_ink";
    }
    let writing = g.raster.writing;
    let frame = writing.frame_of(&g.base);
    let mut bounds = Rect([-frame.w / 2., -frame.h / 2., frame.w / 2., frame.h / 2.]);
    for c in seeded {
        let [l, t, rr, b] = c.bounds;
        for (x, y) in [(l, t), (rr, t), (rr, b), (l, b)] {
            bounds.include(local(frame, r.point(x as f32, y as f32)));
        }
    }
    let candidate = bounds.quad(frame);
    g.candidate = Some(candidate.clone());
    let wrong_axis = bounds.width() <= bounds.height()
        || !same_axis(writing.frame_of(&candidate).angle, frame.angle);
    if arm == Arm::E1 && wrong_axis {
        return "writing_axis";
    }
    let crop_quad = writing.with_margin(&candidate, CROP_MARGIN);
    let crop_frame = writing.crop_frame(&candidate, CROP_MARGIN);
    let actual = Rect([
        -crop_frame.w.round() / 2.,
        -crop_frame.h.round() / 2.,
        crop_frame.w.round() / 2.,
        crop_frame.h.round() / 2.,
    ])
    .quad(crop_frame);
    // Both the geometric margin and the actual rounded canvas must have been inspected.
    let roi_actual = r.footprint();
    let complete_coverage = crop_quad
        .0
        .iter()
        .chain(&actual.0)
        .all(|&p| contains(&r.quad, p) && contains(&roi_actual, p));
    if arm.uses_ink_floor() {
        // Evidence is retained even when an earlier guard refuses this
        // constructed candidate; it never changes the refusal precedence.
        g.witness = (audit == Audit::Components).then(|| {
            ink_witness::Witness::collect(
                r,
                g.scale,
                &labels,
                &components,
                &crop_quad,
                &actual,
                complete_coverage,
            )
        });
        if wrong_axis {
            return "writing_axis";
        }
    }
    if !complete_coverage {
        return "margin_outside_roi";
    }
    // Veto the union of nominal and actual footprints, so rounding never loses a guard.
    if arm == Arm::E1
        && others
            .iter()
            .any(|q| intersects(q, &crop_quad) || intersects(q, &actual))
    {
        return "other_observation";
    }
    if selected
        .iter()
        .any(|q| intersects(q, &crop_quad) || intersects(q, &actual))
    {
        return "candidate_conflict";
    }
    g.footprint = Some(actual.clone());
    if arm.uses_ink_floor() {
        let unowned = g.witness.as_ref().map_or_else(
            || ink_witness::has_unowned_ink(r, g.scale, &labels, &components, &crop_quad, &actual),
            ink_witness::Witness::has_unowned_ink,
        );
        return if unowned { "unowned_ink" } else { "read" };
    }
    for y in 0..h {
        for x in 0..w {
            let id = labels[y * w + x];
            if id == 0 || components[id as usize - 1].seeded {
                continue;
            }
            let p = r.point(x as f32 + 0.5, y as f32 + 0.5);
            if contains(&actual, p) || contains(&crop_quad, p) {
                return "unowned_ink";
            }
        }
    }
    "read"
}

fn read_reason_for(arm: Arm, parents: &[&WordBox], candidate: &WordBox) -> &'static str {
    // E3 separates selection for review from the unchanged confirmation verdict.
    if arm != Arm::E3 && candidate.verdict() != Verdict::Accept {
        return "uncertain";
    }
    let Some(&(top, probability)) = candidate.ranked.first() else {
        return "uncertain";
    };
    if candidate.stray {
        return "stray";
    }
    if candidate.pick() != Some(top) {
        return "pick_disagrees";
    }
    // Historical E1/E2 exempt a same-identity accepted singleton. E3 compares
    // every parent, including that singleton; equality is sufficient.
    let accepted_singleton = match parents {
        [p] if p.verdict() == Verdict::Accept => {
            if candidate.pick() != p.pick() {
                return "accepted_identity";
            }
            true
        }
        _ => false,
    };
    if (arm == Arm::E3 || !accepted_singleton)
        && parents
            .iter()
            .any(|p| p.ranked.first().is_none_or(|r| r.1 > probability))
    {
        return "parent_surer";
    }
    "expanded"
}

#[derive(Clone, Debug, PartialEq)]
struct Read {
    top: Option<usize>,
    probability: Option<f32>,
    margin: Option<f32>,
    pick: Option<usize>,
    kept: bool,
    verdict: Verdict,
    turned: bool,
    raw: Option<String>,
    literal_ocr_agrees: Option<bool>,
}

impl Read {
    fn new(w: &WordBox) -> Self {
        let word = w
            .pick()
            .and_then(|p| crate::vocabulary::Word::from_index(p as u16));
        Self {
            top: w.ranked.first().map(|r| r.0),
            probability: w.ranked.first().map(|r| r.1),
            margin: w
                .ranked
                .first()
                .map(|r| r.1 - w.ranked.get(1).map_or(0., |r| r.1)),
            pick: w.pick(),
            kept: !w.stray,
            verdict: w.verdict(),
            turned: w.turned,
            raw: w.raw.as_ref().map(|r| r.text.clone()),
            literal_ocr_agrees: w
                .raw
                .as_ref()
                .zip(word.as_ref())
                .map(|(raw, word)| raw.text.trim().to_ascii_lowercase() == word.as_str()),
        }
    }

    fn json(&self) -> Value {
        json!({"top":self.top,"probability":self.probability,"margin":self.margin,
            "pick":self.pick,"kept":self.kept,"verdict":format!("{:?}",self.verdict).to_lowercase(),
            "turned":self.turned,"raw":self.raw,"literal_ocr_agrees":self.literal_ocr_agrees})
    }
}

#[cfg(test)]
fn read_json(w: &WordBox) -> Value {
    Read::new(w).json()
}

pub(crate) fn apply(
    photo: &RgbImage,
    layout: &PageLayout,
    scan: &mut PageScan,
    report: &mut Report,
    reader: &Reader,
    recogniser: &Recogniser,
    progress: &mut Stage<'_>,
) -> Result<(), String> {
    apply_with(photo, layout, scan, report, progress, |quad, budget| {
        crate::phrase::read_extent_crop(photo, quad, reader, recogniser, budget)
    })
}

fn apply_with(
    photo: &RgbImage,
    layout: &PageLayout,
    scan: &mut PageScan,
    report: &mut Report,
    progress: &mut Stage<'_>,
    mut read: impl FnMut(&Quad, &mut Budget) -> Result<WordBox, String>,
) -> Result<(), String> {
    let _scope = crate::timing::span("phrase.extent.total");
    report.budget.writing = layout.writing;
    let sources = &report.sources;
    let mut seed_events = Vec::new();
    let mut trials = seeds_observed(layout, sources, scan, |t, rows, structure| {
        if t.parents
            .iter()
            .any(|&p| scan.words[p].evidence.decisions.is_some())
        {
            seed_events.push((t.parents.clone(), json!({"rule":"extent_seed","status":"evaluated",
                "links":history::links(&t.parents,None),"reason":t.reason,"column":t.column,"row":t.row,
                "column_row_sizes":rows.iter().map(|(r,p)| (*r,p.len())).collect::<Vec<_>>(),
                "structure":structure,"minimum_rows":3,"max_row_size":2,"max_pair_rows":1,"minimum_support_rows":2,
                "parents":t.parents.iter().map(|&p| history::reading(&scan.words[p])).collect::<Vec<_>>(),
                "source_labels":sources.iter().map(|s| json!({"detector":s.detector,"part":s.part,"labelled":s.labelled})).collect::<Vec<_>>(),
                "evaluation":"first refusal ends seed evaluation; geometry and readers run only for candidate"})));
        }
    });
    for (parents, event) in seed_events {
        history::record(scan, &parents, |_| event);
    }
    let mut selected = Vec::<Quad>::new();
    for t in &mut trials {
        if t.reason != "candidate" {
            continue;
        }
        if let Err(reason) = report.budget.inspect() {
            t.reason = reason;
            record_guard(scan, t, &report.budget);
            continue;
        }
        let _geometry = crate::timing::span("phrase.extent.geometry");
        let mut g = match geometry_seed(t, layout, sources, scan) {
            Ok(g) => g,
            Err(reason) => {
                t.reason = reason;
                record_guard(scan, t, &report.budget);
                continue;
            }
        };
        let inside_photo = |p: &(f32, f32)| {
            p.0 >= 0. && p.1 >= 0. && p.0 < photo.width() as f32 && p.1 < photo.height() as f32
        };
        if !g
            .raster
            .quad
            .0
            .iter()
            .chain(&g.raster.footprint().0)
            .all(inside_photo)
        {
            t.reason = "photo_edge";
            t.geometry = Some(g);
            record_guard(scan, t, &report.budget);
            continue;
        }
        if let Err(reason) = report
            .budget
            .roi(u64::from(g.raster.w) * u64::from(g.raster.h))
        {
            t.reason = reason;
            t.geometry = Some(g);
            record_guard(scan, t, &report.budget);
            continue;
        }
        let grey = level_crop_in(photo, g.raster.frame);
        let ink = binarise(&grey);
        let parents: Vec<_> = t.sources.iter().map(|&i| &sources[i].quad).collect();
        let others: Vec<_> = sources
            .iter()
            .enumerate()
            .filter_map(|(i, s)| (!t.sources.contains(&i)).then_some(&s.quad))
            .collect();
        t.reason = continue_ink_for(
            report.arm,
            report.audit,
            &mut g,
            &ink,
            &parents,
            &others,
            &selected,
        );
        drop(ink);
        drop(grey);
        drop(_geometry);
        if t.reason == "read" {
            let quad = g.candidate.as_ref().expect("geometry supplied candidate");
            if let Err(reason) = report.budget.read(quad) {
                t.reason = reason;
                t.geometry = Some(g);
                record_guard(scan, t, &report.budget);
                continue;
            }
            let _read = crate::timing::span("phrase.extent.read");
            for &p in &t.parents {
                progress.region(p, &scan.words[p].quad, RegionState::Reading);
            }
            let result = read(quad, &mut report.budget);
            // Refusal (or an error propagated to the outer Failed event)
            // must not leave the original overlays in a reading state.
            let restore = |progress: &Stage<'_>| {
                for &p in &t.parents {
                    let w = &scan.words[p];
                    progress.region(
                        p,
                        &w.quad,
                        if w.stray {
                            RegionState::Excluded
                        } else {
                            RegionState::Read
                        },
                    );
                }
            };
            let mut candidate = match result {
                Ok(candidate) => candidate,
                Err(error) => {
                    restore(progress);
                    return Err(error);
                }
            };
            let parents: Vec<_> = t.parents.iter().map(|&p| &scan.words[p]).collect();
            t.reason = read_reason_for(report.arm, &parents, &candidate);
            t.read = Some(Read::new(&candidate));
            let expanded = t.reason == "expanded";
            if expanded {
                candidate.column_rank = parents.iter().map(|w| w.column_rank).min().unwrap();
                candidate.row_rank = parents.iter().map(|w| w.row_rank).min().unwrap();
                t.original_strays = parents.iter().map(|w| w.stray).collect();
                candidate.expanded_from = t
                    .parents
                    .iter()
                    .zip(&t.original_strays)
                    .map(|(&word_index, &stray)| crate::phrase::ExtentParent { word_index, stray })
                    .collect();
            } else {
                restore(progress);
            }
            let candidate_index = expanded.then_some(scan.words.len());
            history::read_trial(
                scan,
                &t.parents,
                &mut candidate,
                candidate_index,
                "extent",
                t.reason,
                || {
                    json!({
                "arm":report.arm.label(),"geometry":g.json(),
                "ordered_guards":["uncertain","stray","pick_disagrees","accepted_identity","parent_surer"],
                "uncertain":"E1/E2 require confirmed; all arms require a classifier top",
                "stray":"candidate must be kept","pick_disagrees":"candidate pick == classifier top",
                "accepted_identity":"an accepted singleton must retain its identity",
                "parent_surer":"every parent top <= candidate top; missing parent top refuses; E1/E2 exempt accepted singleton",
                "success":"expanded"})
                },
            );
            if expanded {
                for &p in &t.parents {
                    scan.words[p].set_stray_recorded(true, "extent", "selected_replacement");
                }
                t.selected = Some(scan.words.len());
                t.active = true;
                progress.replaced(&candidate.quad, &t.parents);
                scan.words.push(candidate);
                selected.push(g.footprint.as_ref().unwrap().clone());
            }
        }
        t.geometry = Some(g);
        if t.read.is_none() {
            record_guard(scan, t, &report.budget);
        }
    }
    report.trials = trials;
    if report.trials.iter().any(|t| t.active) {
        reindex(scan, report, progress);
    }
    Ok(())
}

fn record_guard(scan: &mut PageScan, trial: &Trial, budget: &Budget) {
    history::record(scan, &trial.parents, |_| {
        json!({"rule":"replacement_guard","owner":"extent","status":"evaluated","candidate_read_status":"skipped",
        "reason":trial.reason,"links":history::links(&trial.parents,None),"geometry":trial.geometry.as_ref().map(Geometry::json),
        "budget":budget.json(),"limits":{"max_rois":crate::read_budget::MAX_ROIS,
            "max_roi_pixels":crate::read_budget::MAX_ROI_PIXELS,"total_roi_pixels":crate::read_budget::TOTAL_ROI_PIXELS,
            "max_reads":crate::read_budget::MAX_READS,"max_read_pixels":crate::read_budget::MAX_READ_PIXELS},
        "evaluation":"geometry/budget guard refused before candidate reader call"})
    });
}

fn reindex(scan: &mut PageScan, report: &mut Report, progress: &mut Stage<'_>) {
    let inverse = scan.reindex_words(progress);
    for s in &mut report.sources {
        s.word_index = inverse[s.word_index];
    }
    for t in &mut report.trials {
        t.parents = t.parents.iter().map(|&p| inverse[p]).collect();
        t.selected = t.selected.map(|p| inverse[p]);
    }
}

#[cfg(test)]
mod tests;
