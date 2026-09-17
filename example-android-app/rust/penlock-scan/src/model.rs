//! Recognisers: anything that turns a cell image into a probability
//! distribution over the 29 symbols.
//!
//! A model never papers over a failure with a uniform distribution: a
//! process that fails or output that cannot be parsed is a [`ModelError`]
//! naming the cell. A cell the model genuinely cannot read is different:
//! it is reported as uniform — no evidence — and listed in
//! [`CellReadings::unread`] so a caller can say so.

use std::fmt;

use image::GrayImage;
use penlock::word::WORD_LEN;
use penlock::Distribution;
use crate::cells::{CELL_SIZE, CellImage};

/// A recogniser for normalised cell images.
pub trait Model {
    /// Reads every cell of every row. `rectified` is the whole card face-on
    /// at [`crate::locate::RECTIFIED_PX_PER_MM`] for models that want context around a
    /// cell. Returns one distribution per cell, in the same layout, and
    /// says which cells the model could not read at all.
    fn read_cells(
        &self,
        cells: &[[CellImage; WORD_LEN]],
        rectified: &GrayImage,
    ) -> Result<CellReadings, ModelError>;

    /// The spec this model was built from, for reports.
    fn spec(&self) -> String;
}

/// What a model made of a card's cells. [`crate::scan()`] checks the
/// contract below before trusting it; see [`CellReadings::validate`].
#[derive(Clone, PartialEq, Debug)]
pub struct CellReadings {
    /// One distribution per cell, one row per written row of the card.
    pub cells: Vec<[Distribution; WORD_LEN]>,
    /// Cells the model produced no reading for, as `(word_index,
    /// position)`, each at most once and within `cells`. Their
    /// distribution is uniform — no evidence — which the decoder treats
    /// as such; this list is how a caller reports it.
    pub unread: Vec<(usize, usize)>,
}

impl CellReadings {
    /// Checks the contract against the `rows` the card has: exactly `rows`
    /// rows of readings, every `unread` coordinate in range and listed
    /// once, and every listed cell uniform.
    pub fn validate(&self, rows: usize) -> Result<(), ModelError> {
        let shape = |detail: String| ModelError::Shape { detail };
        if self.cells.len() != rows {
            return Err(shape(format!(
                "{} rows of readings for a card with {rows} written rows",
                self.cells.len()
            )));
        }
        let mut seen = std::collections::HashSet::new();
        for &(word, position) in &self.unread {
            if word >= rows || position >= WORD_LEN {
                return Err(shape(format!(
                    "unread cell (word {}, symbol {}) is off the card",
                    word + 1,
                    position + 1
                )));
            }
            if !seen.insert((word, position)) {
                return Err(shape(format!(
                    "unread cell (word {}, symbol {}) listed twice",
                    word + 1,
                    position + 1
                )));
            }
            if self.cells[word][position] != Distribution::uniform() {
                return Err(shape(format!(
                    "unread cell (word {}, symbol {}) carries a reading",
                    word + 1,
                    position + 1
                )));
            }
        }
        Ok(())
    }
}

/// Which cell of the card a model error is about.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CellRef {
    /// A symbol cell, by zero-based word index and position.
    Symbol {
        /// Zero-based word index.
        word: usize,
        /// Zero-based position within the word.
        position: usize,
    },
    /// A whole row, for models that read one row at a time.
    Row {
        /// Zero-based word index.
        word: usize,
    },
}

impl fmt::Display for CellRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Symbol { word, position } => {
                write!(f, "word {} symbol {}", word + 1, position + 1)
            }
            Self::Row { word } => write!(f, "word {}", word + 1),
        }
    }
}

/// Why a model could not read a card.
#[derive(Clone, PartialEq, Debug)]
pub enum ModelError {
    /// Running the model failed.
    Process {
        /// The cell being read.
        cell: CellRef,
        /// What went wrong.
        message: String,
    },
    /// The model's output could not be interpreted.
    Parse {
        /// The cell being read.
        cell: CellRef,
        /// What was unexpected.
        detail: String,
    },
    /// The model could not be loaded from the bytes or path it was given.
    Unusable(String),
    /// The model returned readings that do not match the card: the wrong
    /// number of rows, an `unread` entry out of range or repeated, or an
    /// `unread` cell that is not the uniform distribution it must be.
    Shape {
        /// What was wrong.
        detail: String,
    },
}

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unusable(message) => write!(f, "{message}"),
            Self::Process { cell, message } => write!(f, "{cell}: recogniser failed: {message}"),
            Self::Parse { cell, detail } => {
                write!(
                    f,
                    "{cell}: could not read the recogniser's output: {detail}"
                )
            }
            Self::Shape { detail } => {
                write!(f, "recogniser output does not fit the card: {detail}")
            }
        }
    }
}

impl std::error::Error for ModelError {}

// Plain arithmetic, not a backend's: the ONNX classifier uses it here and the
// research tree's test reader uses it there, so it is gated on neither.
#[cfg_attr(not(feature = "onnx"), allow(dead_code))]
pub(crate) fn softmax<const N: usize>(scores: [f32; N], temperature: f32) -> [f32; N] {
    let max = scores.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let weights = scores.map(|s| ((s - max) / temperature).exp());
    let sum: f32 = weights.iter().sum();
    weights.map(|w| w / sum)
}


/// Cells with nothing written in them, as `(word_index, position)`.
pub(crate) fn empty_cells(cells: &[[CellImage; WORD_LEN]]) -> Vec<(usize, usize)> {
    cells
        .iter()
        .enumerate()
        .flat_map(|(w, row)| {
            row.iter()
                .enumerate()
                .filter(|(_, c)| c.is_empty())
                .map(move |(p, _)| (w, p))
        })
        .collect()
}

pub(crate) fn ink_vector(cell: &CellImage) -> Vec<f32> {
    debug_assert_eq!(cell.pixels.dimensions(), (CELL_SIZE, CELL_SIZE));
    cell.pixels
        .pixels()
        .map(|p| 1.0 - f32::from(p.0[0]) / 255.0)
        .collect()
}

/// Normalised cross-correlation of two equally sized vectors; 0 when
/// either is constant.
pub(crate) fn correlation(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len() as f32;
    let (ma, mb) = (a.iter().sum::<f32>() / n, b.iter().sum::<f32>() / n);
    let (mut num, mut da, mut db) = (0.0f32, 0.0f32, 0.0f32);
    for (x, y) in a.iter().zip(b) {
        let (x, y) = (x - ma, y - mb);
        num += x * y;
        da += x * x;
        db += y * y;
    }
    if da <= 0.0 || db <= 0.0 {
        0.0
    } else {
        num / (da * db).sqrt()
    }
}

/// How many logits the per-cell ABI promises.
#[cfg(feature = "onnx")]
pub const ONNX_SYMBOL_OUTPUTS: usize = 29;

/// The per-cell classifier ABI, run in process with `tract`: one input
/// `1×1×128×128` float — the cell exactly as [`CellImage::model_input`]
/// gives it, ink white on black in `0..=1` — and one output tensor of
/// [`ONNX_SYMBOL_OUTPUTS`] logits in symbol order (`=`, `#`, `A`…`Z`,
/// `-`), softmaxed here at temperature one: calibration is the
/// trainer's job, baked in before export. Any other shape is refused
/// at load.
#[cfg(feature = "onnx")]
pub struct Onnx {
    plan: std::sync::Arc<tract_onnx::prelude::TypedRunnableModel>,
    spec: String,
}

#[cfg(feature = "onnx")]
impl Onnx {
    /// Loads the model at `path` and checks it against the contract.
    pub fn load(path: &std::path::Path) -> Result<Onnx, String> {
        use tract_onnx::prelude::*;
        let spec = format!("onnx:{}", path.display());
        let model = tract_onnx::onnx()
            .model_for_path(path)
            .map_err(|e| format!("{spec}: {e}"))?;
        Onnx::checked(model, spec)
    }

    /// Loads a model already in memory and checks it against the
    /// contract: on a phone the model ships inside the app, not at a
    /// filesystem path.
    pub fn from_bytes(bytes: &[u8]) -> Result<Onnx, String> {
        use tract_onnx::prelude::*;
        let model = tract_onnx::onnx()
            .model_for_read(&mut std::io::Cursor::new(bytes))
            .map_err(|e| format!("onnx:<memory>: {e}"))?;
        Onnx::checked(model, "onnx:<memory>".into())
    }

    fn checked(model: tract_onnx::prelude::InferenceModel, spec: String) -> Result<Onnx, String> {
        use tract_onnx::prelude::*;
        let side = crate::cells::MODEL_SIDE as usize;
        let plan = model
            .with_input_fact(0, f32::fact([1, 1, side, side]).into())
            .map_err(|e| format!("{spec}: input is not 1×1×{side}×{side}: {e}"))?
            .into_optimized()
            .map_err(|e| format!("{spec}: {e}"))?
            .into_runnable()
            .map_err(|e| format!("{spec}: {e}"))?;
        let probe = Tensor::from_shape(&[1, 1, side, side], &vec![0.0f32; side * side])
            .map_err(|e| e.to_string())?;
        let outputs = run_plan(&plan, probe)?.len();
        if outputs != ONNX_SYMBOL_OUTPUTS {
            return Err(format!(
                "{spec}: {outputs} outputs; the contract is {ONNX_SYMBOL_OUTPUTS} symbol logits"
            ));
        }
        Ok(Onnx { plan, spec })
    }

    fn logits(&self, cell: &CellImage, at: CellRef) -> Result<Vec<f32>, ModelError> {
        use tract_onnx::prelude::*;
        let side = crate::cells::MODEL_SIDE as usize;
        let input = Tensor::from_shape(&[1, 1, side, side], &cell.model_input()).map_err(|e| {
            ModelError::Process {
                cell: at,
                message: e.to_string(),
            }
        })?;
        let logits = run_plan(&self.plan, input)
            .map_err(|message| ModelError::Process { cell: at, message })?;
        if logits.len() != ONNX_SYMBOL_OUTPUTS || logits.iter().any(|v| !v.is_finite()) {
            return Err(ModelError::Shape {
                detail: format!(
                    "{at}: the model returned {} outputs, not all finite, where {ONNX_SYMBOL_OUTPUTS} were promised",
                    logits.len()
                ),
            });
        }
        Ok(logits)
    }
}

#[cfg(feature = "onnx")]
pub(crate) use bip39_scan::runtime::run_plan;

#[cfg(feature = "onnx")]
impl Model for Onnx {
    fn read_cells(
        &self,
        cells: &[[CellImage; WORD_LEN]],
        _: &GrayImage,
    ) -> Result<CellReadings, ModelError> {
        let mut out = Vec::with_capacity(cells.len());
        for (word, row) in cells.iter().enumerate() {
            let mut distributions = [Distribution::uniform(); WORD_LEN];
            for (position, cell) in row.iter().enumerate() {
                if cell.is_empty() {
                    continue;
                }
                let logits = self.logits(cell, CellRef::Symbol { word, position })?;
                let mut symbols = [0.0f32; 29];
                symbols.copy_from_slice(&logits[..29]);
                distributions[position] =
                    Distribution::new(softmax(symbols, 1.0)).map_err(|e| ModelError::Shape {
                        detail: format!("word {} symbol {}: {e}", word + 1, position + 1),
                    })?;
            }
            out.push(distributions);
        }
        Ok(CellReadings {
            cells: out,
            unread: empty_cells(cells),
        })
    }

    fn spec(&self) -> String {
        self.spec.clone()
    }
}
