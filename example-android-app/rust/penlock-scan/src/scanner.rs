//! The pipeline's entry point, shaped like the word-list scanner's.
//!
//! The two pipelines in this repository answer the same kind of question — a
//! photograph in, structured data with per-item evidence out — so they are
//! loaded, called and read the same way. What differs between a word and a
//! cell belongs in the payload: a word has 2048 candidates and a reading
//! order, a cell has 29 and a share number.

use image::RgbImage;
use penlock::word::WORD_LEN;
use penlock::{Distribution, ShareIndex, Symbol};
use serde::Serialize;

use crate::locate::{Identity, LocateError};
use crate::model::{Model, ModelError};
use crate::scan::ScannedShare;

/// The model files a scanner needs, as bytes the caller owns.
pub struct ModelBytes<'a> {
    /// The 29-way cell classifier.
    pub cells: &'a [u8],
}

/// Per-call settings. Options never alter cell normalisation or the alphabet.
#[derive(Clone, Copy, Debug, Default)]
pub struct ScanOptions {
    /// Retain per-cell distributions in the projection, not only the reading.
    pub diagnostics: bool,
    /// Explicit EXIF transform (1..=8), overriding embedded metadata. Applies
    /// only to encoded input, never to `scan_rgb`.
    pub orientation: Option<u8>,
}

/// A model-loading, image-decoding or strip-locating error.
#[derive(Debug)]
pub enum ScanError {
    /// The classifier could not be loaded or failed on a cell.
    Model(ModelError),
    /// The bytes were not a decodable image, or exceeded a limit.
    Image(String),
    /// No strip could be found in the photograph.
    Locate(LocateError),
}

impl std::fmt::Display for ScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Model(e) => write!(f, "model: {e}"),
            Self::Image(e) => write!(f, "image: {e}"),
            Self::Locate(e) => write!(f, "scan: {e}"),
        }
    }
}

impl std::error::Error for ScanError {}

impl From<crate::scan::ReadError> for ScanError {
    fn from(error: crate::scan::ReadError) -> Self {
        match error {
            crate::scan::ReadError::Locate(e) => Self::Locate(e),
            crate::scan::ReadError::Model(e) => Self::Model(e),
        }
    }
}

/// Reusable offline strip reader. A scanner is shared across threads, so a
/// model it holds must be too.
pub struct Scanner {
    model: Box<dyn Model + Send + Sync>,
}

impl Scanner {
    /// Loads the cell classifier from bytes the caller supplies. On a phone
    /// the model ships inside the app rather than at a filesystem path.
    #[cfg(feature = "onnx")]
    pub fn new(models: ModelBytes<'_>) -> Result<Self, ScanError> {
        let model = crate::model::Onnx::from_bytes(models.cells)
            .map_err(|message| ScanError::Model(ModelError::Unusable(message)))?;
        Ok(Self {
            model: Box::new(model),
        })
    }

    /// A scanner over an already-built model, for callers with their own.
    pub fn from_model(model: Box<dyn Model + Send + Sync>) -> Self {
        Self { model }
    }

    /// Decodes JPEG, PNG or WebP and applies EXIF orientation exactly once,
    /// then reads the one strip in the photograph. Limits and decoding are the
    /// word-list pipeline's, so both read the same files the same way.
    pub fn scan(&self, encoded: &[u8], options: ScanOptions) -> Result<ScanResult, ScanError> {
        self.scan_rgb(&decode(encoded, options)?, options)
    }

    /// Reads the one strip in a photograph already in canonical orientation.
    pub fn scan_rgb(&self, image: &RgbImage, options: ScanOptions) -> Result<ScanResult, ScanError> {
        admit(image)?;
        let share = crate::scan::scan(image, self.model.as_ref())?;
        Ok(ScanResult {
            width: image.width(),
            height: image.height(),
            shares: vec![share],
            diagnostics: options.diagnostics,
        })
    }

    /// Every strip in a photograph of an uncut sheet, in reading order.
    pub fn scan_all(&self, encoded: &[u8], options: ScanOptions) -> Result<ScanResult, ScanError> {
        self.scan_all_rgb(&decode(encoded, options)?, options)
    }

    /// Every strip in a canonical photograph of an uncut sheet.
    pub fn scan_all_rgb(
        &self,
        image: &RgbImage,
        options: ScanOptions,
    ) -> Result<ScanResult, ScanError> {
        admit(image)?;
        Ok(ScanResult {
            width: image.width(),
            height: image.height(),
            shares: crate::scan::scan_all(image, self.model.as_ref())?,
            diagnostics: options.diagnostics,
        })
    }
}

/// Pixels a caller decoded themselves pass the same admission the decoder
/// applies, so supplying RGB cannot reach past the shared limits.
fn admit(image: &RgbImage) -> Result<(), ScanError> {
    bip39_scan::check_dimensions(image.width(), image.height()).map_err(|e| match e {
        bip39_scan::ScanError::Image(message) => ScanError::Image(message),
        other => ScanError::Image(other.to_string()),
    })
}

fn decode(encoded: &[u8], options: ScanOptions) -> Result<RgbImage, ScanError> {
    bip39_scan::decode_image(encoded, options.orientation).map_err(|e| match e {
        bip39_scan::ScanError::Image(message) => ScanError::Image(message),
        other => ScanError::Image(other.to_string()),
    })
}

/// Native readings plus the photograph they came from.
pub struct ScanResult {
    /// Canonical image dimensions.
    pub width: u32,
    /// Canonical image dimensions.
    pub height: u32,
    /// Every strip read, in the order they were found.
    pub shares: Vec<ScannedShare>,
    diagnostics: bool,
}

impl ScanResult {
    /// Projects saved decisions only: does not rerun, rectify or reclassify.
    pub fn document(&self) -> Document {
        Document {
            width: self.width,
            height: self.height,
            observations: self
                .shares
                .iter()
                .map(|share| observation(share, self.diagnostics))
                .collect(),
        }
    }
}

/// Everything read from one photograph, as plain data.
#[derive(Clone, Debug, Serialize)]
pub struct Document {
    /// Canonical image dimensions.
    pub width: u32,
    /// Canonical image dimensions.
    pub height: u32,
    /// The strips found, in the order they were found.
    pub observations: Vec<Observation>,
}

/// One strip observed in the photograph: which it is, where it was, and
/// what every cell said.
#[derive(Clone, Debug, Serialize)]
pub struct Observation {
    /// `share`, `seed`, or `unknown` when the printed code could not be read.
    pub kind: &'static str,
    /// The share number, when the paper carried a readable one. A strip from
    /// a worksheet that prints no identity code has none, and the caller
    /// supplies it from what is written at the top.
    pub share: Option<u8>,
    /// Fiducial centres in photo pixels: top-left, top-right, bottom-left,
    /// bottom-right.
    pub quad: [(f64, f64); 4],
    /// Mean over cells of the most probable symbol's probability. Not a
    /// statement that the reading is correct.
    pub confidence: f64,
    /// The reading as written, with `?` where a cell could not be read.
    pub text: Vec<String>,
    /// Twelve words of six cells, in worksheet order.
    pub words: Vec<Vec<Cell>>,
}

/// One cell's reading.
#[derive(Clone, Debug, Serialize)]
pub struct Cell {
    /// Position within the word: 0 and 1 are the word's checksum.
    pub position: usize,
    /// The most probable symbol, or `None` when the cell could not be read.
    pub selected: Option<char>,
    /// That symbol's probability.
    pub probability: f32,
    /// True when the classifier returned nothing usable and the distribution
    /// is uniform; distinguishes "could not read" from "read a `Q`".
    pub unread: bool,
    /// Every symbol with its probability, under `diagnostics`.
    pub candidates: Vec<(char, f32)>,
}

fn observation(share: &ScannedShare, diagnostics: bool) -> Observation {
    let unread = |word: usize, position: usize| share.unread.contains(&(word, position));
    Observation {
        kind: match share.identity {
            Identity::Seed => "seed",
            Identity::Share(_) => "share",
            Identity::Unknown => "unknown",
        },
        share: share.identity.share().map(ShareIndex::number),
        quad: share.located.fiducials,
        confidence: share.confidence(),
        text: share
            .cells
            .iter()
            .enumerate()
            .map(|(word, row)| {
                row.iter()
                    .enumerate()
                    .map(|(position, cell)| {
                        if unread(word, position) {
                            '?'
                        } else {
                            cell.argmax().to_char()
                        }
                    })
                    .collect()
            })
            .collect(),
        words: share
            .cells
            .iter()
            .enumerate()
            .map(|(word, row)| {
                (0..WORD_LEN)
                    .map(|position| cell(&row[position], position, unread(word, position), diagnostics))
                    .collect()
            })
            .collect(),
    }
}

fn cell(distribution: &Distribution, position: usize, unread: bool, diagnostics: bool) -> Cell {
    let best = distribution.argmax();
    Cell {
        position,
        selected: (!unread).then(|| best.to_char()),
        probability: distribution.p(best),
        unread,
        candidates: diagnostics
            .then(|| {
                Symbol::all()
                    .map(|symbol| (symbol.to_char(), distribution.p(symbol)))
                    .collect()
            })
            .unwrap_or_default(),
    }
}
