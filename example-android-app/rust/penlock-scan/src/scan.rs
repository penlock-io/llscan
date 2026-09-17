//! A photo to a share: locate, rectify, cut, recognise.

use std::fmt;

use image::RgbImage;
use penlock::word::WORD_LEN;
use penlock::{Distribution, Share, Symbol};

use crate::locate::{Found, Identity, LocateError, Located, find_strip, find_strips};
use crate::model::{Model, ModelError};

/// A strip read from a photo, every symbol as a distribution.
#[derive(Clone, Debug)]
pub struct ScannedShare {
    /// Which strip the printed code said it was.
    pub identity: Identity,
    /// The recogniser's reading of every cell.
    pub cells: Vec<[Distribution; WORD_LEN]>,
    /// Cells the recogniser could not read (uniform in `cells`), as
    /// `(word_index, position)`.
    pub unread: Vec<(usize, usize)>,
    /// Where the strip was in the photo.
    pub located: Located,
}

impl ScannedShare {
    /// The most probable symbol in every cell.
    pub fn symbols(&self) -> Vec<[Symbol; WORD_LEN]> {
        self.cells
            .iter()
            .map(|row| row.each_ref().map(Distribution::argmax))
            .collect()
    }

    /// The most probable symbols as a text share, if this is a share strip.
    pub fn best_guess(&self) -> Option<Share> {
        self.identity
            .share()
            .map(|index| Share::new(index, self.symbols()))
    }

    /// Mean over cells of the most probable symbol's probability.
    pub fn confidence(&self) -> f64 {
        let total: f64 = self
            .cells
            .iter()
            .flatten()
            .map(|d| f64::from(d.p(d.argmax())))
            .sum();
        total / (self.cells.len() * WORD_LEN).max(1) as f64
    }

    /// The cell whose best symbol is least probable: `(word_index,
    /// position, symbol, probability)`.
    pub fn least_confident(&self) -> Option<(usize, usize, Symbol, f64)> {
        self.cells
            .iter()
            .enumerate()
            .flat_map(|(w, row)| {
                row.iter().enumerate().map(move |(p, d)| {
                    let best = d.argmax();
                    (w, p, best, f64::from(d.p(best)))
                })
            })
            .min_by(|a, b| a.3.total_cmp(&b.3))
    }
}

/// Why locating or classifying yielded no reading. The pipeline's own
/// error is [`crate::ScanError`]; this is what its stages raise.
#[derive(Clone, PartialEq, Debug)]
pub enum ReadError {
    /// The strip could not be found.
    Locate(LocateError),
    /// The recogniser failed on a cell.
    Model(ModelError),
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Locate(e) => e.fmt(f),
            Self::Model(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for ReadError {}

/// Reads the one strip in `photo` with `model`. The identity comes from
/// the printed code and may be [`Identity::Unknown`]; the caller decides
/// what to do with a strip that is not a share.
pub fn scan(photo: &RgbImage, model: &dyn Model) -> Result<ScannedShare, ReadError> {
    let gray = image::DynamicImage::ImageRgb8(photo.clone()).into_luma8();
    read_found(find_strip(&gray).map_err(ReadError::Locate)?, model)
}

/// Reads every strip in `photo` — a not-yet-cut sheet — in reading order.
pub fn scan_all(photo: &RgbImage, model: &dyn Model) -> Result<Vec<ScannedShare>, ReadError> {
    let gray = image::DynamicImage::ImageRgb8(photo.clone()).into_luma8();
    find_strips(&gray)
        .map_err(ReadError::Locate)?
        .into_iter()
        .map(|found| read_found(found, model))
        .collect()
}

fn read_found(found: Found, model: &dyn Model) -> Result<ScannedShare, ReadError> {
    let read = model
        .read_cells(&found.cells, &found.rectified)
        .map_err(ReadError::Model)?;
    read.validate(found.cells.len()).map_err(ReadError::Model)?;
    Ok(ScannedShare {
        identity: found.identity,
        cells: read.cells,
        unread: read.unread,
        located: found.located,
    })
}
