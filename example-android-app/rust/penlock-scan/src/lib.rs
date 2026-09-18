//! Runtime share-card/worksheet support; BIP39 scanning is owned by bitcoin-vision.
#![deny(missing_docs)]

pub mod bare;
pub mod cells;
pub mod locate;
pub mod marks;
pub mod model;
pub mod pdf;
pub mod scan;
pub mod scanner;
pub mod sheet;
pub mod template;
pub mod title;
pub mod wordsheet;

#[cfg(feature = "word-extent")]
pub use bitcoin_vision::extent;
pub use bitcoin_vision::{
    boxes, detect, dot_grid, fragments, homography, hybrid, inference_limit, joins, layout,
    numbering, page_frame, phrase, progress, recogniser, region, sources, split, timing, words,
};
pub use cells::{CellImage, crop_cell, crop_cells};
pub use homography::Homography;
pub use image;
pub use imageproc;
pub use locate::{
    Anchors, Found, Identity, LocateError, Located, find_strip, find_strips, locate, locate_all,
};
pub use model::{CellReadings, CellRef, Model, ModelError};
pub use scan::ScannedShare;
pub use scanner::{
    Cell, Document, ModelBytes, Observation, ScanError, ScanOptions, ScanResult, Scanner,
};
pub use sheet::{Aids, Cut, Fill, Layout, Sheet, Strip, StripKind};
pub use template::{Font, Jitter, Rect, WORDS};
