//! A page of handwritten list words read as a phrase: the detector's
//! polygons cut into words, each word cropped level without padding
//! and read by the word model in one page direction,
//! the page's item numbers found over the whole group and cut off by
//! the run they make, and both reading
//! orders kept beside the numbers', since pages are written down
//! columns and along rows and geometry cannot say which. The
//! harness's `detect` and the phone's phrase scanner both call this,
//! so they cannot drift apart.

use std::path::Path;

use bitcoin_hashes::Hash as _;
use image::{GrayImage, RgbImage};

use crate::detect::{Quad, clockwise};
use crate::hybrid::{CURRENT, Selection, normalise, select};
use crate::numbering::{Label, Numbering, fit, label_of, past_label};
use crate::page_frame::{PageAxis, PhotoUp, WritingFrame};
use crate::progress::{FinalRegion, Observer, RegionState, Stage};
use crate::recogniser::{LineRead, Recogniser};
use crate::sources::{RawSource, Segmentation};
use crate::split::{Split, binarise, split_columns, strip_rules, turn, upside_down};
use crate::vocabulary::Word;
use crate::words::{CANON_RULE, Verdict, WordModel};

mod cuts;
mod decisions;
#[cfg(test)]
mod direction_tests;
mod evidence;
mod grid;
#[doc(hidden)]
pub mod history;
mod labels;
mod order_support;
pub use order_support::OrderSupport;

pub use decisions::DecisionTrace;
pub use evidence::{CellSupport, LabelRead, MatchingRead, OriginalRead, WordEvidence};
pub use labels::label_evidence;

/// Recognition uses only the word box. Improve its geometry, never add a halo.
pub const CROP_MARGIN: f32 = 0.0;

fn require_box_only(margin: f32) -> Result<(), String> {
    if margin != CROP_MARGIN {
        return Err("recognition padding is disabled; change the word box instead".into());
    }
    Ok(())
}

#[test]
fn recognition_rejects_nonzero_or_invalid_padding() {
    assert!(require_box_only(0.).is_ok());
    for margin in [0.15, -0.15, f32::EPSILON, f32::NAN, f32::INFINITY] {
        assert!(require_box_only(margin).is_err());
    }
}

/// What the trainer's calibration artifact promises about a model:
/// the thresholds it selected and the canvas rule it learned, bound
/// to the model bytes by digest.
#[derive(Clone, Debug, PartialEq)]
pub struct Calibration {
    /// A read below this top probability is uncertain.
    pub min_prob: f32,
    /// A read whose top two are closer than this is uncertain.
    pub min_margin: f32,
    /// The SHA-256 of the model the thresholds were selected for.
    pub digest: String,
    /// The canvas rule the model learned under.
    pub rule: String,
}

impl Calibration {
    /// Parses the artifact's `key=value` lines and refuses one whose
    /// canvas rule is not this crate's: a model served another
    /// geometry would still be digest-valid and quietly wrong.
    pub fn parse(text: &str) -> Result<Calibration, String> {
        let (mut min_prob, mut min_margin, mut digest, mut rule) = (None, None, None, None);
        for line in text.lines() {
            match line.split_once('=') {
                Some(("min_prob", v)) => min_prob = v.trim().parse::<f32>().ok(),
                Some(("min_margin", v)) => min_margin = v.trim().parse::<f32>().ok(),
                Some(("model_sha256", v)) => digest = Some(v.trim().to_owned()),
                Some(("canon_rule", v)) => rule = Some(v.trim().to_owned()),
                _ => {}
            }
        }
        let (Some(min_prob), Some(min_margin), Some(digest), Some(rule)) =
            (min_prob, min_margin, digest, rule)
        else {
            return Err(
                "the calibration artifact needs min_prob, min_margin, model_sha256 and canon_rule"
                    .into(),
            );
        };
        if rule != CANON_RULE {
            return Err(format!(
                "the model was trained under canvas rule {rule:?}, this socket canonicalises with {CANON_RULE:?}"
            ));
        }
        for (key, value) in [("min_prob", min_prob), ("min_margin", min_margin)] {
            if !(0.0..=1.0).contains(&value) {
                return Err(format!("{key}={value} is not a probability"));
            }
        }
        Ok(Calibration {
            min_prob,
            min_margin,
            digest,
            rule,
        })
    }
}

/// An original word retained when an extent replacement is selected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtentParent {
    /// Index into the returned page, after all insertion/reindexing.
    pub word_index: usize,
    /// The original keep state, before the replacement hid this word.
    pub stray: bool,
}

/// One word found on the page, read.
#[derive(Clone, Debug)]
pub struct WordBox {
    /// The word's polygon in photo pixels, after the split.
    pub quad: Quad,
    /// The exact crop the model read, in the chosen writing direction and margin.
    pub crop: GrayImage,
    /// Every list word with its probability, most probable first.
    pub ranked: Vec<(usize, f32)>,
    /// One selected identity and its support. None only for geometry-only
    /// diagnostics that deliberately ran no reader, never a fabricated word.
    pub selection: Option<Selection>,
    /// The word's place reading down columns, left to right.
    pub column_rank: usize,
    /// The word's place reading along rows, top to bottom.
    pub row_rank: usize,
    /// Legacy local-frame half-turn after leveling. Generic pages already
    /// include polarity in PageScan.page_direction and never turn independently.
    pub turned: bool,
    /// The recogniser's read of the crop, when one read it: the exact
    /// literal string belonging to this exact crop.
    pub raw: Option<LineRead>,
    /// Immutable label/OCR observations and the separately selected matching text.
    pub evidence: WordEvidence,
    /// Whether the box was narrowed past a leading number.
    pub narrowed: bool,
    /// Not a word: dropped from both reading orders, kept here so a
    /// person can put it back.
    pub stray: bool,
    /// The item number the page's numbering gives the word, when the
    /// page is numbered and this word has a place in the run.
    pub number: Option<u32>,
    /// What the leading run of ink before the word's first gap read
    /// as, or the number standing on its own to the word's left that
    /// labels it: the evidence the numbering was fitted to.
    pub label: Option<String>,
    /// A word by its read, but far from the phrase's region: left out
    /// as a stray for that reason, a key cap under the page.
    pub apart: bool,
    /// Original pieces replaced by this union, indexed into the returned page.
    /// They remain inactive, intact and recoverable; they conflict with this word.
    pub joined_from: Option<[usize; 2]>,
    /// One or two originals replaced by an extent expansion, not a native join.
    /// Original crops/reads remain intact; their prior keep state is restorable.
    pub expanded_from: Vec<ExtentParent>,
}

impl WordBox {
    /// Established word geometry cannot be replaced by later OCR repair.
    /// An ordinal is optional: a local prefix or fragment assembly also owns
    /// its detected ink even when no full label grid could be established.
    pub(crate) fn geometry_owned(&self) -> bool {
        self.evidence.cell.is_some()
    }

    /// Selected word, absent on an explicitly unread geometry-only observation.
    pub fn pick(&self) -> Option<usize> {
        self.selection.as_ref().map(Selection::word)
    }

    /// Confirmation of that selected word, never another classifier identity.
    pub fn verdict(&self) -> Verdict {
        self.selection
            .as_ref()
            .map_or(Verdict::Uncertain, Selection::verdict)
    }
}

#[cfg(test)]
impl WordBox {
    // Pure guard fixtures can request either confirmation state, but must build
    // it through the actual selector with explicit test calibration. An accepted
    // fixture cannot name a different word from its classifier top.
    pub(crate) fn select_fixture(&mut self, chosen: usize, accepted: bool) {
        let text = Word::from_index(chosen as u16).unwrap().as_str().to_owned();
        self.evidence.matching = Some(MatchingRead {
            text: text.clone(),
            original: false,
            removed_prefix_bytes: 0,
        });
        let threshold = if accepted { 0.0 } else { 1.0 };
        self.selection = Some(
            select(
                self.raw.as_ref().map(|r| r.text.as_str()),
                Some(&text),
                &self.ranked,
                threshold,
                threshold,
            )
            .unwrap(),
        );
        assert_eq!(self.pick(), Some(chosen));
        assert_eq!(self.verdict() == Verdict::Accept, accepted);
    }
}

/// A page read: every word, listed in column order.
#[derive(Clone, Debug)]
pub struct PageScan {
    /// One generic-page writing decision, including empty-page fallbacks.
    /// None for registered sheets and historical/unrecorded stage constructors.
    pub page_direction: Option<crate::page_frame::PageDirection>,
    /// The photo's width.
    pub width: u32,
    /// The photo's height.
    pub height: u32,
    /// The words, in column order; `row_rank` gives the other order.
    pub words: Vec<WordBox>,
    /// What the item numbers beside the words made, over the whole
    /// page.
    pub numbering: Numbering,
    /// Bounded join decisions, including refused two-piece rows; no extra crops.
    pub join_trials: Vec<crate::joins::JoinTrial>,
    /// Raw detector geometry and every split outcome. Word links follow all
    /// traversal/replacement reindexing; empty sources have no invented read.
    /// Empty for registered worksheet reads, which do not use the detector.
    pub sources: Vec<RawSource>,
}

/// The initial traversal to present, before a person changes the order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InitialOrder {
    /// Held item numbers take precedence over both geometric traversals.
    Numbers,
    /// Supported rows, or an explicitly unverified row-order suggestion.
    Rows,
    /// Supported columns, identical traversals, or an unverified fallback.
    Columns,
}

impl InitialOrder {
    /// Stable name used by numeric stage diagnostics.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Numbers => "numbers",
            Self::Rows => "rows",
            Self::Columns => "columns",
        }
    }
}

/// The shared stage's initial choice and checksum observations, not word repair.
#[derive(Clone, Debug, PartialEq)]
pub struct InitialTraversal {
    /// Which traversal to present initially.
    pub order: InitialOrder,
    /// All source indices, including strays so restoring one preserves its place.
    pub indices: Vec<usize>,
    /// Whether the kept column-order picks pass the checksum.
    pub columns_pass: bool,
    /// Whether the kept row-order picks pass the checksum.
    pub rows_pass: bool,
    /// Whether the kept number-order picks pass the checksum.
    pub numbered_pass: bool,
    /// True when neither numbering nor geometry establishes the suggested order.
    pub requires_review: bool,
    /// Stable explanation, distinct from checksum and recognition confidence.
    pub basis: &'static str,
    /// Measured whitespace evidence; contains no expected words or checksum input.
    pub layout_support: OrderSupport,
}

impl PageScan {
    /// Positive list evidence is independent of a trusted numeric traversal.
    pub fn list_mode(&self) -> crate::numbering::dotted::ListMode {
        crate::numbering::dotted::ListMode::from_evidence(
            self.words
                .iter()
                .any(|w| w.evidence.labels.iter().any(|l| l.dotted)),
            &self.numbering,
        )
    }

    /// Shared insertion permutation for repair, joins and extent. Existing
    /// observations are never removed; every provenance/number/progress link
    /// follows the same old-to-new map.
    pub(crate) fn reindex_words(&mut self, progress: &mut Stage<'_>) -> Vec<usize> {
        let mut order: Vec<_> = (0..self.words.len()).collect();
        order.sort_by_key(|&i| self.words[i].column_rank);
        let mut inverse = vec![0; order.len()];
        for (new, &old) in order.iter().enumerate() {
            inverse[old] = new;
        }
        progress.reindex(&inverse);
        crate::sources::reindex(&mut self.sources, &inverse);
        self.words.sort_by_key(|w| w.column_rank);
        for (i, w) in self.words.iter_mut().enumerate() {
            if let Some(trace) = &mut w.evidence.decisions {
                trace.reindex(&inverse);
            }
            w.column_rank = i;
            w.joined_from = w.joined_from.map(|pair| pair.map(|p| inverse[p]));
            for parent in &mut w.expanded_from {
                parent.word_index = inverse[parent.word_index];
            }
        }
        for (rank, &i) in self.by_rows().iter().enumerate() {
            self.words[i].row_rank = rank;
        }
        for trial in &mut self.join_trials {
            trial.parents = trial.parents.map(|p| inverse[p]);
        }
        if let Numbering::Held {
            numbers,
            unresolved,
            ..
        } = &mut self.numbering
        {
            let mut mapped = vec![None; order.len()];
            for (old, number) in numbers.iter().enumerate() {
                mapped[inverse[old]] = *number;
            }
            *numbers = mapped;
            for i in unresolved {
                *i = inverse[*i];
            }
        }
        inverse
    }

    /// Numbering takes precedence; otherwise read the longer layout dimension.
    /// Checksum observations never select or certify a traversal.
    /// Checksum observations exclude strays; the chosen indices retain them.
    pub fn initial_traversal(&self) -> InitialTraversal {
        let columns: Vec<usize> = (0..self.words.len()).collect();
        let rows = self.by_rows();
        let numbers = self.by_numbers();
        let passes = |order: &[usize]| {
            let words: Vec<Word> = order
                .iter()
                .filter(|&&i| !self.words[i].stray)
                .filter_map(|&i| {
                    self.words[i]
                        .pick()
                        .and_then(|p| Word::from_index(p as u16))
                })
                .collect();
            crate::vocabulary::checksum_ok(&words)
        };
        let (columns_pass, rows_pass, numbered_pass) =
            (passes(&columns), passes(&rows), passes(&numbers));
        let kept = |order: &[usize]| {
            order
                .iter()
                .copied()
                .filter(|&i| !self.words[i].stray)
                .collect::<Vec<_>>()
        };
        let quads: Vec<_> = self
            .words
            .iter()
            .filter(|w| !w.stray)
            .map(|w| w.quad.clone())
            .collect();
        let writing = self
            .page_direction
            .as_ref()
            .map_or(WritingFrame::Local, |d| WritingFrame::Page(d.direction()));
        let layout_support = OrderSupport::measure(&quads, writing);
        let (order, requires_review, basis) = order_support::decide(
            self.numbered(),
            kept(&columns) == kept(&rows),
            &layout_support,
        );
        InitialTraversal {
            order,
            indices: match order {
                InitialOrder::Numbers => numbers,
                InitialOrder::Rows => rows,
                InitialOrder::Columns => columns,
            },
            columns_pass,
            rows_pass,
            numbered_pass,
            requires_review,
            basis,
            layout_support,
        }
    }

    /// The words' indices in row order.
    pub fn by_rows(&self) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.words.len()).collect();
        order.sort_by_key(|&i| self.words[i].row_rank);
        order
    }

    /// The words' indices in the order the page's numbers give: the
    /// numbered words by number, then the rest in column order. The
    /// column order itself when the page is not numbered.
    pub fn by_numbers(&self) -> Vec<usize> {
        let mut numbered: Vec<usize> = (0..self.words.len())
            .filter(|&i| self.words[i].number.is_some())
            .collect();
        numbered.sort_by_key(|&i| self.words[i].number);
        numbered.extend((0..self.words.len()).filter(|&i| self.words[i].number.is_none()));
        numbered
    }

    /// Whether the page's numbers were read as a run.
    pub fn numbered(&self) -> bool {
        self.numbering.has_held_sequence()
    }

    /// The kept words' indices in column order.
    pub fn kept_columns(&self) -> Vec<usize> {
        (0..self.words.len())
            .filter(|&i| !self.words[i].stray)
            .collect()
    }

    /// The kept words' indices in row order.
    pub fn kept_rows(&self) -> Vec<usize> {
        self.by_rows()
            .into_iter()
            .filter(|&i| !self.words[i].stray)
            .collect()
    }

    /// The kept words' indices in the numbers' order.
    pub fn kept_numbers(&self) -> Vec<usize> {
        self.by_numbers()
            .into_iter()
            .filter(|&i| !self.words[i].stray)
            .collect()
    }
}

/// The trim's constants: a leading number sits before the first low
/// gap the splitter finds at these settings.
const TRIM: Split = Split {
    factor: 1.0,
    gap: 0.08,
    rejoin: 0.0,
    pad: 0.0,
};

/// Whether an entrant read begins with a digit, as the Python's
/// `^\d+[\W_]*` matches.
pub fn leading_number(raw: &str) -> bool {
    raw.chars().next().is_some_and(|c| c.is_ascii_digit())
}

/// Where the crop past its leading run of components begins, in crop
/// pixels, or `None` when there is nothing to cut or nothing left.
pub fn trim_offset(crop: &GrayImage) -> Option<u32> {
    let mut ink = binarise(crop);
    strip_rules(&mut ink);
    let spans = split_columns(&ink, TRIM);
    let x0 = spans.get(1)?.0;
    (x0 + 2 < crop.width() as usize).then_some(x0 as u32)
}

/// The word polygon with its leading edge moved past the cut, through
/// the level frame the crop was cut in: the crop is the frame's box
/// grown by `margin` of its height on every side, so a cut `cut` pixels
/// into the crop sits `cut` less the margin along the frame's writing
/// direction from the box's leading edge.
pub fn narrowed(quad: &Quad, cut: f32, margin: f32) -> Quad {
    narrowed_in(quad, cut, margin, WritingFrame::Local)
}

fn narrowed_in(quad: &Quad, cut: f32, margin: f32, writing: WritingFrame) -> Quad {
    let f = writing.frame_of(quad);
    let shift = cut - margin * f.h;
    if f.w <= 0.0 || shift <= 0.0 {
        return quad.clone();
    }
    let shift = shift.min(f.w - 1.0);
    let (hw, hh) = (f.w / 2.0, f.h / 2.0);
    let corners = [(-hw + shift, -hh), (hw, -hh), (hw, hh), (-hw + shift, hh)].map(|(lx, ly)| {
        let (dx, dy) = turn(lx, ly, f.angle);
        (f.cx + dx, f.cy + dy)
    });
    Quad(clockwise(corners))
}

/// What the trim decides for a box.
#[derive(Clone, Debug, PartialEq)]
pub struct Trim {
    /// The narrowed crop and its reads stand in place of the whole.
    pub narrowed: bool,
    /// The exact entrant string carried forward, one of the two given.
    pub raw: String,
    /// The box is dropped outright.
    pub stray: bool,
}

/// Whether the cut past a leading number stands, for a box whose
/// entrant read begins with a digit and whose trimmed crop the entrant
/// read as `raw2` (`None` when there was nothing to cut): the box's own
/// decision, used when the page's numbers make no run to decide by.
/// The word model never decides; its reads follow the cut.
pub fn trim_decision(raw: &str, raw2: Option<&str>) -> Trim {
    cuts::trim_decision_traced(raw, raw2, &mut None)
}

/// The word model with the calibration selected on it: the only way
/// to read a crop, so thresholds never meet a model they were not
/// chosen for.
#[derive(Clone)]
pub struct Reader {
    model: WordModel,
    calibration: Calibration,
    decision_trace: bool,
    photo_up: PhotoUp,
    pub(crate) writing: WritingFrame,
    page_decisions: Option<DecisionTrace>,
    grid: Option<std::sync::Arc<grid::Grid>>,
}

impl Reader {
    /// Binds model bytes to their calibration, refusing bytes whose
    /// digest is not the one the calibration names before the model
    /// is even loaded.
    pub fn bind(model: &[u8], calibration: Calibration) -> Result<Reader, String> {
        let digest = bitcoin_hashes::sha256::Hash::hash(model).to_string();
        if digest != calibration.digest {
            return Err(format!(
                "model digest {digest} does not match the calibration artifact's {}",
                calibration.digest
            ));
        }
        Ok(Reader {
            model: WordModel::from_bytes(model)?,
            calibration,
            decision_trace: false,
            photo_up: PhotoUp::Unknown,
            writing: WritingFrame::Local,
            page_decisions: None,
            grid: None,
        })
    }

    /// [`Reader::bind`] from the calibration artifact's text.
    pub fn from_bytes(model: &[u8], calibration: &str) -> Result<Reader, String> {
        Reader::bind(model, Calibration::parse(calibration)?)
    }

    /// [`Reader::bind`] with the model file at `model`; its errors
    /// name the file.
    pub fn open(model: &Path, calibration: Calibration) -> Result<Reader, String> {
        let named = |e: String| format!("{}: {e}", model.display());
        let bytes = std::fs::read(model).map_err(|e| named(e.to_string()))?;
        Reader::bind(&bytes, calibration).map_err(named)
    }

    /// [`Reader::open`] with the calibration file read and checked
    /// first, so a bad artifact is reported whether or not the model
    /// exists.
    pub fn load(model: &Path, calibration: &Path) -> Result<Reader, String> {
        let text = std::fs::read_to_string(calibration)
            .map_err(|e| format!("{}: {e}", calibration.display()))?;
        let calibration =
            Calibration::parse(&text).map_err(|e| format!("{}: {e}", calibration.display()))?;
        Reader::open(model, calibration)
    }

    /// The model.
    pub fn model(&self) -> &WordModel {
        &self.model
    }

    /// The calibration bound to it.
    pub fn calibration(&self) -> &Calibration {
        &self.calibration
    }

    /// Opt in to crop/decision evidence on reads made with this reader. Default
    /// scans retain no such history; enabling this never adds reader calls.
    pub fn with_decision_trace(mut self, enabled: bool) -> Self {
        self.decision_trace = enabled;
        self
    }

    /// Provenance of already-canonical photo-up for generic calls with this reader.
    /// This does not transform pixels, and defaults to Unknown for legacy callers.
    pub fn with_photo_up(mut self, photo_up: PhotoUp) -> Self {
        self.photo_up = photo_up;
        self
    }
}

/// A box read once, before the page decides what to cut: its chosen-direction
/// crop (local half-turn only on the registered/legacy path), the model's read, the
/// recogniser's read, and where its leading run of ink ends with what
/// that run reads as.
struct Prepared {
    cell: Option<evidence::CellSupport>,
    writing: WritingFrame,
    quad: Quad,
    crop: GrayImage,
    ranked: Vec<(usize, f32)>,
    turned: bool,
    read: Option<LineRead>,
    cut: Option<u32>,
    token: Option<LineRead>,
    decisions: Option<DecisionTrace>,
}

fn prepare(
    photo: &RgbImage,
    word: &Quad,
    reader: &Reader,
    recogniser: Option<&Recogniser>,
    margin: f32,
    labels: bool,
) -> Result<Prepared, String> {
    prepare_inner(photo, word, reader, recogniser, margin, labels, None)
}

fn prepare_inner(
    photo: &RgbImage,
    word: &Quad,
    reader: &Reader,
    recogniser: Option<&Recogniser>,
    margin: f32,
    labels: bool,
    budget: Option<&mut crate::read_budget::Budget>,
) -> Result<Prepared, String> {
    require_box_only(margin)?;
    let (mut effective, mut frame, bounded) = reader.grid.as_ref().map_or_else(
        || (word.clone(), reader.writing.crop_frame(word, margin), false),
        |grid| grid.constrain(word, reader.writing, margin),
    );
    // Finalize ink-side geometry BEFORE either reader. A post-read cosmetic
    // shrink would attach probabilities to a different box from the output.
    let ink_trim = reader
        .grid
        .as_ref()
        .and_then(|g| g.ink_width(photo, &effective, reader.writing));
    if let Some((quad, _)) = &ink_trim {
        effective = quad.clone();
        frame = reader.writing.crop_frame(&effective, margin);
    }
    let assembled = reader
        .grid
        .as_ref()
        .is_some_and(|g| g.compact_word(&effective));
    let mut prepared = prepare_frame_in(
        photo,
        &effective,
        |crop| reader.model.read(crop),
        recogniser.map(|r| {
            move |crop: &GrayImage| {
                if let Some(read) = reader.grid.as_ref().and_then(|g| g.cached(crop)) {
                    return Ok(read);
                }
                r.read(crop)
            }
        }),
        margin,
        labels && !bounded && !assembled,
        reader.decision_trace,
        budget,
        reader.writing,
        reader.page_decisions.as_ref(),
        bounded.then_some(frame),
    )?;
    if assembled {
        DecisionTrace::push(&mut prepared.decisions, || {
            serde_json::json!({
            "rule":"compact_prefix_protection","status":"evaluated",
            "recognition_quad":effective.0,"local_prefix_cut_disabled":true,
            "reason":"gap_is_between_owned_fragments_not_a_confirmed_label"})
        });
    }
    if let Some((quad, ruling)) = ink_trim {
        DecisionTrace::push(&mut prepared.decisions, || {
            serde_json::json!({
                "rule":"grid_ink_width","status":"evaluated","input_quad":word.0,
                "recognition_quad":quad.0,"final_quad":quad.0,"before_recognition":true,
                "height_unchanged":true,"mask":"source_otsu_context_rule_stripped","ruling_bands":ruling
            })
        });
    }
    if let Some(column) = reader
        .grid
        .as_ref()
        .and_then(|g| g.assigned_column(word, reader.writing))
    {
        DecisionTrace::push(&mut prepared.decisions, || {
            serde_json::json!({
            "rule":"grid_column_assignment","status":"evaluated","column_quad":column.0,
            "original_quad":word.0,"effective_quad":effective.0,"width_enforced":true,
            "crop_changed":bounded,"per_box_label_read_required":false})
        });
    }
    if bounded {
        DecisionTrace::push(&mut prepared.decisions, || {
            serde_json::json!({
            "rule":"grid_crop", "status":"evaluated", "applied":true, "original_quad":word.0,
            "effective_quad":effective.0, "reader_margin_clamped":true,
            "local_prefix_cut_disabled":true})
        });
    }
    prepared.cell = reader
        .grid
        .as_ref()
        .and_then(|g| g.word_cell(photo, &effective, reader.writing));
    DecisionTrace::push(&mut prepared.decisions, || {
        serde_json::json!({
        "rule":"grid_cell_support","status":"evaluated","support":prepared.cell.as_ref().map(evidence::CellSupport::to_json),
        "recognition_independent":true,"extra_ocr_calls":0})
    });
    Ok(prepared)
}

// Actual read/crop owner with injectable readers for model-free parity tests.
#[cfg(test)]
fn prepare_with(
    photo: &RgbImage,
    word: &Quad,
    classify: impl FnMut(&GrayImage) -> Result<Vec<(usize, f32)>, String>,
    recognise: Option<impl FnMut(&GrayImage) -> Result<LineRead, String>>,
    margin: f32,
    labels: bool,
    trace: bool,
    budget: Option<&mut crate::read_budget::Budget>,
) -> Result<Prepared, String> {
    prepare_in(
        photo,
        word,
        classify,
        recognise,
        margin,
        labels,
        trace,
        budget,
        WritingFrame::Local,
        None,
    )
}

#[cfg(test)]
fn prepare_in(
    photo: &RgbImage,
    word: &Quad,
    classify: impl FnMut(&GrayImage) -> Result<Vec<(usize, f32)>, String>,
    recognise: Option<impl FnMut(&GrayImage) -> Result<LineRead, String>>,
    margin: f32,
    labels: bool,
    trace: bool,
    budget: Option<&mut crate::read_budget::Budget>,
    writing: WritingFrame,
    page_decisions: Option<&DecisionTrace>,
) -> Result<Prepared, String> {
    prepare_frame_in(
        photo,
        word,
        classify,
        recognise,
        margin,
        labels,
        trace,
        budget,
        writing,
        page_decisions,
        None,
    )
}

fn prepare_frame_in(
    photo: &RgbImage,
    word: &Quad,
    mut classify: impl FnMut(&GrayImage) -> Result<Vec<(usize, f32)>, String>,
    mut recognise: Option<impl FnMut(&GrayImage) -> Result<LineRead, String>>,
    margin: f32,
    labels: bool,
    trace: bool,
    mut budget: Option<&mut crate::read_budget::Budget>,
    writing: WritingFrame,
    page_decisions: Option<&DecisionTrace>,
    frame_override: Option<crate::split::Frame>,
) -> Result<Prepared, String> {
    use serde_json::json;
    let _prepare = crate::timing::span("phrase.prepare");
    let mut decisions = trace.then(DecisionTrace::default);
    if trace {
        DecisionTrace::append(&mut decisions, page_decisions.cloned());
    }
    let _crop = crate::timing::span("phrase.level_crop");
    let frame = frame_override.unwrap_or_else(|| writing.crop_frame(word, margin));
    crate::read_budget::pixels_ceil(frame).map_err(str::to_owned)?;
    let mut crop = if margin == 0. {
        crate::split::level_crop_within(photo, frame, word)
    } else {
        // Internal historical geometry tests only; production prepare rejects
        // nonzero margins before any model call.
        crate::split::level_crop_in(photo, frame)
    };
    drop(_crop);
    let grown_quad = if frame_override.is_some() {
        Quad(
            [
                (-frame.w / 2., -frame.h / 2.),
                (frame.w / 2., -frame.h / 2.),
                (frame.w / 2., frame.h / 2.),
                (-frame.w / 2., frame.h / 2.),
            ]
            .map(|(x, y)| {
                let (dx, dy) = turn(x, y, frame.angle);
                (frame.cx + dx, frame.cy + dy)
            }),
        )
    } else {
        writing.with_margin(word, margin)
    };
    DecisionTrace::push(&mut decisions, || {
        json!({"rule":"crop_frame", "status":"evaluated",
        "quad":word.0, "grown_quad":grown_quad.0, "margin":margin, "margin_clamped":frame_override.is_some(),
        "writing_frame":if writing.shared() {"page"} else {"local"},
        "frame":{"cx":frame.cx, "cy":frame.cy, "width":frame.w, "height":frame.h,
            "angle_degrees":frame.angle, "angle_convention":"axis anticlockwise in photo; level rotation is negative axis angle"},
        "crop_size":[crop.width(), crop.height()]})
    });
    let mut classify = |crop: &GrayImage| {
        let _call = if let Some(b) = budget.as_deref_mut() {
            b.classifier()?;
            Some(crate::timing::span("phrase.extent.classifier"))
        } else {
            None
        };
        classify(crop)
    };
    let mut ranked = classify(&crop)?;
    DecisionTrace::push(&mut decisions, || {
        json!({"rule":"classifier_read", "status":"evaluated",
        "view":"level", "ranked":decisions::ranks(&ranked)})
    });
    let mut turned = false;
    // Only the registered/legacy local-frame path may try its own half-turn.
    // A generic page has already chosen once, even when this region is tall.
    let axis_angle = writing.frame_of(word).angle;
    if !writing.shared() && axis_angle.abs() > 45.0 {
        let flipped = upside_down(&crop);
        let other = classify(&flipped)?;
        let chosen = other.first().map(|r| r.1) > ranked.first().map(|r| r.1);
        DecisionTrace::push(&mut decisions, || {
            json!({"rule":"half_turn", "status":"evaluated",
            "axis_angle_degrees":axis_angle, "absolute_angle_threshold":45.0,
            "comparison":"flipped_top > level_top", "level_top":ranked.first().map(|r| r.1),
            "flipped_top":other.first().map(|r| r.1), "flipped_ranked":decisions::ranks(&other),
            "adopted":chosen})
        });
        if chosen {
            ranked = other;
            crop = flipped;
            turned = true;
        }
    } else {
        DecisionTrace::push(&mut decisions, || {
            json!({"rule":"half_turn", "status":"skipped",
            "reason":if writing.shared() {"shared_page_direction"} else {"axis_not_steep"}, "axis_angle_degrees":axis_angle,
            "absolute_angle_threshold":45.0, "comparison":"local_frame and abs(axis_angle) > threshold", "adopted":false})
        });
    }
    DecisionTrace::push(&mut decisions, || {
        json!({"rule":"reader_orientation", "status":"evaluated",
        "level_rotation_degrees_ccw":-frame.angle, "additional_half_turn":turned,
        "net_rotation_degrees_ccw":-frame.angle + if turned {180.0} else {0.0},
        "crop_size":[crop.width(), crop.height()]})
    });
    let (mut read, mut cut, mut token) = (None, None, None);
    if let Some(recognise) = recognise.as_mut() {
        {
            let _whole = crate::timing::span("phrase.whole_ocr");
            let _call = if let Some(b) = budget.as_deref_mut() {
                b.ocr()?;
                Some(crate::timing::span("phrase.extent.ocr"))
            } else {
                None
            };
            read = Some(recognise(&crop)?);
            DecisionTrace::push(&mut decisions, || {
                json!({"rule":"whole_ocr", "status":"evaluated",
                "literal":read.as_ref().unwrap().text, "confidence":read.as_ref().unwrap().confidence})
            });
        }
        if labels {
            cut = if writing.shared()
                && crate::numbering::dotted::prefix(&read.as_ref().unwrap().text).is_some()
            {
                cuts::dotted_offset(
                    &crop,
                    writing.frame_of(word),
                    &read.as_ref().unwrap().text,
                    &mut decisions,
                )
            } else {
                trim_offset(&crop)
            };
            DecisionTrace::push(&mut decisions, || {
                json!({"rule":"leading_gap", "status":"evaluated",
                "cut_x":cut, "crop_size":[crop.width(), crop.height()]})
            });
            if let Some(cut) = cut {
                let _token = crate::timing::span("phrase.prefix_ocr");
                if let Some(b) = budget.as_deref_mut() {
                    b.crop(u64::from(cut) * u64::from(crop.height()))
                        .map_err(str::to_owned)?;
                    b.ocr()?;
                }
                let leading = image::imageops::crop_imm(&crop, 0, 0, cut, crop.height()).to_image();
                token = Some(recognise(&leading)?);
                DecisionTrace::push(&mut decisions, || {
                    json!({"rule":"token_ocr", "status":"evaluated",
                    "cut_x":cut, "crop_size":[leading.width(), leading.height()],
                    "literal":token.as_ref().unwrap().text, "confidence":token.as_ref().unwrap().confidence})
                });
            } else {
                DecisionTrace::push(
                    &mut decisions,
                    || json!({"rule":"token_ocr", "status":"skipped", "reason":"no_gap"}),
                );
            }
        } else {
            DecisionTrace::push(
                &mut decisions,
                || json!({"rule":"leading_gap", "status":"skipped", "reason":"labels_disabled"}),
            );
            DecisionTrace::push(
                &mut decisions,
                || json!({"rule":"token_ocr", "status":"skipped", "reason":"labels_disabled"}),
            );
        }
    } else {
        for rule in ["whole_ocr", "leading_gap", "token_ocr"] {
            DecisionTrace::push(
                &mut decisions,
                || json!({"rule":rule, "status":"skipped", "reason":"no_recogniser"}),
            );
        }
    }
    Ok(Prepared {
        cell: None,
        writing,
        quad: word.clone(),
        crop,
        ranked,
        turned,
        read,
        cut,
        token,
        decisions,
    })
}

/// The crop past a cut, read again by both readers, for the caller
/// to take or leave.
struct Past {
    crop: GrayImage,
    ranked: Vec<(usize, f32)>,
    read: LineRead,
}

fn past(
    p: &mut Prepared,
    cut: u32,
    reader: &Reader,
    recogniser: &Recogniser,
) -> Result<Past, String> {
    let _past = crate::timing::span("phrase.reread_cut");
    let crop = image::imageops::crop_imm(&p.crop, cut, 0, p.crop.width() - cut, p.crop.height())
        .to_image();
    let read = recogniser.read(&crop)?;
    let ranked = reader.model.read(&crop)?;
    DecisionTrace::push(&mut p.decisions, || {
        serde_json::json!({"rule":"cut_read", "status":"evaluated",
        "cut_x":cut, "crop_size":[crop.width(), crop.height()],
        "literal":read.text, "confidence":read.confidence, "ranked":decisions::ranks(&ranked)})
    });
    Ok(Past { crop, ranked, read })
}

/// Legacy fallback confidence tolerance for cuts without decisive literal word
/// evidence. A label-bearing crop can be confidently wrong, so a corroborated
/// dotted label plus an exact, identity-preserving suffix bypasses this test.
const CUT_TOLERANCE: f32 = 0.15;

/// The keep rule and the pick over a box as it finally reads. A box
/// `excluded` before this point — a number on its own, a box far from
/// the phrase, a trim that dropped it — stays out whatever it reads
/// as; the keep rule can only add to that.
fn finish(
    p: Prepared,
    calibration: &Calibration,
    narrowed: bool,
    excluded: bool,
    number: Option<u32>,
) -> Result<WordBox, String> {
    finish_evidenced(
        p,
        calibration,
        narrowed,
        excluded,
        number,
        WordEvidence::default(),
    )
}

fn finish_evidenced(
    mut p: Prepared,
    calibration: &Calibration,
    narrowed: bool,
    excluded: bool,
    number: Option<u32>,
    mut owned: WordEvidence,
) -> Result<WordBox, String> {
    let selection = select(
        p.read.as_ref().map(|r| r.text.as_str()),
        owned.matching.as_ref().map(|r| r.text.as_str()),
        &p.ranked,
        calibration.min_prob,
        calibration.min_margin,
    )
    .map_err(|e| format!("word selection: {e}"))?;
    let geometric = p.cell.is_some();
    owned.cell = p.cell.take();
    let number = owned.cell.as_ref().and_then(|c| c.ordinal).or(number);
    let stray = if excluded {
        DecisionTrace::push(&mut p.decisions, || {
            serde_json::json!({"rule":"word_like", "site":"finish",
            "status":"skipped", "reason":"already_excluded"})
        });
        true
    } else {
        let recognised = DecisionTrace::eligibility(&mut p.decisions, &selection, "finish");
        DecisionTrace::push(&mut p.decisions, || {
            serde_json::json!({
            "rule":"cell_retention","status":"evaluated","recognition_word_like":recognised,
            "geometry_supported":geometric,"kept":recognised||geometric,
            "recognition_confirmation_unchanged":true,"cell":owned.cell.as_ref().map(evidence::CellSupport::to_json)})
        });
        !(recognised || geometric)
    };
    DecisionTrace::push(&mut p.decisions, || {
        serde_json::json!({"rule":"finish", "status":"evaluated",
        "excluded":excluded, "narrowed":narrowed, "kept":!stray, "number":number,
        "selection":selection.diagnostic(), "scope":"before subsequent replacement/restoration"})
    });
    owned.decisions = p.decisions;
    Ok(WordBox {
        quad: p.quad,
        crop: p.crop,
        ranked: p.ranked,
        selection: Some(selection),
        column_rank: 0,
        row_rank: 0,
        turned: p.turned,
        raw: p.read,
        evidence: owned,
        narrowed,
        stray,
        number,
        label: None,
        apart: false,
        joined_from: None,
        expanded_from: Vec::new(),
    })
}

/// Reads one polygon that holds nothing but its word, as a word
/// sheet's field does: the level crop with the margin, the word model
/// with the half turn for a line on end, and with a recogniser its raw
/// read, the keep rule and the hybrid pick. No number is looked for.
/// The ranks come back zero for the caller to set.
pub fn read_crop(
    photo: &RgbImage,
    word: &Quad,
    reader: &Reader,
    recogniser: Option<&Recogniser>,
    margin: f32,
) -> Result<WordBox, String> {
    let p = prepare(photo, word, reader, recogniser, margin, false)?;
    finish(p, &reader.calibration, false, false, None)
}

#[cfg(feature = "word-extent")]
pub(crate) fn read_extent_crop(
    photo: &RgbImage,
    word: &Quad,
    reader: &Reader,
    recogniser: &Recogniser,
    budget: &mut crate::read_budget::Budget,
) -> Result<WordBox, String> {
    let p = prepare_inner(
        photo,
        word,
        reader,
        Some(recogniser),
        CROP_MARGIN,
        false,
        Some(budget),
    )?;
    finish(p, &reader.calibration, false, false, None)
}

/// Where a box's label came from, which says what may be done about
/// it: only a run of ink that itself read as the label is cut off by
/// ink; a label the read begins with is stripped from the read; a
/// number standing beside the word leaves the word's crop whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Evidence {
    /// Ordinal inferred from an established cell; never evidence of prefix ink.
    Cell(Label),
    /// The leading run of ink before the word's first gap, read alone.
    Token(Label),
    /// The label the whole read begins with, when the run read as none.
    Prefix(Label),
    /// A number standing on its own beside the word.
    Beside(Label),
}

impl Evidence {
    /// The label, wherever it came from.
    pub fn label(self) -> Label {
        match self {
            Evidence::Token(l) | Evidence::Prefix(l) | Evidence::Beside(l) | Evidence::Cell(l) => l,
        }
    }

    /// Whether the label may be cut off the crop by ink.
    pub fn by_ink(self) -> bool {
        matches!(self, Evidence::Token(_))
    }
}

/// One box as the group pass sees it before it decides anything.
pub struct Candidate<'a> {
    /// The box on the page.
    pub quad: &'a Quad,
    /// The recogniser's read of the whole crop.
    pub read: Option<&'a str>,
    /// The read of the leading run of ink alone, where a gap gave one.
    pub token: Option<&'a str>,
    /// Whether the box is a word by the keep rule.
    pub word_like: bool,
}

/// What the group pass made of the boxes' labels.
pub struct Labelled {
    /// Each box's label and where it came from.
    pub evidence: Vec<Option<Evidence>>,
    /// The boxes that are a number standing on its own, not words.
    pub label_box: Vec<bool>,
    /// What each box's label evidence read as, for the record.
    pub label_read: Vec<Option<String>>,
    /// Every observed label with its source, including unresolved/conflicting ones.
    pub observations: Vec<WordEvidence>,
}

/// Cuts the detector's polygons into words and reads each, the page's
/// numbers found over the group first: the stage the harness's
/// `detect` runs, given the polygons (the detector's or any other's)
/// and the calibrated reader.
/// Every build uses the same raw-source accounting, fragment decisions and
/// bounded whole-source recovery/union reads, including classifier-only and
/// custom-margin diagnostics.
/// Normal ONNX scans with a recogniser and [`CROP_MARGIN`] include lean E3
/// expansion. Classifier-only and custom-margin diagnostic reads do not: E3
/// was measured only with the ordinary recogniser and fixed crop margin.
pub fn read_words(
    photo: &RgbImage,
    quads: &[Quad],
    reader: &Reader,
    recogniser: Option<&Recogniser>,
    margin: f32,
) -> Result<PageScan, String> {
    read_words_observed(photo, quads, reader, recogniser, margin, None).map(|(scan, _)| scan)
}

/// [`read_words`] with optional, synchronous geometry/work observations.
/// Returns the unchanged result and its stable-ID mapping; the owning mobile
/// call emits the terminal outcome only after it has also packed that result.
pub fn read_words_observed(
    photo: &RgbImage,
    quads: &[Quad],
    reader: &Reader,
    recogniser: Option<&Recogniser>,
    margin: f32,
    observer: Option<&dyn Observer>,
) -> Result<(PageScan, Vec<FinalRegion>), String> {
    #[cfg(feature = "word-extent")]
    let mut extent = default_extent(recogniser.is_some(), margin);
    read_words_inner(
        photo,
        quads,
        reader,
        recogniser,
        margin,
        observer,
        #[cfg(feature = "word-extent")]
        extent.as_mut(),
    )
}

#[cfg(feature = "word-extent")]
fn default_extent(has_recogniser: bool, margin: f32) -> Option<crate::extent::Report> {
    (has_recogniser && margin == CROP_MARGIN)
        .then(|| crate::extent::Report::new(crate::extent::Arm::E3, crate::extent::Audit::None))
}

/// Explicit pre-E3 baseline for historical comparisons. Normal app and CLI
/// calls use [`read_words_observed`], even in a build with experiment features.
#[cfg(feature = "extent-experiment")]
pub fn read_words_baseline_observed(
    photo: &RgbImage,
    quads: &[Quad],
    reader: &Reader,
    recogniser: Option<&Recogniser>,
    margin: f32,
    observer: Option<&dyn Observer>,
) -> Result<(PageScan, Vec<FinalRegion>), String> {
    read_words_inner(photo, quads, reader, recogniser, margin, observer, None)
}

/// Runs the reviewed E1 arm explicitly; never used by the normal scanner.
#[cfg(feature = "extent-experiment")]
pub fn read_words_with_extent(
    photo: &RgbImage,
    quads: &[Quad],
    reader: &Reader,
    recogniser: &Recogniser,
) -> Result<crate::extent::Experiment, String> {
    read_words_with_extent_arm(
        photo,
        quads,
        reader,
        recogniser,
        crate::extent::Arm::E1,
        crate::extent::Audit::Components,
        None,
    )
    .map(|(experiment, _)| experiment)
}

/// Runs the pre-registered E2 ink-ownership arm; never the normal scanner.
#[cfg(feature = "extent-experiment")]
pub fn read_words_with_extent_e2(
    photo: &RgbImage,
    quads: &[Quad],
    reader: &Reader,
    recogniser: &Recogniser,
) -> Result<crate::extent::Experiment, String> {
    read_words_with_extent_arm(
        photo,
        quads,
        reader,
        recogniser,
        crate::extent::Arm::E2,
        crate::extent::Audit::Components,
        None,
    )
    .map(|(experiment, _)| experiment)
}

/// Runs E3 with explicit component diagnostics; normal scans use lean E3.
#[cfg(feature = "extent-experiment")]
pub fn read_words_with_extent_e3(
    photo: &RgbImage,
    quads: &[Quad],
    reader: &Reader,
    recogniser: &Recogniser,
) -> Result<crate::extent::Experiment, String> {
    read_words_with_extent_e3_audit(
        photo,
        quads,
        reader,
        recogniser,
        crate::extent::Audit::Components,
    )
}

/// Same E3 decisions, with optional component evidence. Audit::None
/// avoids collecting detailed witnesses; neither mode constructs report JSON.
#[cfg(feature = "extent-experiment")]
pub fn read_words_with_extent_e3_audit(
    photo: &RgbImage,
    quads: &[Quad],
    reader: &Reader,
    recogniser: &Recogniser,
    audit: crate::extent::Audit,
) -> Result<crate::extent::Experiment, String> {
    read_words_with_extent_e3_observed(photo, quads, reader, recogniser, audit, None)
        .map(|(experiment, _)| experiment)
}

/// Explicit E3 activation with optional observations and the final stable-ID
/// map. Extent work remains in Checking; the post-split Reading total is fixed.
/// The mobile owner emits completion only after result packing. Normal scans
/// share this owner with Audit::None and discard its compact report on return.
#[cfg(feature = "extent-experiment")]
pub fn read_words_with_extent_e3_observed(
    photo: &RgbImage,
    quads: &[Quad],
    reader: &Reader,
    recogniser: &Recogniser,
    audit: crate::extent::Audit,
    observer: Option<&dyn Observer>,
) -> Result<(crate::extent::Experiment, Vec<FinalRegion>), String> {
    read_words_with_extent_arm(
        photo,
        quads,
        reader,
        recogniser,
        crate::extent::Arm::E3,
        audit,
        observer,
    )
}

#[cfg(feature = "extent-experiment")]
fn read_words_with_extent_arm(
    photo: &RgbImage,
    quads: &[Quad],
    reader: &Reader,
    recogniser: &Recogniser,
    arm: crate::extent::Arm,
    audit: crate::extent::Audit,
    observer: Option<&dyn Observer>,
) -> Result<(crate::extent::Experiment, Vec<FinalRegion>), String> {
    let mut report = crate::extent::Report::new(arm, audit);
    let (scan, regions) = read_words_inner(
        photo,
        quads,
        reader,
        Some(recogniser),
        CROP_MARGIN,
        observer,
        Some(&mut report),
    )?;
    Ok((crate::extent::Experiment { scan, report }, regions))
}

fn read_words_inner(
    photo: &RgbImage,
    quads: &[Quad],
    reader: &Reader,
    recogniser: Option<&Recogniser>,
    margin: f32,
    observer: Option<&dyn Observer>,
    #[cfg(feature = "word-extent")] mut extent: Option<&mut crate::extent::Report>,
) -> Result<(PageScan, Vec<FinalRegion>), String> {
    require_box_only(margin)?;
    let _words = crate::timing::span("phrase.total");
    let page_direction = PageAxis::from_detections(quads).resolve(
        photo,
        reader.photo_up,
        recogniser,
        margin,
        reader.decision_trace,
    )?;
    let direction = page_direction.direction();
    let scoped_reader = Reader {
        writing: WritingFrame::Page(direction),
        page_decisions: page_direction.decisions.clone(),
        ..reader.clone()
    };
    let reader = &scoped_reader;
    let _split = crate::timing::span("phrase.split_order");
    let Segmentation {
        words: split_words,
        mut sources,
    } = Segmentation::in_direction(photo, quads, direction);
    let mut input = crate::repair::Input::with_writing(&mut sources, margin, reader.writing);
    drop(split_words);
    // everything from here is in column order: prepared[k] is the box
    // of column rank k
    let quads_by_column = &input.quads;
    drop(_split);
    let mut progress = Stage::geometry(observer, quads_by_column.len());
    if observer.is_some() {
        for (k, quad) in quads_by_column.iter().enumerate() {
            progress.region(k, quad, RegionState::Found);
        }
    }
    let mut standalone_budget = crate::read_budget::Budget::default();
    #[cfg(feature = "word-extent")]
    let budget = extent
        .as_deref_mut()
        .map(|r| &mut r.budget)
        .unwrap_or(&mut standalone_budget);
    #[cfg(not(feature = "word-extent"))]
    let budget = &mut standalone_budget;
    budget.writing = reader.writing;
    input.reserve(budget, margin, recogniser.is_some())?;
    let mut label_grid = if let Some(recogniser) = recogniser {
        let ordinary = input
            .ordinary()
            .map(|(i, q)| (i, q.clone()))
            .collect::<Vec<_>>();
        Some(std::sync::Arc::new(grid::build(
            photo,
            &ordinary,
            &input.quads,
            reader.writing,
            margin,
            budget,
            reader.decision_trace,
            &progress,
            |crop| recogniser.read(crop),
        )?))
    } else {
        None
    };
    if let Some(grid) = &mut label_grid {
        let grid = std::sync::Arc::get_mut(grid).expect("grid not shared before partition");
        let original_quads = input.quads.clone();
        if let Some(parents) = input.partition_columns(&mut sources, reader.writing, |q| {
            grid.partition(photo, q, reader.writing)
        }) {
            progress.partitioned(&parents, &input.quads);
            DecisionTrace::push(&mut grid.decisions, || {
                serde_json::json!({
                "rule":"grid_column_partition","status":"evaluated",
                "source_index_space":"pre_partition_observations",
                "original_quads":original_quads.iter().map(|q|q.0).collect::<Vec<_>>(),
                "parts":input.quads.iter().enumerate().map(|(i,q)|serde_json::json!({
                    "observation":i,"parent_observation":parents[i],"quad":q.0,
                    "source":input.observations[i].source,"part":input.observations[i].part
                })).collect::<Vec<_>>() })
            });
        }
        let ordinary = input
            .ordinary()
            .map(|(i, q)| (i, q.clone()))
            .collect::<Vec<_>>();
        let mut groups = grid.cell_groups(photo, &ordinary, reader.writing, margin);
        groups.extend(grid.compact_groups(photo, &ordinary, reader.writing));
        let original_quads = input.quads.clone();
        let original_sources = input.observations.clone();
        if let Some(parents) = input.coalesce_cells(&mut sources, reader.writing, &groups) {
            progress.coalesced(&parents, &input.quads);
            DecisionTrace::push(&mut grid.decisions, || {
                serde_json::json!({
                    "rule":"grid_cell_coalescence","status":"evaluated",
                    "source_index_space":"after_column_partition_before_cell_coalescence",
                    "groups":parents.iter().enumerate().filter(|(_,p)|p.len()>1).map(|(i,p)|serde_json::json!({
                        "observation":i,"quad":input.quads[i].0,
                        "parents":p.iter().map(|&j|serde_json::json!({"source":original_sources[j].source,
                            "part":original_sources[j].part,"quad":original_quads[j].0})).collect::<Vec<_>>()
                    })).collect::<Vec<_>>(),
                    "recognition":"one_read_per_owned_extent","height_clipping":false
                })
            });
        }
    }
    // Partitioning changes the observations, so derive both traversals from
    // their new geometry rather than keeping the merged box's old column rank.
    let words = input.quads.clone();
    let layout = input.layout.clone();
    let columns = layout.column_order();
    let rows = layout.row_order();
    let mut column_rank = vec![0; words.len()];
    let mut row_rank = vec![0; words.len()];
    for (r, &i) in columns.iter().enumerate() {
        column_rank[i] = r;
    }
    for (r, &i) in rows.iter().enumerate() {
        row_rank[i] = r;
    }
    let layout = layout.reindexed(&columns);
    let mut scoped_reader = reader.clone();
    if let Some(grid) = &label_grid {
        DecisionTrace::append(&mut scoped_reader.page_decisions, grid.decisions.clone());
    }
    scoped_reader.grid = label_grid;
    let reader = &scoped_reader;
    progress.reading();
    let mut prepared = input.prepare_reserved_with(
        crate::repair::Traversal::for_recogniser(recogniser.map(Recogniser::preparation)),
        budget,
        &progress,
        |quad| {
            reader.grid.as_ref().map_or_else(
                || quad.clone(),
                |grid| grid.constrain(quad, reader.writing, margin).0,
            )
        },
        |quad, labels, budget| {
            prepare_inner(photo, quad, reader, recogniser, margin, labels, budget)
        },
    )?;
    let n = input.observations.len();
    debug_assert_eq!(prepared.len(), n);
    progress.checking();

    // the labels over the group, then the fit
    let word_like: Vec<bool> = prepared
        .iter_mut()
        .map(|p| {
            select(
                p.read.as_ref().map(|r| r.text.as_str()),
                None,
                &p.ranked,
                reader.calibration.min_prob,
                reader.calibration.min_margin,
            )
            .map(|s| {
                let recognised = DecisionTrace::eligibility(&mut p.decisions, &s, "initial");
                DecisionTrace::push(&mut p.decisions, || serde_json::json!({
                    "rule":"cell_eligibility","status":"evaluated","recognition_word_like":recognised,
                    "geometry_supported":p.cell.is_some(),"eligible":recognised||p.cell.is_some()}));
                recognised || p.cell.is_some()
            })
            .map_err(|e| format!("word eligibility: {e}"))
        })
        .collect::<Result<_, _>>()?;
    let candidates: Vec<Candidate> = prepared
        .iter()
        .zip(&word_like)
        .map(|(p, &word_like)| Candidate {
            quad: &p.quad,
            read: p.read.as_ref().map(|r| r.text.as_str()),
            token: p.token.as_ref().map(|t| t.text.as_str()),
            word_like,
        })
        .collect();
    let mut labelled =
        labels::label_evidence_in(&candidates, reader.decision_trace, reader.writing);
    for (k, p) in prepared.iter().enumerate() {
        if let Some(cell) = &p.cell {
            // A word-side cell, not an OCR spelling, establishes this role.
            // Explicit geometric label/gutter exclusions still apply below.
            labelled.label_box[k] = false;
            if let Some(n) = cell.ordinal {
                labelled.evidence[k] = Some(Evidence::Cell(Label::Exact(n)));
            }
        }
        if let Some(label) = reader
            .grid
            .as_ref()
            .and_then(|g| g.label(&p.quad, reader.writing))
        {
            let value = label.ordinal.map_or(Label::Shaped, Label::Exact);
            labelled.evidence[k] = Some(Evidence::Token(value));
            labelled.label_read[k] = Some(label.literal.clone());
            labelled.observations[k].labels.push(label);
        }
    }
    let grid_excluded: Vec<_> = prepared
        .iter()
        .map(|p| {
            reader.grid.as_ref().and_then(|g| {
                g.exclusion_read(
                    &p.quad,
                    reader.writing,
                    p.read.as_ref().map(|r| r.text.as_str()),
                )
            })
        })
        .collect();
    for (k, reason) in grid_excluded.iter().enumerate() {
        if matches!(
            *reason,
            Some("confirmed_standalone_label" | "inside_label_gutter")
        ) {
            labelled.label_box[k] = true;
        }
    }
    for (p, observed) in prepared.iter_mut().zip(&mut labelled.observations) {
        DecisionTrace::append(&mut p.decisions, observed.decisions.take());
    }
    // the phrase's region: the words within reach of one another; a
    // few far from all of them are left out before the numbers are fitted
    let region_words: Vec<bool> = (0..n)
        .map(|k| {
            let eligible = word_like[k] && !labelled.label_box[k] && grid_excluded[k].is_none();
            DecisionTrace::push(&mut prepared[k].decisions, || serde_json::json!({
                "rule":"region_eligibility", "status":"evaluated", "word_like":word_like[k],
                "standalone_label":labelled.label_box[k], "comparison":"word_like and not standalone_label", "eligible":eligible}));
            eligible
        })
        .collect();
    let far = crate::region::apart_in_observed(&layout, &region_words, |k, event| {
        DecisionTrace::push(&mut prepared[k].decisions, event);
    });
    let unplaced: Vec<bool> = (0..n)
        .map(|k| labelled.label_box[k] || !word_like[k] || far[k] || grid_excluded[k].is_some())
        .collect();
    let placed = |k: &usize| !unplaced[*k];
    let column_order: Vec<usize> = (0..n).filter(placed).collect();
    let row_order: Vec<usize> = rows
        .iter()
        .map(|&i| column_rank[i])
        .filter(placed)
        .collect();
    // PENLOCK_TRACE_LABELS=1 prints the fit's inputs, for a page whose
    // numbering came out other than expected
    if std::env::var_os("PENLOCK_TRACE_LABELS").is_some() {
        for k in 0..n {
            eprintln!(
                "trace k={k} word_like={} label_box={} evidence={:?} read={:?} token={:?}",
                word_like[k],
                labelled.label_box[k],
                labelled.evidence[k],
                prepared[k].read.as_ref().map(|r| &r.text),
                prepared[k].token.as_ref().map(|r| &r.text)
            );
        }
        eprintln!("trace columns={column_order:?} rows={row_order:?}");
    }
    let numbering = labelled.numbering(&column_order, &row_order);
    let Labelled {
        evidence,
        label_box,
        mut label_read,
        mut observations,
    } = labelled;

    let mut out = Vec::with_capacity(n);
    for (k, mut p) in prepared.into_iter().enumerate() {
        let number = p
            .cell
            .as_ref()
            .and_then(|c| c.ordinal)
            .or(match &numbering {
                Numbering::Held { numbers, .. } => numbers[k],
                _ => None,
            });
        // a box left out by the group — a number on its own, or far
        // from the phrase — stays out; the box's own trim can only add
        let excluded = label_box[k] || far[k] || grid_excluded[k].is_some();
        DecisionTrace::push(&mut p.decisions, || {
            serde_json::json!({
            "rule":"grid_exclusion", "status":"evaluated", "reason":grid_excluded[k],
            "excluded":grid_excluded[k].is_some(),
            "literal_alpha_tokens":p.read.as_ref().map(|r|r.text.split_whitespace().filter(|s|s.chars().any(char::is_alphabetic)).count()),
            "prose_min_tokens":2,"wide_line_ratio":1.5,"uncertainty_rows":0.5,
            "top_prose_uncertainty_rows":0.0,"below_end_requires_prose_or_wide_line":true})
        });
        let mut owned = std::mem::take(&mut observations[k]);
        let (narrowed_box, trim_stray) = cuts::apply(
            &mut p,
            &mut owned,
            number,
            numbering.has_held_sequence(),
            label_box[k],
            evidence[k],
            recogniser.is_some(),
            margin,
            |p, cut| {
                progress.region(k, &p.quad, RegionState::Reading);
                past(p, cut, reader, recogniser.expect("cut requires recogniser"))
            },
        )?;
        let grid_narrowed = reader
            .grid
            .as_ref()
            .is_some_and(|g| g.constrain(&input.quads[k], reader.writing, margin).2);
        if narrowed_box {
            p.cell = reader
                .grid
                .as_ref()
                .and_then(|g| g.word_cell(photo, &p.quad, reader.writing));
        }
        // A failed recognition-based trim must not erase supported word ink.
        let trim_stray = trim_stray && p.cell.is_none();
        let mut word = finish_evidenced(
            p,
            &reader.calibration,
            narrowed_box || grid_narrowed,
            excluded || trim_stray,
            number,
            owned,
        )?;
        word.label = label_read[k].take();
        word.apart = far[k] || grid_excluded[k].is_some();
        word.column_rank = k;
        word.row_rank = row_rank[columns[k]];
        progress.region(
            k,
            &word.quad,
            if word.stray {
                RegionState::Excluded
            } else {
                RegionState::Read
            },
        );
        out.push(word);
    }
    // Every prepared observation keeps a slot, including strays and labels;
    // filtering here would invalidate ledger and live-progress links.
    debug_assert_eq!(out.len(), words.len());
    let mut scan = PageScan {
        page_direction: Some(page_direction),
        width: photo.width(),
        height: photo.height(),
        words: out,
        numbering,
        join_trials: Vec::new(),
        sources,
    };
    crate::repair::unions(
        &mut scan,
        &label_box,
        margin,
        recogniser.is_some(),
        budget,
        &mut progress,
        |quad, budget| {
            let p = prepare_inner(photo, quad, reader, recogniser, margin, false, Some(budget))?;
            finish(p, &reader.calibration, false, false, None)
        },
    )?;
    crate::joins::apply(
        &layout,
        &label_box,
        &mut scan,
        margin,
        &|quad| read_crop(photo, quad, reader, recogniser, margin),
        &mut progress,
    )?;
    if scan
        .sources
        .iter()
        .any(|s| s.union.as_ref().is_some_and(|u| u.word_index.is_some()))
    {
        scan.reindex_words(&mut progress);
    }
    #[cfg(feature = "word-extent")]
    if let Some(report) = extent {
        // The ledger owns raw/part identity and the current original-word link.
        // Build extent's diagnostic snapshot from it, not a second reconstruction
        // based on which final words happen to be native joins.
        report.sources = input
            .observations
            .iter()
            .enumerate()
            .map(|(k, &o)| crate::extent::Source {
                detector: scan.sources[o.source].detector,
                part: o.part,
                quad: o.quad(&scan.sources).clone(),
                column_rank: k,
                row_rank: row_rank[columns[k]],
                word_index: o
                    .word_index(&scan.sources)
                    .expect("every selected observation was read"),
                labelled: label_box[k] || evidence[k].is_some(),
            })
            .collect();
        crate::extent::apply(
            photo,
            &layout,
            &mut scan,
            report,
            reader,
            recogniser.ok_or("word expansion requires a recogniser")?,
            &mut progress,
        )?;
    }
    // Geometry repair and ownership have finished. Tightening must not change
    // which merge/recovery proposals ran. Re-read only the changed final boxes;
    // retain their identity, ordinality and keep state even if recognition worsens.
    for (index, word) in scan.words.iter_mut().enumerate().filter(|(_, w)| !w.stray) {
        let (tight, decision) =
            grid::ink_width::shrink(photo, &word.quad, reader.writing, reader.decision_trace);
        if tight != word.quad {
            let p = prepare_frame_in(
                photo,
                &tight,
                |crop| reader.model.read(crop),
                recogniser.map(|r| move |crop: &GrayImage| r.read(crop)),
                0.,
                false,
                reader.decision_trace,
                None,
                reader.writing,
                None,
                None,
            )?;
            let selection = select(
                p.read.as_ref().map(|r| r.text.as_str()),
                None,
                &p.ranked,
                reader.calibration.min_prob,
                reader.calibration.min_margin,
            )
            .map_err(|e| format!("final box selection: {e}"))?;
            word.quad = p.quad;
            word.crop = p.crop;
            word.ranked = p.ranked;
            word.raw = p.read;
            word.turned = p.turned;
            word.selection = Some(selection);
            word.evidence.matching = None;
            DecisionTrace::append(&mut word.evidence.decisions, p.decisions);
            progress.region(index, &word.quad, RegionState::Read);
        }
        if let Some(event) = decision {
            DecisionTrace::push(&mut word.evidence.decisions, || event);
        }
    }
    let regions = progress.into_regions();
    history::terminal(&mut scan, &regions, "scan_return");
    Ok((scan, regions))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::region::apart;

    #[cfg(feature = "word-extent")]
    #[test]
    fn normal_extent_is_exactly_lean_e3_only_for_the_measured_read_configuration() {
        let report = default_extent(true, CROP_MARGIN).unwrap();
        assert_eq!(
            report,
            crate::extent::Report::new(crate::extent::Arm::E3, crate::extent::Audit::None)
        );
        assert_eq!(report.retained_component_witnesses(), 0);
        assert!(default_extent(false, CROP_MARGIN).is_none());
        assert!(default_extent(true, 0.15).is_none());
        assert!(default_extent(true, 0.2).is_none());
    }

    #[test]
    fn a_leading_number_is_a_leading_digit() {
        assert!(leading_number("1. walk"));
        assert!(leading_number("12"));
        assert!(!leading_number("walk 1"));
        assert!(!leading_number(""));
    }

    #[test]
    fn a_number_beside_a_word_labels_it_and_leaves_its_own_leading_run_alone() {
        let quad = |x0: f32, x1: f32| Quad([(x0, 0.0), (x1, 0.0), (x1, 50.0), (x0, 50.0)]);
        let (number, silk, dice, stray) = (
            quad(0.0, 30.0),
            quad(40.0, 240.0),
            quad(300.0, 500.0),
            quad(600.0, 700.0),
        );
        let boxes = [
            Candidate {
                quad: &number,
                read: Some("4"),
                token: Some("4"),
                word_like: false,
            },
            Candidate {
                quad: &silk,
                read: Some("SILK"),
                token: Some("S"),
                word_like: true,
            },
            Candidate {
                quad: &dice,
                read: Some("l.dice"),
                token: Some(""),
                word_like: true,
            },
            Candidate {
                quad: &stray,
                read: Some("120"),
                token: Some("1"),
                word_like: false,
            },
        ];
        let labelled = label_evidence(&boxes);
        assert_eq!(labelled.label_box, vec![true, false, false, false]);
        assert_eq!(
            labelled.evidence[1],
            Some(Evidence::Beside(Label::Exact(4)))
        );
        assert!(
            !labelled.evidence[1].unwrap().by_ink(),
            "the S is the word's, not a label to cut"
        );
        assert_eq!(labelled.evidence[2], Some(Evidence::Prefix(Label::Shaped)));
        assert!(!labelled.evidence[2].unwrap().by_ink());
        assert_eq!(labelled.evidence[3], None);
        assert_eq!(labelled.label_read[1].as_deref(), Some("4"));
        // the same word with no number beside it: its own run is a
        // label by shape, and may be cut
        let alone = [Candidate {
            quad: &silk,
            read: Some("SILK"),
            token: Some("S"),
            word_like: true,
        }];
        let labelled = label_evidence(&alone);
        assert_eq!(labelled.evidence[0], Some(Evidence::Token(Label::Shaped)));
        assert!(labelled.evidence[0].unwrap().by_ink());
    }

    #[test]
    fn a_box_the_group_left_out_stays_out_however_well_it_reads() {
        let calibration = Calibration {
            min_prob: 0.5,
            min_margin: 0.1,
            digest: String::new(),
            rule: CANON_RULE.to_owned(),
        };
        let walk = usize::from(Word::from_bip39("walk").unwrap().index());
        let prepared = || Prepared {
            cell: None,
            writing: WritingFrame::Local,
            decisions: None,
            quad: Quad([(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]),
            crop: GrayImage::new(1, 1),
            ranked: vec![(walk, 0.99)],
            turned: false,
            read: Some(LineRead {
                text: "walk".into(),
                confidence: 0.9,
            }),
            cut: None,
            token: None,
        };
        let kept = finish(prepared(), &calibration, false, false, None).unwrap();
        assert!(!kept.stray);
        assert_eq!(kept.pick(), Some(walk));
        let out = finish(prepared(), &calibration, false, true, None).unwrap();
        assert!(out.stray, "a list word read with confidence, and still out");
        assert_eq!(out.pick(), Some(walk));
    }

    #[test]
    fn a_box_far_from_the_phrase_takes_no_part_in_its_numbering() {
        // twelve numbered words in two columns, and a box far below
        // them labelled 5: the region leaves it out, and the fit,
        // given the orders without it, neither duplicates 5 nor gives
        // it a place
        let word = |x: f32, y: f32| {
            Quad([
                (x, y),
                (x + 400.0, y),
                (x + 400.0, y + 200.0),
                (x, y + 200.0),
            ])
        };
        let mut quads: Vec<Quad> = (0..12)
            .map(|i| {
                word(
                    if i < 6 { 900.0 } else { 1800.0 },
                    1000.0 + 250.0 * (i % 6) as f32,
                )
            })
            .collect();
        quads.push(word(900.0, 4000.0));
        let far = apart(&quads, &[true; 13]);
        assert_eq!(far.iter().filter(|&&f| f).count(), 1);
        assert!(far[12]);
        let mut labels: Vec<Option<Label>> = (1..=12).map(|n| Some(Label::Exact(n))).collect();
        labels.push(Some(Label::Exact(5)));
        let order: Vec<usize> = (0..13).filter(|&k| !far[k]).collect();
        match fit(&labels, &order, &order) {
            Numbering::Held {
                numbers,
                count,
                missing,
                ..
            } => {
                assert_eq!(numbers[12], None);
                assert_eq!((count, missing), (12, vec![]));
                assert_eq!(numbers[4], Some(5));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_page_s_orders_follow_its_numbers_when_it_has_them() {
        let word = |number: Option<u32>, stray: bool| WordBox {
            quad: Quad([(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]),
            crop: GrayImage::new(1, 1),
            ranked: Vec::new(),
            selection: None,
            column_rank: 0,
            row_rank: 0,
            turned: false,
            raw: None,
            evidence: Default::default(),
            narrowed: false,
            stray,
            number,
            label: None,
            apart: false,
            joined_from: None,
            expanded_from: Vec::new(),
        };
        let scan = PageScan {
            page_direction: None,
            width: 1,
            height: 1,
            sources: Vec::new(),
            join_trials: Vec::new(),
            words: vec![
                word(Some(2), false),
                word(None, false),
                word(Some(1), true),
                word(Some(3), false),
            ],
            numbering: Numbering::Held {
                numbers: vec![Some(2), None, Some(1), Some(3)],
                count: 3,
                missing: Vec::new(),
                out_of_place: Vec::new(),
                unresolved: Vec::new(),
            },
        };
        assert_eq!(scan.by_numbers(), vec![2, 0, 3, 1]);
        assert_eq!(scan.kept_numbers(), vec![0, 3, 1]);
        assert!(scan.numbered());
        let initial = scan.initial_traversal();
        assert_eq!(initial.order, InitialOrder::Numbers);
        assert_eq!(initial.indices, vec![2, 0, 3, 1]); // includes the stray
        assert!(!initial.numbered_pass); // held numbers still take precedence
    }

    #[test]
    fn initial_traversal_checks_kept_picks_and_preserves_every_source() {
        let mut words: Vec<_> = (0..12)
            .map(|i| WordBox {
                quad: Quad([(0., 0.), (1., 0.), (1., 1.), (0., 1.)]),
                crop: GrayImage::new(1, 1),
                ranked: vec![(if i == 0 { 3 } else { 0 }, 1.0)],
                selection: Some(
                    select(None, None, &[(if i == 0 { 3 } else { 0 }, 1.0)], 0.85, 0.75).unwrap(),
                ),
                column_rank: i,
                row_rank: if i == 0 { 11 } else { i - 1 },
                turned: false,
                raw: None,
                evidence: Default::default(),
                narrowed: false,
                stray: false,
                number: None,
                label: None,
                apart: false,
                joined_from: None,
                expanded_from: Vec::new(),
            })
            .collect();
        let mut stray = words[1].clone();
        stray.stray = true;
        stray.column_rank = 12;
        stray.row_rank = 12;
        words.push(stray);
        let mut scan = PageScan {
            page_direction: None,
            width: 1,
            height: 1,
            sources: Vec::new(),
            words,
            numbering: Numbering::None,
            join_trials: Vec::new(),
        };
        let initial = scan.initial_traversal();
        assert!(!initial.columns_pass && initial.rows_pass);
        assert_eq!(initial.order, InitialOrder::Columns);
        assert_eq!(initial.indices, (0..13).collect::<Vec<_>>());
        assert!(initial.requires_review);
        // The very same picks/checksums must not override supported columns.
        for (i, word) in scan.words.iter_mut().take(12).enumerate() {
            let x = (i / 6) as f32 * 100.;
            let y = (i % 6) as f32 * 25.;
            word.quad = Quad([(x, y), (x + 60., y), (x + 60., y + 20.), (x, y + 20.)]);
        }
        let supported = scan.initial_traversal();
        assert!(supported.rows_pass && !supported.columns_pass);
        assert_eq!(supported.order, InitialOrder::Columns);
        assert!(!supported.requires_review);
        assert_eq!(supported.basis, "longest_dimension_first");
        for word in scan.words.iter_mut().take(12) {
            word.quad = Quad([(0., 0.), (1., 0.), (1., 1.), (0., 1.)]);
        }
        let corners = scan.words[0].quad.0;
        scan.words[0].evidence.labels.push(LabelRead {
            corners,
            literal: "G.".into(),
            origin: "prefix".into(),
            ordinal: None,
            dotted: true,
            ambiguous: false,
        });
        assert_eq!(
            scan.list_mode(),
            crate::numbering::dotted::ListMode::Numbered
        );
        assert!(!scan.numbered());
        assert_eq!(scan.initial_traversal(), initial);
        scan.words[0].ranked = vec![(0, 1.0)];
        scan.words[0].selection =
            Some(select(None, None, &scan.words[0].ranked, 0.85, 0.75).unwrap()); // neither traversal passes
        assert_eq!(scan.initial_traversal().order, InitialOrder::Columns);
        scan.words[11].ranked = vec![(3, 1.0)];
        scan.words[11].selection =
            Some(select(None, None, &scan.words[11].ranked, 0.85, 0.75).unwrap());
        for (i, word) in scan.words.iter_mut().enumerate() {
            word.row_rank = i;
        }
        let initial = scan.initial_traversal();
        assert!(initial.columns_pass && initial.rows_pass);
        assert_eq!(initial.order, InitialOrder::Columns);

        // Every checksum combination and both confidence states must leave
        // each geometric default unchanged. No models or reference words.
        for accepted in [false, true] {
            for cp in [false, true] {
                for rp in [false, true] {
                    for (cols, expected) in [(2, InitialOrder::Columns), (6, InitialOrder::Rows)] {
                        let rows = 12 / cols;
                        for (i, word) in scan.words.iter_mut().take(12).enumerate() {
                            let picked = if (cp && i == 11) || (!cp && rp && i == 0) {
                                3
                            } else {
                                0
                            };
                            word.ranked = vec![(picked, 0.9), (1, 0.1)];
                            word.select_fixture(picked, accepted);
                            word.row_rank = if cp && rp {
                                if i == 11 { 11 } else { 10 - i }
                            } else if i == 0 {
                                11
                            } else {
                                i - 1
                            };
                            let x = (i / rows) as f32 * 100.;
                            let y = (i % rows) as f32 * 25.;
                            word.quad =
                                Quad([(x, y), (x + 60., y), (x + 60., y + 20.), (x, y + 20.)]);
                        }
                        let result = scan.initial_traversal();
                        assert_eq!((result.columns_pass, result.rows_pass), (cp, rp));
                        assert_eq!(result.order, expected);
                        assert!(!result.requires_review);
                    }
                }
            }
        }
    }

    #[test]
    fn a_model_the_calibration_was_not_selected_on_is_refused_unloaded() {
        let text =
            format!("min_prob=0.5\nmin_margin=0.1\nmodel_sha256=abc\ncanon_rule={CANON_RULE}\n");
        let err = Reader::from_bytes(b"not the model", &text).err().unwrap();
        assert!(
            err.contains("does not match the calibration artifact's abc"),
            "{err}"
        );
    }

    #[test]
    fn the_calibration_artifact_is_parsed_and_checked() {
        let text =
            format!("min_prob=0.5\nmin_margin=0.1\nmodel_sha256=abc\ncanon_rule={CANON_RULE}\n");
        let c = Calibration::parse(&text).unwrap();
        assert_eq!(
            (c.min_prob, c.min_margin, c.digest.as_str()),
            (0.5, 0.1, "abc")
        );
        assert!(Calibration::parse("min_prob=0.5\n").is_err());
        assert!(Calibration::parse(&text.replace(CANON_RULE, "another-rule")).is_err());
        assert!(Calibration::parse(&text.replace("0.5", "1.5")).is_err());
    }
}
