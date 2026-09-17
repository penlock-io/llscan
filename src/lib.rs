//! Offline BIP39 photo recognition, without training or evaluation dependencies.
//!
//! The high-level entry point is [`Scanner`]. Low-level geometry modules expose
//! the same implementation for integrations and diagnostics; they are not a
//! separate recognition pipeline.

pub mod boxes;
pub mod detect;
#[doc(hidden)]
pub mod dot_grid;
#[cfg(feature = "word-extent")]
pub mod extent;
pub mod fragments;
pub mod homography;
pub mod hybrid;
pub mod inference_limit;
pub mod joins;
pub mod layout;
pub mod numbering;
pub mod page_frame;
pub mod phrase;
pub mod progress;
mod read_budget;
pub mod recogniser;
pub mod region;
mod repair;
#[cfg(feature = "scan-profile")]
pub use repair::set_read_workers;
#[doc(hidden)]
pub mod runtime;
pub mod sources;
pub mod split;
pub mod timing;
pub mod vocabulary;
pub mod words;

pub mod output;
mod scanner;
pub use image;
pub use scanner::{
    ModelBytes, ScanError, ScanOptions, ScanResult, Scanner, check_dimensions, decode_image,
};
pub use vocabulary::Word;
