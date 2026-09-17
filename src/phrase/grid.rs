//! Label-first geometry. No vocabulary, classifier score, expected phrase length,
//! reference coordinates or detector ROI enters this pass.
//!
//! Two locations describe a line. Readable labels establish measured tracks;
//! word-layout groups complete missing tracks with explicit estimates. Both
//! kinds constrain every assigned box regardless of its own label OCR/gap.

use super::{DecisionTrace, LineRead, Quad, RegionState, Stage, WritingFrame};
use crate::read_budget::Budget;
#[cfg(test)]
use crate::split::binarise;
use crate::split::{Frame, level_crop_in, strip_rules, turn};
use bitcoin_hashes::{Hash, sha256};
use image::{GrayImage, RgbImage};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Mutex;

mod cells;
mod compact;
mod dots;
pub(super) mod ink_width;
mod layout;

mod overlap_ink;
mod partition;
mod recovery;
mod retention;
mod rows;
mod ruling;

#[derive(Clone, Copy, Debug)]
struct Bounds {
    l: f32,
    t: f32,
    r: f32,
    b: f32,
}

impl Bounds {
    fn of(q: &Quad) -> Self {
        Self {
            l: q.0.iter().map(|p| p.0).fold(f32::INFINITY, f32::min),
            t: q.0.iter().map(|p| p.1).fold(f32::INFINITY, f32::min),
            r: q.0.iter().map(|p| p.0).fold(f32::NEG_INFINITY, f32::max),
            b: q.0.iter().map(|p| p.1).fold(f32::NEG_INFINITY, f32::max),
        }
    }
    fn w(self) -> f32 {
        self.r - self.l
    }
    fn h(self) -> f32 {
        self.b - self.t
    }
    fn y(self) -> f32 {
        (self.t + self.b) / 2.
    }
    fn quad(self) -> Quad {
        Quad([
            (self.l, self.t),
            (self.r, self.t),
            (self.r, self.b),
            (self.l, self.b),
        ])
    }
    fn overlap(self, b: Self) -> bool {
        self.l < b.r && self.r > b.l && self.t < b.b && self.b > b.t
    }
}

#[derive(Clone, Debug)]
struct Anchor {
    source: usize,
    bounds: Bounds,
    ordinal: Option<u32>,
    literal: String,
    confidence: f32,
    // Separator and associated word's footprint in page coordinates.
    split: Option<(f32, Bounds)>,
}

#[derive(Debug)]
struct Column {
    anchors: Vec<usize>,
    intercept: f32,
    slope: f32,
    height: f32,
    // Ordinal/y fit is independent of the label-right/y fit.
    rows: Option<(f32, f32)>,
}

#[derive(Debug)]
struct Limit {
    left: f32,
    word: Bounds,
    label: Option<super::LabelRead>,
}

#[derive(Debug)]
struct RightLimit {
    right: f32,
    word: Bounds,
}

impl Limit {
    fn applies(&self, b: Bounds) -> bool {
        let cx = (b.l + b.r) / 2.;
        b.overlap(self.word)
            && b.y() >= self.word.t
            && b.y() <= self.word.b
            && cx >= self.word.l
            && cx <= self.word.r
            && b.r.min(self.word.r) - b.l.max(self.word.l) >= 0.5 * b.w().min(self.word.w())
    }
}

#[derive(Debug)]
struct Envelope {
    bounds: Bounds,
    step: f32,
    top_supported: bool,
    bottom_supported: bool,
    terminal_label: bool,
}

#[derive(Clone, Copy, Debug)]
struct ColumnBounds {
    intercept: f32,
    slope: f32,
    right: f32,
    right_slope: f32,
    top: f32,
    bottom: f32,
}

impl ColumnBounds {
    fn left(self, y: f32) -> f32 {
        self.intercept + self.slope * y
    }
    fn right_at(self, y: f32) -> f32 {
        self.right + self.right_slope * y
    }
    fn right_limit(self, b: Bounds) -> f32 {
        self.right_at(b.t).min(self.right_at(b.b))
    }
    fn owns(self, b: Bounds) -> bool {
        let cx = (b.l + b.r) / 2.;
        b.y() >= self.top
            && b.y() <= self.bottom
            && (cx >= self.left(b.y()) || b.r >= self.left(b.y()) + b.h())
            && cx < self.right_at(b.y())
    }
    fn left_limit(self, b: Bounds) -> f32 {
        self.left(b.t).max(self.left(b.b))
    }
    fn quad(self) -> Quad {
        Quad([
            (self.left(self.top), self.top),
            (self.right_at(self.top), self.top),
            (self.right_at(self.bottom), self.bottom),
            (self.left(self.bottom), self.bottom),
        ])
    }
}

#[derive(Default)]
pub(super) struct Grid {
    photo_size: Option<(u32, u32)>,
    cache: Mutex<HashMap<(u32, u32, [u8; 32]), LineRead>>,
    limits: Vec<Limit>,
    right_limits: Vec<RightLimit>,
    labels: Vec<Bounds>,
    envelopes: Vec<Envelope>,
    columns: Vec<ColumnBounds>,
    row_centres: Vec<Vec<f32>>,
    row_support: Vec<Vec<Option<retention::Cell>>>,
    // Exact assembled extents, not invented grid cells or ordinal positions.
    compact_words: Vec<Quad>,
    // A label track is excluded geometrically, even if this particular label
    // was misread. Width is measured from the supporting label observations.
    gutters: Vec<(ColumnBounds, f32)>,
    pub(super) decisions: Option<DecisionTrace>,
}

fn key(crop: &GrayImage) -> (u32, u32, [u8; 32]) {
    (
        crop.width(),
        crop.height(),
        sha256::Hash::hash(crop.as_raw()).to_byte_array(),
    )
}

fn word_crop(photo: &RgbImage, frame: Frame, quad: &Quad, margin: f32) -> GrayImage {
    if margin == 0. {
        crate::split::level_crop_within(photo, frame, quad)
    } else {
        // Historical geometry tests only; production rejects padded recognition.
        level_crop_in(photo, frame)
    }
}

impl Grid {
    // Intersect each horizontal edge with the actual rotated photo polygon.
    // Its projected AABB alone includes triangular areas outside the image.
    fn photo_limits(&self, b: Bounds, writing: WritingFrame) -> Option<(f32, f32)> {
        let (w, h) = self.photo_size?;
        let guard = 0.002; // f32 unprojection must not round outside the raster
        let q = writing.project(&Quad([
            (guard, guard),
            (w as f32 - guard, guard),
            (w as f32 - guard, h as f32 - guard),
            (guard, h as f32 - guard),
        ]));
        let at = |y: f32| -> Option<(f32, f32)> {
            let mut xs = Vec::new();
            for i in 0..4 {
                let (a, c) = (q.0[i], q.0[(i + 1) % 4]);
                if y < a.1.min(c.1) || y > a.1.max(c.1) {
                    continue;
                }
                if (c.1 - a.1).abs() < f32::EPSILON {
                    xs.extend([a.0, c.0]);
                } else {
                    xs.push(a.0 + (c.0 - a.0) * (y - a.1) / (c.1 - a.1));
                }
            }
            Some((
                xs.iter().copied().reduce(f32::min)?,
                xs.iter().copied().reduce(f32::max)?,
            ))
        };
        let (top, bottom) = (at(b.t)?, at(b.b)?);
        Some((top.0.max(bottom.0), top.1.min(bottom.1)))
    }
    /// A directly read numeric prefix and its measured source gap own a local
    /// boundary even when the label is an outlier of the page's column fit.
    /// This adds no reads and cannot create a column, gutter or list envelope.
    fn install_local_prefixes(
        &mut self,
        anchors: &[Anchor],
        columns: &[Column],
        writing: WritingFrame,
    ) {
        for (k, a) in anchors.iter().enumerate() {
            let in_track = columns.iter().any(|c| c.anchors.contains(&k));
            let Some((left, word)) = a.split else {
                // Standalone exclusions still need their existing track support.
                if in_track {
                    self.labels.push(a.bounds);
                }
                continue;
            };
            // Preserve existing supported ambiguous-dot handling. Without a
            // track, require an actual numeric prefix, not a shaped glyph/dot.
            let apply = in_track || a.ordinal.is_some();
            DecisionTrace::push(&mut self.decisions, || {
                json!({"rule":"grid_local_prefix", "status":"evaluated",
                    "source_observation":a.source,"literal":a.literal,"confidence":a.confidence,
                    "ordinal":a.ordinal,"label_quad":writing.unproject(&a.bounds.quad()).0,
                    "word_quad":writing.unproject(&word.quad()).0,"boundary_x":left,
                    "column_track_support":in_track,"applied":apply,"extra_ocr_calls":0,
                    "reason":if in_track {"supported_track_and_read_prefix"}
                        else if apply {"numeric_prefix_and_local_source_gap"}
                        else {"ambiguous_prefix_without_track"}})
            });
            if apply {
                self.limits.push(Limit {
                    left,
                    word,
                    label: Some(super::LabelRead {
                        corners: writing.unproject(&a.bounds.quad()).0,
                        literal: a.literal.clone(),
                        origin: "grid_prefix".into(),
                        ordinal: a.ordinal,
                        dotted: a.literal.contains('.'),
                        ambiguous: false,
                    }),
                });
            }
        }
    }

    pub(super) fn assigned_column(&self, q: &Quad, writing: WritingFrame) -> Option<Quad> {
        let b = Bounds::of(&writing.project(q));
        self.columns
            .iter()
            .filter(|c| c.owns(b))
            .max_by(|a, c| a.left(b.y()).total_cmp(&c.left(b.y())))
            .map(|c| writing.unproject(&c.quad()))
    }
    pub(super) fn cached(&self, crop: &GrayImage) -> Option<LineRead> {
        self.cache
            .lock()
            .expect("per-page OCR cache")
            .get(&key(crop))
            .cloned()
    }

    /// All read owners, including union/extent repair, use this same constraint.
    /// Only x is clipped; top/bottom and their normal reader margin are retained.
    pub(super) fn constrain(
        &self,
        q: &Quad,
        writing: WritingFrame,
        margin: f32,
    ) -> (Quad, Frame, bool) {
        let mut b = Bounds::of(&writing.project(q));
        let original = b;
        let owner = self
            .columns
            .iter()
            .filter(|c| c.owns(b))
            .max_by(|a, b| a.left(original.y()).total_cmp(&b.left(original.y())))
            .copied();
        let photo_limits = self.photo_limits(b, writing);
        let left = self
            .limits
            .iter()
            .filter(|limit| limit.applies(b))
            .map(|limit| limit.left)
            .chain(owner.map(|c| c.left_limit(b)))
            .chain(photo_limits.map(|(l, _)| l))
            .reduce(f32::max);
        let right = self
            .right_limits
            .iter()
            .filter(|limit| {
                b.overlap(limit.word)
                    && b.y() >= limit.word.t
                    && b.y() <= limit.word.b
                    && b.l >= limit.word.l - 0.5 * limit.word.h()
                    && b.l < limit.right - b.h()
            })
            .map(|limit| limit.right)
            .chain(owner.map(|c| c.right_limit(b)))
            .chain(photo_limits.map(|(_, r)| r))
            .reduce(f32::min);
        if left.is_none() && right.is_none() {
            return (q.clone(), writing.crop_frame(q, margin), false);
        }
        b.l = b.l.max(left.unwrap_or(b.l));
        b.r = b.r.min(right.unwrap_or(b.r));
        if b.r - b.l <= 1. {
            return (q.clone(), writing.crop_frame(q, margin), false);
        }
        let effective = if b.l > original.l || b.r < original.r {
            writing.unproject(&b.quad())
        } else {
            q.clone()
        };
        let mut grown = Bounds::of(&writing.project(&writing.with_margin(&effective, margin)));
        grown.l = grown.l.max(left.unwrap_or(grown.l));
        grown.r = grown.r.min(right.unwrap_or(grown.r));
        if let Some(c) = owner {
            grown.l = grown.l.max(c.left_limit(grown));
            grown.r = grown.r.min(c.right_limit(grown));
        }
        let frame = writing.frame_of(&writing.unproject(&grown.quad()));
        let normal = Bounds::of(&writing.project(&writing.with_margin(q, margin)));
        let bounded = grown.l > normal.l || grown.r < normal.r;
        (effective, frame, bounded)
    }

    pub(super) fn label(&self, q: &Quad, writing: WritingFrame) -> Option<super::LabelRead> {
        let b = Bounds::of(&writing.project(q));
        self.limits
            .iter()
            .find(|l| l.applies(b))
            .and_then(|l| l.label.clone())
    }

    #[cfg(test)]
    pub(super) fn exclusion(&self, q: &Quad, writing: WritingFrame) -> Option<&'static str> {
        self.exclusion_read(q, writing, None)
    }

    pub(super) fn exclusion_read(
        &self,
        q: &Quad,
        writing: WritingFrame,
        read: Option<&str>,
    ) -> Option<&'static str> {
        let b = Bounds::of(&writing.project(q));
        if !self.columns.iter().any(|c| c.owns(b))
            && self.gutters.iter().any(|(c, width)| {
                let x = (b.l + b.r) / 2.;
                b.y() >= c.top
                    && b.y() <= c.bottom
                    && x >= c.left(b.y()) - width
                    && x < c.left(b.y())
            })
        {
            return Some("inside_label_gutter");
        }
        if self
            .labels
            .iter()
            .any(|l| b.overlap(*l) && b.w() <= 1.3 * l.w() && b.h() <= 1.3 * l.h())
        {
            return Some("confirmed_standalone_label");
        }
        // No horizontal exclusion: a partial/unsupported second column must
        // remain available. A half-row uncertainty band protects boundary ink.
        // Below the final observed ordinal, a distant single word may be a
        // missed continuation: require prose/wide-line evidence there. This
        // uses literal structure, never membership in the closed vocabulary.
        let prose = read.is_some_and(|s| {
            s.split_whitespace()
                .filter(|s| s.chars().any(char::is_alphabetic))
                .count()
                >= 2
        });
        if self
            .envelopes
            .iter()
            .any(|e| b.l < e.bounds.r && b.r > e.bounds.l)
            && self.envelopes.iter().all(|e| {
                (e.top_supported && b.b < e.bounds.t - if prose { 0. } else { 0.5 * e.step })
                    || (e.terminal_label
                        && b.t > e.bounds.b + 0.5 * e.step
                        && (prose || b.w() > 1.5 * e.bounds.w()))
            })
        {
            return Some("outside_supported_list_rows");
        }
        None
    }
}

fn median(mut ns: Vec<f32>) -> f32 {
    ns.sort_by(f32::total_cmp);
    (ns[(ns.len() - 1) / 2] + ns[ns.len() / 2]) / 2.
}

fn numeric(read: &LineRead) -> Option<Option<u32>> {
    if !read.confidence.is_finite() || read.confidence < 0.9 {
        return None;
    }
    let text = read.text.trim();
    let digits = text.strip_suffix('.').unwrap_or(text);
    if !digits.is_empty() && digits.len() <= 2 && digits.bytes().all(|b| b.is_ascii_digit()) {
        return digits.parse::<u32>().ok().filter(|n| *n > 0).map(Some);
    }
    // Literal ambiguous dot labels are evidence for a location, not an ordinal.
    if text == "."
        || (text.ends_with('.')
            && digits.chars().count() <= 2
            && digits.chars().all(|c| c.is_ascii_alphanumeric()))
    {
        return Some(None);
    }
    None
}

fn regress(points: &[(f32, f32)]) -> Option<(f32, f32)> {
    let x = points.iter().map(|p| p.0).sum::<f32>() / points.len() as f32;
    let y = points.iter().map(|p| p.1).sum::<f32>() / points.len() as f32;
    let den = points.iter().map(|p| (p.0 - x).powi(2)).sum::<f32>();
    if den <= f32::EPSILON {
        return None;
    }
    let slope = points.iter().map(|p| (p.0 - x) * (p.1 - y)).sum::<f32>() / den;
    Some((y - slope * x, slope))
}

fn fit(anchors: &[Anchor]) -> Vec<Column> {
    fit_minimum(anchors, 3)
}

fn fit_minimum(anchors: &[Anchor], minimum: usize) -> Vec<Column> {
    let mut remaining: Vec<usize> = (0..anchors.len()).collect();
    let mut result = Vec::new();
    while remaining.len() >= minimum {
        let scale = median(remaining.iter().map(|&i| anchors[i].bounds.h()).collect());
        let mut best = Vec::new();
        for (pos, &i) in remaining.iter().enumerate() {
            for &j in &remaining[pos + 1..] {
                let (a, b) = (anchors[i].bounds, anchors[j].bounds);
                if (a.y() - b.y()).abs() < scale {
                    continue;
                }
                let slope = (b.r - a.r) / (b.y() - a.y());
                if slope.abs() > 0.3 {
                    continue;
                }
                let intercept = a.r - slope * a.y();
                let mut hits = Vec::new();
                for &k in &remaining {
                    let c = anchors[k].bounds;
                    if (c.r - (intercept + slope * c.y())).abs() <= 0.75 * scale
                        && !hits.iter().any(|&h: &usize| {
                            anchors[h].source == anchors[k].source
                                || (anchors[h].bounds.y() - c.y()).abs() < 0.5 * scale
                        })
                    {
                        hits.push(k);
                    }
                }
                if hits.len() > best.len() {
                    best = hits;
                }
            }
        }
        if best.len() < minimum {
            break;
        }
        let (intercept, slope) = regress(
            &best
                .iter()
                .map(|&k| (anchors[k].bounds.y(), anchors[k].bounds.r))
                .collect::<Vec<_>>(),
        )
        .unwrap();
        if slope.abs() > 0.3 {
            break;
        }
        best.retain(|&k| {
            (anchors[k].bounds.r - intercept - slope * anchors[k].bounds.y()).abs() <= 0.75 * scale
        });
        if best.len() < minimum {
            break;
        }
        let indexed: Vec<_> = best
            .iter()
            .filter_map(|&k| {
                anchors[k]
                    .ordinal
                    .map(|n| (n as f32, anchors[k].bounds.y()))
            })
            .collect();
        let rows = (indexed.len() >= minimum)
            .then(|| regress(&indexed))
            .flatten()
            .filter(|&(a, b)| {
                b >= scale
                    && indexed
                        .iter()
                        .all(|&(n, y)| (y - a - b * n).abs() <= 0.5 * scale)
            });
        remaining.retain(|k| !best.contains(k));
        result.push(Column {
            anchors: best,
            intercept,
            slope,
            height: scale,
            rows,
        });
    }
    result
}

/// Spend scarce reads establishing an unsupported coherent track, rather than
/// repeatedly confirming a column which already has enough independent labels.
/// Proposal geometry only ranks reads: it never authorises a cut or exclusion.
fn schedule(
    proposals: &mut [(usize, Quad, Frame, Bounds, f32)],
    anchors: &[Anchor],
    writing: WritingFrame,
) {
    let supported = fit(anchors);
    let proposed: Vec<_> = proposals
        .iter()
        .map(|p| Anchor {
            source: p.0,
            bounds: from_crop(p.3, p.2, writing),
            ordinal: None,
            literal: String::new(),
            confidence: 0.,
            split: None,
        })
        .collect();
    let tracks = fit(&proposed);
    let rank = |p: &(usize, Quad, Frame, Bounds, f32)| {
        let b = from_crop(p.3, p.2, writing);
        let known = supported
            .iter()
            .any(|c| (b.r - c.intercept - c.slope * b.y()).abs() <= 0.75 * c.height);
        let track = tracks
            .iter()
            .position(|c| c.anchors.iter().any(|&i| proposed[i].source == p.0));
        (
            !(!known && track.is_some()),
            track.unwrap_or(usize::MAX),
            (p.3.w() * p.3.h()) as u64,
            p.0,
        )
    };
    proposals.sort_by_key(rank);
}

/// A compact leading ink group, separated from a taller word component. This
/// is only a proposal until its own Paddle read and cross-row support agree.
#[cfg(test)]
fn prefix(crop: &GrayImage) -> Option<(Bounds, f32)> {
    prefix_ink(binarise(crop))
}

fn prefix_ink(ink: Vec<Vec<bool>>) -> Option<(Bounds, f32)> {
    prefix_ink_ranked(ink, true)
}

fn prefix_ink_ranked(mut ink: Vec<Vec<bool>>, require_rise: bool) -> Option<(Bounds, f32)> {
    strip_rules(&mut ink);
    let height = ink.len();
    let width = ink.first()?.len();
    let mut spans = Vec::new();
    let mut x = 0usize;
    while x < width {
        if !ink.iter().any(|r| r[x]) {
            x += 1;
            continue;
        }
        let l = x;
        let (mut top, mut bottom) = (height, 0);
        while x < width && ink.iter().any(|r| r[x]) {
            for (y, row) in ink.iter().enumerate() {
                if row[x] {
                    top = top.min(y);
                    bottom = bottom.max(y + 1);
                }
            }
            x += 1;
        }
        spans.push(Bounds {
            l: l as f32,
            t: top as f32,
            r: x as f32,
            b: bottom as f32,
        });
    }
    let mut p = *spans.first()?;
    let mut best = None;
    for next in spans.iter().skip(1) {
        let gap = next.l - p.r;
        let rise = next.h() / p.h();
        let score = if require_rise { rise } else { gap / p.h() };
        if p.w() <= 2.2 * p.h()
            && gap >= 0.06 * height as f32
            && rise >= if require_rise { 1.25 } else { 0.5 }
            && best.as_ref().is_none_or(|(_, _, old)| score > *old)
        {
            best = Some((p, gap, score));
        }
        p.r = next.r;
        p.t = p.t.min(next.t);
        p.b = p.b.max(next.b);
    }
    best.map(|(p, gap, _)| (p, gap))
}

/// A separated low full stop terminates the entire leading label, including
/// both digits. Unlike the height-rise proposal, this cannot choose the gap
/// between `1` and `3` in `13.`. It is a location proposal, not an OCR result.
fn dotted_prefix_ink(mut ink: Vec<Vec<bool>>) -> Option<(Bounds, f32)> {
    strip_rules(&mut ink);
    let height = ink.len();
    let width = ink.first()?.len();
    let mut spans = Vec::new();
    let mut x = 0;
    while x < width {
        if !ink.iter().any(|r| r[x]) {
            x += 1;
            continue;
        }
        let l = x;
        let (mut t, mut b) = (height, 0);
        while x < width && ink.iter().any(|r| r[x]) {
            for (y, row) in ink.iter().enumerate() {
                if row[x] {
                    t = t.min(y);
                    b = b.max(y + 1);
                }
            }
            x += 1;
        }
        spans.push(Bounds {
            l: l as f32,
            r: x as f32,
            t: t as f32,
            b: b as f32,
        });
    }
    let mut head = *spans.first()?;
    let line_top = spans.iter().map(|b| b.t).fold(f32::INFINITY, f32::min);
    let line_bottom = spans.iter().map(|b| b.b).fold(f32::NEG_INFINITY, f32::max);
    let line_height = line_bottom - line_top;
    for (i, dot) in spans.iter().enumerate().skip(1) {
        if let Some(word) = spans.get(i + 1) {
            if dot.w() < 0.45 * line_height
                && dot.h() < 0.45 * line_height
                && dot.y() > (line_top + line_bottom) / 2.
                && head.h() >= 0.5 * line_height
                && dot.r - head.l <= 2.2 * head.h()
                && word.h() >= 0.5 * line_height
                && word.l - dot.r >= 0.06 * line_height
            {
                head.r = dot.r;
                head.t = head.t.min(dot.t);
                head.b = head.b.max(dot.b);
                return Some((head, word.l - dot.r));
            }
        }
        head.r = dot.r;
        head.t = head.t.min(dot.t);
        head.b = head.b.max(dot.b);
    }
    None
}

fn from_crop(b: Bounds, f: Frame, writing: WritingFrame) -> Bounds {
    let q = Quad(b.quad().0.map(|(x, y)| {
        let (dx, dy) = turn(x - f.w.round() / 2., y - f.h.round() / 2., f.angle);
        (f.cx + dx, f.cy + dy)
    }));
    Bounds::of(&writing.project(&q))
}

fn source_ink(photo: &RgbImage, q: &Quad, f: Frame, crop: &GrayImage) -> Vec<Vec<bool>> {
    source_ink_with_level(photo, q, f, crop).0
}

fn source_ink_with_level(
    photo: &RgbImage,
    q: &Quad,
    f: Frame,
    crop: &GrayImage,
) -> (Vec<Vec<bool>>, Option<u8>) {
    let valid = (0..crop.height())
        .map(|y| {
            (0..crop.width())
                .map(|x| {
                    let (dx, dy) = turn(
                        x as f32 + 0.5 - crop.width() as f32 / 2.,
                        y as f32 + 0.5 - crop.height() as f32 / 2.,
                        f.angle,
                    );
                    let p = (f.cx + dx, f.cy + dy);
                    let crosses = [0, 1, 2, 3].map(|i| {
                        let (a, b) = (q.0[i], q.0[(i + 1) % 4]);
                        (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0)
                    });
                    p.0 >= 0.
                        && p.1 >= 0.
                        && p.0 < photo.width() as f32
                        && p.1 < photo.height() as f32
                        && (crosses.iter().all(|v| *v >= 0.) || crosses.iter().all(|v| *v <= 0.))
                })
                .collect()
        })
        .collect();
    crate::split::binarise_valid_with_level(crop, valid)
}

// Only the next independently supported column can impose a trailing limit.
// A real local whitespace separator is still required; a fitted x alone is
// never a licence to sever connected word ink.
fn trailing_limits(
    photo: &RgbImage,
    all: &[Quad],
    anchors: &[Anchor],
    columns: &[Column],
    writing: WritingFrame,
    margin: f32,
) -> Vec<RightLimit> {
    let mut result = Vec::new();
    for q in all {
        let word = Bounds::of(&writing.project(q));
        if word.w() < 1.5 * word.h() {
            continue;
        }
        let owner = columns
            .iter()
            // Trailing-only constrained reads skip local prefix processing.
            // Require an already word-side start, not a merged own label.
            .filter(|c| c.intercept + c.slope * word.y() + 0.5 * c.height < word.l)
            .max_by(|a, b| {
                (a.intercept + a.slope * word.y()).total_cmp(&(b.intercept + b.slope * word.y()))
            });
        let Some(owner) = owner else {
            continue;
        };
        let next = columns
            .iter()
            .filter(|c| c.intercept + c.slope * word.y() > word.l + word.h())
            .min_by(|a, b| {
                (a.intercept + a.slope * word.y()).total_cmp(&(b.intercept + b.slope * word.y()))
            });
        let Some(next) = next else {
            continue;
        };
        let in_span = |c: &Column| {
            let lo = c
                .anchors
                .iter()
                .map(|&i| anchors[i].bounds.t)
                .fold(f32::INFINITY, f32::min);
            let hi = c
                .anchors
                .iter()
                .map(|&i| anchors[i].bounds.b)
                .fold(f32::NEG_INFINITY, f32::max);
            word.y() >= lo && word.y() <= hi
        };
        if !in_span(owner) || !in_span(next) {
            continue;
        }
        for &i in &next.anchors {
            let label = anchors[i].bounds;
            if (label.y() - word.y()).abs() > 0.5 * word.h().max(label.h())
                || label.l > word.r + margin * word.h()
                || label.l < word.l + word.h()
            {
                continue;
            }
            let cut = if word.r < label.l {
                // No detection pixels change; keep the reader margin out of
                // the independently detected neighbouring label.
                Some((word.r + label.l) / 2.)
            } else {
                let f = writing.crop_frame(q, margin);
                let crop = level_crop_in(photo, f);
                let ink = source_ink(photo, q, f, &crop);
                let mut gap = 0usize;
                let mut found = None;
                for x in 0..crop.width() as usize {
                    if ink.iter().any(|r| r[x]) {
                        gap = 0;
                        continue;
                    }
                    gap += 1;
                    if gap as f32 >= 0.06 * word.h() {
                        let cut = from_crop(band(x as f32 - gap as f32 / 2.), f, writing).l;
                        if cut <= label.l
                            && cut >= label.l - 0.75 * label.h()
                            && cut > word.l + word.h()
                        {
                            found = Some(cut);
                        }
                    }
                }
                found
            };
            if let Some(right) = cut {
                result.push(RightLimit { right, word });
            }
        }
    }
    result
}

fn band(x: f32) -> Bounds {
    Bounds {
        l: x,
        r: x,
        t: 0.,
        b: 1.,
    }
}

fn column_guides(
    anchors: &[Anchor],
    all: &[Quad],
    writing: WritingFrame,
    minimum: usize,
) -> Vec<(ColumnBounds, serde_json::Value)> {
    let columns = fit_minimum(anchors, minimum);
    let mut guides: Vec<_> = columns.iter().map(|c| {
        let mut top=c.anchors.iter().map(|&i|anchors[i].bounds.t).fold(f32::INFINITY,f32::min);
        let mut bottom=c.anchors.iter().map(|&i|anchors[i].bounds.b).fold(f32::NEG_INFINITY,f32::max);
        let observed=[top,bottom];
        let mut right=c.intercept+c.slope*((top+bottom)/2.);
        let mut row_bounds=HashMap::<i32,Bounds>::new();
        for q in all {
            let b=Bounds::of(&writing.project(q));
            let x=c.intercept+c.slope*b.y();
            if b.w()<1.2*b.h() || b.l<x-2.2*c.height || b.r<x+c.height || b.h()>3.*c.height
                || columns.iter().any(|other|other.intercept+other.slope*b.y()>x+c.height && other.intercept+other.slope*b.y()<b.l) {continue;}
            if b.y()>=observed[0] && b.y()<=observed[1] {
                top=top.min(b.t);bottom=bottom.max(b.b);right=right.max(b.r);
            }
            if let Some((a,step))=c.rows {
                let n=((b.y()-a)/step).round() as i32;
                if n>=1 && (b.y()-a-step*n as f32).abs()<=0.45*step {row_bounds.entry(n).and_modify(|old|{
                    old.l=old.l.min(b.l);old.r=old.r.max(b.r);old.t=old.t.min(b.t);old.b=old.b.max(b.b);
                }).or_insert(b);}
            }
        }
        let mut drawn=Vec::new();
        if let Some((a,step))=c.rows {
            let ns:Vec<_>=c.anchors.iter().filter_map(|&i|anchors[i].ordinal.map(|n|n as i32)).collect();
            let (mut lo,mut hi)=(*ns.iter().min().unwrap(),*ns.iter().max().unwrap());
            while lo>1 && row_bounds.contains_key(&(lo-1)) {lo-=1;}
            while row_bounds.contains_key(&(hi+1)) {hi+=1;}
            for n in lo..=hi {
                if let Some(b)=row_bounds.get(&n) {top=top.min(b.t);bottom=bottom.max(b.b);right=right.max(b.r);}
            }
            for n in lo..=hi {
                let y=a+step*n as f32;
                let q=writing.unproject(&Bounds {l:c.intercept+c.slope*y,r:right,t:y,b:y}.quad());
                drawn.push(json!({"ordinal":n,"line":[q.0[0],q.0[1]],"observed_label":ns.contains(&n)}));
            }
        }
        // A neighbouring column's labels are outside this column's word area.
        if let Some(next)=columns.iter().filter(|other|other.intercept+other.slope*((top+bottom)/2.)>c.intercept+c.slope*((top+bottom)/2.)+c.height)
            .min_by(|a,b|a.intercept.total_cmp(&b.intercept)) {
            let width=next.anchors.iter().map(|&i|anchors[i].bounds.w()).fold(0.,f32::max);
            right=right.min((next.intercept+next.slope*top).min(next.intercept+next.slope*bottom)-width);
        }
        let bounds=ColumnBounds { right_slope: 0.,intercept:c.intercept,slope:c.slope,right,top,bottom};
        let q=writing.unproject(&bounds.quad());
        (bounds,json!({"quad":q.0,"rows":drawn,"display_only":minimum<3,"supports":c.anchors.len(),"observed_y_span":observed}))
    }).filter(|(b,_)| b.right>b.left(b.top).max(b.left(b.bottom))+1.).collect();
    // These are columns of ONE list, not independent little lists bounded by
    // whichever labels happened to read successfully. Share the observed list
    // extent, then refit widths against words throughout that entire extent.
    let top = guides
        .iter()
        .map(|(b, _)| b.top)
        .fold(f32::INFINITY, f32::min);
    let bottom = guides
        .iter()
        .map(|(b, _)| b.bottom)
        .fold(f32::NEG_INFINITY, f32::max);
    for (bounds, view) in &mut guides {
        bounds.top = top;
        bounds.bottom = bottom;
        let c = columns
            .iter()
            .find(|c| c.intercept == bounds.intercept && c.slope == bounds.slope)
            .unwrap();
        // Label rectangles can extend into a separately detected word. Fit
        // one separating line inside the observed label/word corridor before
        // enforcing it; do not give individual word crops an exemption.
        let projected: Vec<_> = all
            .iter()
            .map(|q| Bounds::of(&writing.project(q)))
            .collect();
        let mut shift = 0.0_f32;
        for word in &projected {
            let x = bounds.left(word.y());
            if word.y() < top
                || word.y() > bottom
                || word.w() < 2.2 * word.h()
                || word.l < x - 0.75 * c.height
                || word.l > x + c.height
            {
                continue;
            }
            let separate_label = projected.iter().any(|label| {
                let centre = (label.l + label.r) / 2.;
                label.w() <= 2.2 * label.h()
                    && label.h() <= 2. * c.height
                    && (label.y() - word.y()).abs() < 0.5 * word.h()
                    && centre < word.l
                    && centre < x
                    && centre >= x - 2.2 * c.height
            });
            if separate_label {
                shift = shift.max(bounds.left_limit(*word) - word.l);
            }
        }
        bounds.intercept -= shift;
        view["separator_corridor_shift"] = json!(shift);
        view["label_gutter_width"] = json!(
            c.anchors
                .iter()
                .map(|&i| anchors[i].bounds.w())
                .fold(0., f32::max)
                + 0.75 * c.height
        );
        for q in all {
            let b = Bounds::of(&writing.project(q));
            let x = bounds.left(b.y());
            if b.y() >= top
                && b.y() <= bottom
                && b.w() >= 1.2 * b.h()
                && b.h() <= 3. * c.height
                && b.l >= x - 2.2 * c.height
                && b.r > x + c.height
                && !columns.iter().any(|other| {
                    other.intercept + other.slope * b.y() > x + c.height
                        && other.intercept + other.slope * b.y() < b.l
                })
            {
                bounds.right = bounds.right.max(b.r);
            }
        }
        if let Some(next) = columns
            .iter()
            .filter(|other| {
                other.intercept + other.slope * ((top + bottom) / 2.)
                    > bounds.left((top + bottom) / 2.) + c.height
            })
            .min_by(|a, b| a.intercept.total_cmp(&b.intercept))
        {
            let width = next
                .anchors
                .iter()
                .map(|&i| anchors[i].bounds.w())
                .fold(0., f32::max)
                + 0.75 * next.height;
            bounds.right = bounds.right.min(
                (next.intercept + next.slope * top).min(next.intercept + next.slope * bottom)
                    - width,
            );
        }
        view["quad"] = json!(writing.unproject(&bounds.quad()).0);
        view["shared_list_y_span"] = json!([top, bottom]);
        if let Some((a, step)) = c.rows {
            let lo = (((top - a) / step).ceil() as i32).max(1);
            let hi = ((bottom - a) / step).floor() as i32;
            view["rows"] = json!((lo..=hi).map(|n| {
                let y = a + step * n as f32;
                let q = writing.unproject(&Bounds {l:bounds.left(y),r:bounds.right,t:y,b:y}.quad());
                json!({"ordinal":n,"line":[q.0[0],q.0[1]],
                    "observed_label":c.anchors.iter().any(|&i| anchors[i].ordinal == Some(n as u32))})
            }).collect::<Vec<_>>());
        }
    }
    guides
}

pub(super) fn build(
    photo: &RgbImage,
    ordinary: &[(usize, Quad)],
    all: &[Quad],
    writing: WritingFrame,
    margin: f32,
    budget: &mut Budget,
    trace: bool,
    progress: &Stage<'_>,
    mut recognise: impl FnMut(&GrayImage) -> Result<LineRead, String>,
) -> Result<Grid, String> {
    let _time = crate::timing::span("phrase.label_grid");
    let initial_extra_calls = budget.ocrs;
    let initial_extra_pixels = budget.read_pixels;
    let mut early_whole_reads = 0usize;
    let mut grid = Grid {
        photo_size: Some(photo.dimensions()),
        decisions: trace.then(DecisionTrace::default),
        ..Grid::default()
    };
    let mut anchors = Vec::new();
    let mut display_anchors = Vec::new();
    let mut proposals = Vec::new();
    for (source, q) in ordinary {
        let bounds = Bounds::of(&writing.project(q));
        let f = writing.crop_frame(q, margin);
        crate::read_budget::pixels_ceil(f).map_err(str::to_owned)?;
        let crop = word_crop(photo, f, q, margin);
        if bounds.w() <= 2.2 * bounds.h() {
            progress.region(*source, q, RegionState::Reading);
            // This replaces this exact region's later ordinary whole OCR call.
            let _read_time = crate::timing::span("phrase.label_grid.whole_ocr");
            early_whole_reads += 1;
            let read = recognise(&crop)?;
            grid.cache.lock().unwrap().insert(key(&crop), read.clone());
            let accepted = numeric(&read);
            if read.confidence >= 0.5 && read.confidence.is_finite() {
                if let Some(ordinal) = numeric(&LineRead {
                    text: read.text.clone(),
                    confidence: 1.,
                }) {
                    display_anchors.push(Anchor {
                        source: *source,
                        bounds,
                        ordinal,
                        literal: read.text.clone(),
                        confidence: read.confidence,
                        split: None,
                    });
                }
            }
            DecisionTrace::push(&mut grid.decisions, || {
                json!({"rule":"grid_anchor_read", "status":"evaluated",
                "kind":"standalone", "source_observation":source, "quad":q.0,
                "literal":read.text,"confidence":read.confidence,"accepted":accepted.is_some(),
                "orientation_degrees":f.angle,"cost":"reused_whole_read",
                "reader_frame":{"cx":f.cx,"cy":f.cy,"width":f.w,"height":f.h,"angle_degrees":f.angle}})
            });
            if let Some(ordinal) = accepted {
                anchors.push(Anchor {
                    source: *source,
                    bounds,
                    ordinal,
                    literal: read.text,
                    confidence: read.confidence,
                    split: None,
                });
            }
        } else {
            let mut ink = source_ink(photo, q, f, &crop);
            strip_rules(&mut ink);
            let (removed_components, removed_pixels) = ink_width::filter_small(&mut ink);
            let proposal = dotted_prefix_ink(ink.clone())
                .or_else(|| prefix_ink(ink.clone()))
                .or_else(|| prefix_ink_ranked(ink, false));
            DecisionTrace::push(&mut grid.decisions, || {
                json!({"rule":"grid_prefix_components","status":"evaluated",
                    "source_observation":source,"small_cutoff":ink_width::SMALL_PIXELS,
                    "connectivity":8,"removed_components":removed_components,
                    "removed_pixels":removed_pixels,"proposal_found":proposal.is_some(),
                    "mask_only":true,"recognition_pixels_changed":false})
            });
            if let Some((p, gap)) = proposal {
                proposals.push((*source, q.clone(), f, p, gap));
            }
        }
    }
    schedule(&mut proposals, &anchors, writing);
    DecisionTrace::push(&mut grid.decisions, || {
        json!({"rule":"grid_prefix_schedule","status":"evaluated",
        "policy":"unsupported_coherent_track_first","proposals":proposals.iter().map(|p|json!({
            "source_observation":p.0,"quad":writing.unproject(&from_crop(p.3,p.2,writing).quad()).0
        })).collect::<Vec<_>>()})
    });
    let all_proposals = proposals.clone();
    let mut read_sources = Vec::new();
    for (source, q, f, p, gap) in proposals {
        let pad = 0.15 * p.h();
        let l = (p.l - pad).floor().max(0.) as u32;
        let t = (p.t - pad).floor().max(0.) as u32;
        let r = (p.r + pad.min(gap / 2.)).ceil().min(f.w.round()) as u32;
        let b = (p.b + pad).ceil().min(f.h.round()) as u32;
        if r <= l || b <= t {
            continue;
        }
        if let Err(reason) = budget.crop(u64::from(r - l) * u64::from(b - t)) {
            DecisionTrace::push(&mut grid.decisions, || {
                json!({"rule":"grid_prefix_read",
                "source_observation":source,"status":"skipped","reason":reason})
            });
            break;
        }
        budget.ocr()?;
        let whole = word_crop(photo, f, &q, margin);
        let crop = image::imageops::crop_imm(&whole, l, t, r - l, b - t).to_image();
        let _read_time = crate::timing::span("phrase.label_grid.prefix_ocr");
        let read = recognise(&crop)?;
        let accepted = numeric(&read);
        read_sources.push(source);
        let bounds = from_crop(p, f, writing);
        let (rx, ry) = turn(
            (l + r) as f32 / 2. - whole.width() as f32 / 2.,
            (t + b) as f32 / 2. - whole.height() as f32 / 2.,
            f.angle,
        );
        DecisionTrace::push(&mut grid.decisions, || {
            json!({"rule":"grid_prefix_read",
            "source_observation":source,"status":"evaluated","quad":writing.unproject(&bounds.quad()).0,
            "literal":read.text,"confidence":read.confidence,"accepted":accepted.is_some(),
            "orientation_degrees":f.angle,"pixels":(r-l)*(b-t),
            "reader_frame":{"cx":f.cx+rx,"cy":f.cy+ry,"width":r-l,"height":b-t,"angle_degrees":f.angle}})
        });
        if let Some(ordinal) = accepted {
            let mut word = Bounds::of(&writing.project(&q));
            let cut = from_crop(
                Bounds {
                    l: p.r + gap / 2.,
                    r: p.r + gap / 2.,
                    t: 0.,
                    b: 1.,
                },
                f,
                writing,
            )
            .l;
            word.l = cut;
            anchors.push(Anchor {
                source,
                bounds,
                ordinal,
                literal: read.text,
                confidence: read.confidence,
                split: Some((cut, word)),
            });
        }
    }
    let _fit_time = crate::timing::span("phrase.label_grid.fit");
    display_anchors.extend(anchors.iter().filter(|a| a.split.is_some()).cloned());
    let mut columns = fit(&anchors);
    // The same two-anchor, spatially fitted grid that is drawn is enforced.
    // Confidence of an individual later label cannot exempt its gutter.
    let mut guides = column_guides(&display_anchors, all, writing, 2);
    dots::reconcile(
        photo,
        all,
        writing,
        &mut guides,
        &mut grid.decisions,
        !anchors.is_empty(),
    );
    layout::complete(
        &mut guides,
        all,
        writing,
        &all_proposals,
        !anchors.is_empty(),
    );
    rows::reconcile(&mut guides, all, writing, &mut grid.decisions);
    layout::extend_right_domains(&mut guides, photo, writing, all);
    let unsupported = guides
        .iter()
        .any(|(_, v)| v["method"] == "estimated_word_starts");
    if unsupported || guides.iter().any(|(_, v)| v["row_fit"]["accepted"] != true) {
        DecisionTrace::push(&mut grid.decisions, || {
            json!({
                "rule":"grid_layout_fallback","status":"evaluated",
                "reason":if unsupported {"column_without_label_separation_evidence"}else{"unresolved_complete_cell_layout"},
                "candidate_guides":guides.iter().map(|(_,v)|v).collect::<Vec<_>>(),
                "enforced":false,"fallback":"ordinary_boxes_and_local_read_prefixes",
                "global_width_cuts":false,"global_exclusion":false
            })
        });
        guides.clear();
        // A local numeric prefix and its actual source gap remain valid.
        // An unresolved global model cannot supply inferred cuts/envelopes.
        columns.clear();
    }
    grid.columns = guides.iter().map(|(b, _)| *b).collect();
    grid.row_centres = guides
        .iter()
        .map(|(column, v)| {
            let mut ys: Vec<f32> = v["rows"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|row| {
                    let p: (f32, f32) = serde_json::from_value(row["line"][0].clone()).unwrap();
                    writing.project(&Quad([p; 4])).0[0].1
                })
                .collect();
            ys.sort_by(f32::total_cmp);
            ys.dedup_by(|a, b| (*a - *b).abs() < 1.);
            if v["row_fit"]["accepted"] != true {
                cells::complete_rows(&mut ys, *column);
            }
            ys
        })
        .collect();
    for ((column, view), centres) in guides.iter_mut().zip(&grid.row_centres) {
        if let Some(rows) = view["rows"].as_array_mut() {
            for &y in centres {
                if rows.iter().any(|row| {
                    let p: (f32, f32) = serde_json::from_value(row["line"][0].clone()).unwrap();
                    (writing.project(&Quad([p; 4])).0[0].1 - y).abs() < 1.
                }) {
                    continue;
                }
                let q = writing.unproject(&Quad([
                    (column.left(y), y),
                    (column.right_at(y), y),
                    (column.right_at(y), y),
                    (column.left(y), y),
                ]));
                rows.push(json!({"line":[q.0[0],q.0[1]],"ordinal":null,"observed_label":false,"extrapolated":true}));
            }
        }
    }
    grid.gutters = guides
        .iter()
        .map(|(b, v)| (*b, v["label_gutter_width"].as_f64().unwrap() as f32))
        .collect();
    grid.install_cells(&guides, &anchors, writing);
    grid.install_local_prefixes(&anchors, &columns, writing);
    // Cross-row geometry can corroborate an UNREAD prefix, but never override
    // a failed literal read. It still needs a local source-mask gap, matching
    // label dimensions, a unique column, and contiguous observed word rows.
    for (source, q, f, p, gap) in all_proposals {
        if read_sources.contains(&source) {
            continue;
        }
        let b = from_crop(p, f, writing);
        let candidates: Vec<_> = columns
            .iter()
            .filter(|c| {
                let Some((origin, step)) = c.rows else {
                    return false;
                };
                let width = median(c.anchors.iter().map(|&i| anchors[i].bounds.w()).collect());
                let n = ((b.y() - origin) / step).round() as i32;
                if n < 1
                    || (b.y() - origin - step * n as f32).abs() > 0.4 * step
                    || (b.r - c.intercept - c.slope * b.y()).abs() > 0.4 * c.height
                    || b.h() < 0.7 * c.height
                    || b.h() > 1.3 * c.height
                    || b.w() < 0.75 * width
                    || b.w() > 1.5 * width
                {
                    return false;
                }
                let nearest = c
                    .anchors
                    .iter()
                    .filter_map(|&i| anchors[i].ordinal.map(|n| n as i32))
                    .min_by_key(|a| (a - n).abs())
                    .unwrap();
                (nearest.min(n)..=nearest.max(n)).all(|row| {
                    all.iter()
                        .filter(|q| {
                            let w = Bounds::of(&writing.project(q));
                            let x = c.intercept + c.slope * w.y();
                            (w.y() - origin - step * row as f32).abs() <= 0.45 * step
                                && w.w() >= 1.2 * w.h()
                                && w.h() >= 0.6 * c.height
                                && w.h() <= 3. * c.height
                                && w.l >= x - 2.2 * c.height
                                && w.r > x + c.height
                                && !columns.iter().any(|other| {
                                    other.intercept + other.slope * w.y() > x + c.height
                                        && other.intercept + other.slope * w.y() < w.l
                                })
                        })
                        .count()
                        == 1
                })
            })
            .collect();
        let supported = candidates.len() == 1;
        DecisionTrace::push(&mut grid.decisions, || {
            json!({"rule":"grid_cross_row_prefix","status":"evaluated",
            "source_observation":source,"applied":supported,"matching_columns":candidates.len(),
            "literal_read":false,"reason":if supported {"unique_indexed_column_and_local_gap"} else {"insufficient_or_competing_geometry"},
            "label_quad":writing.unproject(&b.quad()).0})
        });
        if supported {
            let left = from_crop(band(p.r + gap / 2.), f, writing).l;
            let mut word = Bounds::of(&writing.project(&q));
            word.l = left;
            grid.limits.push(Limit {
                left,
                word,
                label: None,
            });
        }
    }
    grid.right_limits = trailing_limits(photo, all, &anchors, &columns, writing, margin);
    let bounds: Vec<_> = all
        .iter()
        .map(|q| {
            let (q, _, _) = grid.constrain(q, writing, 0.);
            Bounds::of(&writing.project(&q))
        })
        .collect();
    for c in &columns {
        let Some((origin, step)) = c.rows else {
            continue;
        };
        let mut rows: HashMap<i32, Vec<Bounds>> = HashMap::new();
        for &b in &bounds {
            let n = ((b.y() - origin) / step).round() as i32;
            let x = c.intercept + c.slope * b.y();
            // A merged label (e.g. 8. choice) must not end the row sequence.
            if n >= 1
                && (b.y() - origin - step * n as f32).abs() <= 0.45 * step
                && b.w() >= 1.2 * b.h()
                && b.h() >= 0.6 * c.height
                && b.h() <= 3. * c.height
                && b.r > x + c.height
                && b.l >= x - 2.2 * c.height
                && !columns.iter().any(|other| {
                    let ox = other.intercept + other.slope * b.y();
                    ox > x + c.height && ox < b.l
                })
            {
                rows.entry(n).or_default().push(b);
            }
        }
        let ns: Vec<_> = c
            .anchors
            .iter()
            .filter_map(|&k| anchors[k].ordinal.map(|n| n as i32))
            .collect();
        let (mut lo, mut hi) = (*ns.iter().min().unwrap(), *ns.iter().max().unwrap());
        if (lo..=hi).any(|n| rows.get(&n).map_or(0, Vec::len) != 1) {
            continue;
        }
        while lo > 1 && rows.get(&(lo - 1)).is_some_and(|r| r.len() == 1) {
            lo -= 1;
        }
        while rows.get(&(hi + 1)).is_some_and(|r| r.len() == 1) {
            hi += 1;
        }
        let selected: Vec<_> = (lo..=hi).map(|n| rows[&n][0]).collect();
        let e = Bounds {
            l: selected.iter().map(|b| b.l).fold(f32::INFINITY, f32::min),
            r: selected
                .iter()
                .map(|b| b.r)
                .fold(f32::NEG_INFINITY, f32::max),
            t: selected.iter().map(|b| b.t).fold(f32::INFINITY, f32::min) - 0.25 * c.height,
            b: selected
                .iter()
                .map(|b| b.b)
                .fold(f32::NEG_INFINITY, f32::max)
                + 0.25 * c.height,
        };
        // A later row-compatible detection across a gap is evidence against a
        // terminal boundary, not an excuse to silently remove that detection.
        let bottom_supported = ns.contains(&hi) && !rows.keys().any(|n| *n > hi);
        grid.envelopes.push(Envelope {
            bounds: e,
            step,
            top_supported: lo == 1,
            bottom_supported,
            terminal_label: ns.contains(&hi),
        });
    }
    drop(_fit_time);
    DecisionTrace::push(&mut grid.decisions, || {
        json!({"rule":"label_grid","status":"evaluated",
        "writing_angle_degrees":writing.angle_or(0.),"authority_support":2,
        "shared_list_extent":true,"drawn_grid_is_enforced":true,
        "early_whole_reads":early_whole_reads,
        "extra_ocr_calls":budget.ocrs-initial_extra_calls,
        "extra_read_pixels":budget.read_pixels-initial_extra_pixels,"label_classifier_calls":0,
            "display_guides":guides.iter().map(|(_,v)|v).collect::<Vec<_>>(),
            "enforced_columns":guides.iter().map(|(c,v)|json!({"quad":writing.unproject(&c.quad()).0,
                "estimated":v.get("estimated").and_then(|v|v.as_bool()).unwrap_or(false),
                "method":v.get("method").cloned().unwrap_or(json!("label_track")),
                "word_layout_members":v.get("word_layout_members"),
                "assignment":"centre_or_word_height_overlap","width_limits_apply_to_all_assigned_boxes":true,
                "height_clipping":false})).collect::<Vec<_>>(),
            "label_gutters":grid.gutters.iter().map(|(c,width)| {
                let q = Quad([(c.left(c.top)-width,c.top),(c.left(c.top),c.top),
                    (c.left(c.bottom),c.bottom),(c.left(c.bottom)-width,c.bottom)]);
                json!({"quad":writing.unproject(&q).0,"rule":"inside_label_gutter"})
            }).collect::<Vec<_>>(),
            "display_anchor_min_confidence":0.5,"display_support":2,
            "anchors":anchors.iter().map(|a|json!({"source_observation":a.source,
                "quad":writing.unproject(&a.bounds.quad()).0,"ordinal":a.ordinal,
                "literal":a.literal,"confidence":a.confidence})).collect::<Vec<_>>(),
            "columns":columns.iter().map(|c|json!({"anchor_indices":c.anchors,
                "label_right_x_intercept":c.intercept,"label_right_x_per_y":c.slope,
                "ordinal_y_fit":c.rows})).collect::<Vec<_>>(),
            "cuts":grid.limits.iter().map(|l|json!({"left":l.left,"word_quad":writing.unproject(&l.word.quad()).0,
                "evidence":if l.label.is_some(){"read_prefix"}else{"cross_row_geometry"}})).collect::<Vec<_>>(),
            "trailing_cuts":grid.right_limits.iter().map(|l|json!({"right":l.right,"word_quad":writing.unproject(&l.word.quad()).0,
                "rule":"supported_next_column_local_gap"})).collect::<Vec<_>>(),
            "envelopes":grid.envelopes.iter().map(|e|json!({"quad":writing.unproject(&e.bounds.quad()).0,
            "row_step":e.step,"top_supported":e.top_supported,"bottom_supported":e.bottom_supported,
            "terminal_label":e.terminal_label,"below_boundary_requires_prose_or_wide_line":true})).collect::<Vec<_>>()
        })
    });
    Ok(grid)
}

#[cfg(test)]
mod tests;
