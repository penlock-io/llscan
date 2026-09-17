//! The phone bridge over UniFFI: the same pipeline the bench gates,
//! behind a UniFFI surface small enough for an app — photograph
//! strips, keep their soft readings, recover the phrase; and the
//! backup the other way round: photograph a written phrase, read its
//! words for the person to confirm, split them into shares to write
//! down; a new phrase made the way bdk makes a wallet's, the wallet a
//! phrase is, as a descriptor to save and export; and a PSBT reviewed
//! against that descriptor and signed from the review.

#![deny(missing_docs)]

use std::sync::Arc;

use bdk_wallet::KeychainKind;
use bdk_wallet::bitcoin::bip32::Xpriv;
use bdk_wallet::bitcoin::secp256k1::Secp256k1;
use bdk_wallet::bitcoin::{Network, NetworkKind};
use bdk_wallet::keys::bip39::{Language, Mnemonic, WordCount};
use bdk_wallet::keys::{GeneratableKey, GeneratedKey};
use bdk_wallet::miniscript::{Descriptor, DescriptorPublicKey};
use bdk_wallet::template::{Bip86, DescriptorTemplate};
use bitcoin_vision::detect::Detector;
use bitcoin_vision::phrase::{CROP_MARGIN, Reader, read_words_observed};
use bitcoin_vision::progress::{Event, FinalRegion, Observer, Phase, emit, work};
use bitcoin_vision::recogniser::{Preparation, Recogniser};
use bitcoin_vision::words::Verdict;
use penlock::soft::combine;
use penlock::soft::search::{Bip39Checksum, Filter, Floor, Outcome, search, slots};
use penlock::word::symbols_to_string;
use penlock::{ShareCount, Word};
use penlock_scan::{Identity, ScannedShare};
use rand::TryRngCore;

mod boxes;
mod diagnostics;
mod photo;
mod progress;
mod psbt;
mod qr;
mod sources;

pub use boxes::{BoxObserver, BoxProgressEvent, BoxProgressUpdate, BoxScan, FinalBox};
pub use diagnostics::{DiagnosedScan, DiagnosticFile};
pub use sources::{DetectionOwner, DetectionPart, DetectionSource, DetectionUnion};

pub use photo::{PhotoUpSource, canonical_photo};
#[cfg(feature = "scan-profile")]
pub use progress::ProgressCallbackProbe;
pub use progress::{
    DeliveryWatch, ProgressDeliveryError, ScanProgressEvent, ScanProgressObserver,
    ScanProgressUpdate, ScanRegion, ScanRegionMapping, ScanRegionState, ScanWorkPhase,
};

pub use psbt::{FEE_CEILING, OutputLine, Review, review_psbt, sign_review};
pub use qr::{PsbtQrDecoder, PsbtQrEncoder, QrModules, QrScan, psbt_qr_parts, qr_modules};

uniffi::setup_scaffolding!();

/// Everything the surface fails with, in the pipeline's own words.
#[derive(Clone, Debug, uniffi::Error)]
pub enum VisionError {
    /// The bytes did not decode as a photo.
    Photo {
        /// The decoder's words.
        detail: String,
    },
    /// The model bytes failed the classifier contract.
    Model {
        /// The loader's words.
        detail: String,
    },
    /// No strip could be found or read in the photo.
    Scan {
        /// The scanner's words.
        detail: String,
    },
    /// The text does not fit a QR code.
    Qr {
        /// The encoder's words.
        detail: String,
    },
    /// The pair cannot recover a phrase.
    Recover {
        /// Why not.
        detail: String,
    },
    /// The words are not a phrase that can be split.
    Phrase {
        /// Which word, or that the checksum fails.
        detail: String,
    },
    /// The PSBT is refused, or cannot be signed as reviewed.
    Psbt {
        /// The reason, naming the input or output.
        detail: String,
    },
}

impl std::fmt::Display for VisionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VisionError::Photo { detail }
            | VisionError::Model { detail }
            | VisionError::Scan { detail }
            | VisionError::Recover { detail }
            | VisionError::Phrase { detail }
            | VisionError::Psbt { detail }
            | VisionError::Qr { detail } => f.write_str(detail),
        }
    }
}

impl std::error::Error for VisionError {}

/// The recogniser, loaded once per app from the model's bytes.
#[derive(uniffi::Object)]
pub struct Scanner {
    scanner: penlock_scan::Scanner,
}

#[uniffi::export]
impl Scanner {
    /// Builds the recogniser from `cell-reference.onnx`'s bytes.
    #[uniffi::constructor]
    pub fn new(model: Vec<u8>) -> Result<Self, VisionError> {
        Ok(Scanner {
            scanner: penlock_scan::Scanner::new(penlock_scan::ModelBytes { cells: &model })
                .map_err(|e| VisionError::Model {
                    detail: e.to_string(),
                })?,
        })
    }

    /// Reads every strip in one photo (JPEG, PNG or WebP bytes).
    pub fn read(&self, photo: Vec<u8>) -> Result<Vec<Arc<StripReading>>, VisionError> {
        self.read_oriented(photo, 1)
    }

    /// Reads strips after applying the app's resolved EXIF-numbered transform once.
    pub fn read_oriented(
        &self,
        photo: Vec<u8>,
        orientation: u8,
    ) -> Result<Vec<Arc<StripReading>>, VisionError> {
        let photo = photo::decode(&photo, orientation)?;
        let read = self
            .scanner
            .scan_all_rgb(&photo, penlock_scan::ScanOptions::default())
            .map_err(|e| VisionError::Scan {
                detail: e.to_string(),
            })?;
        Ok(read
            .shares
            .into_iter()
            .map(|share| Arc::new(StripReading { share }))
            .collect())
    }
}

/// One strip read from a photo, its soft readings kept for recovery.
#[derive(uniffi::Object)]
pub struct StripReading {
    share: ScannedShare,
}

/// What the strip's printed marking said it was.
#[derive(Clone, Copy, PartialEq, Eq, Debug, uniffi::Enum)]
pub enum StripIdentity {
    /// The seed-phrase strip.
    Seed,
    /// A share strip.
    Share {
        /// The share number written on the worksheet, in `1..=3`.
        index: u8,
    },
    /// The marking could not be read.
    Unknown,
}

#[uniffi::export]
impl StripReading {
    /// What the strip is.
    pub fn identity(&self) -> StripIdentity {
        match self.share.identity {
            Identity::Seed => StripIdentity::Seed,
            Identity::Share(i) => StripIdentity::Share { index: i.number() },
            Identity::Unknown => StripIdentity::Unknown,
        }
    }

    /// Rows the strip carries.
    pub fn rows(&self) -> u32 {
        self.share.cells.len() as u32
    }

    /// Cells the recogniser could not read at all.
    pub fn unread(&self) -> u32 {
        self.share.unread.len() as u32
    }
}

/// Recovers the phrase from two share strips: their readings combined,
/// candidate phrases enumerated best-first, the first that passes the
/// BIP39 checksum returned. The checksum is a filter — a wrong phrase
/// slips through about one time in sixteen — not an oracle; a saved
/// wallet descriptor will later confirm the phrase properly.
#[uniffi::export]
pub fn recover(a: Arc<StripReading>, b: Arc<StripReading>) -> Result<Vec<String>, VisionError> {
    let index = |s: &StripReading| {
        s.share
            .identity
            .share()
            .ok_or_else(|| VisionError::Recover {
                detail: "recovery needs two share strips".into(),
            })
    };
    let (index_a, index_b) = (index(&a)?, index(&b)?);
    let mut combined = Vec::with_capacity(a.share.cells.len());
    for (x, y) in a.share.cells.iter().zip(&b.share.cells) {
        combined.push(
            combine(x, index_a, y, index_b).map_err(|e| VisionError::Recover {
                detail: e.to_string(),
            })?,
        );
    }
    let ranked = slots(&combined, &[], Floor::DEFAULT);
    match search(&ranked, &[&Bip39Checksum], None, 0) {
        Outcome::Unconfirmed { words, .. } => Ok(words.iter().map(|w| w.to_string()).collect()),
        _ => Err(VisionError::Recover {
            detail: "no candidate phrase passed the BIP39 checksum".into(),
        }),
    }
}

/// The phrase reader: the word detector, the word model with its
/// calibration, and the text recogniser that says what is written
/// whether or not it is a word, loaded once per app from the bundled
/// bytes.
#[derive(uniffi::Object)]
pub struct PhraseScanner {
    detector: Detector,
    reader: Reader,
    recogniser: Recogniser,
}

/// One list word with the probability the model gives it.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct RankedWord {
    /// The list word.
    pub word: String,
    /// Its probability under the model.
    pub probability: f32,
}

/// Original observation retained by an extent replacement, not a native join.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ExtentParent {
    /// Index into the returned page's words, including inactive alternatives.
    pub word_index: u32,
    /// Whether this original was left out before the expansion was selected.
    pub stray: bool,
}

/// One word found on the page and read.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct OcrObservation {
    /// Original photo polygon.
    pub corners: Vec<f32>,
    /// Exact crop supplied to OCR.
    pub crop_png: Vec<u8>,
    /// Verbatim OCR of that crop.
    pub raw: String,
}

/// Matching input is distinct from verbatim OCR.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct MatchingEvidence {
    /// Selected matching text before normalisation.
    pub text: String,
    /// Whether the selected literal belongs to original_ocr.
    pub original: bool,
    /// Leading UTF-8 bytes omitted from that literal.
    pub removed_prefix_bytes: u32,
}

/// Observed label values are not trusted word numbers.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct LabelObservation {
    /// Source polygon in photo coordinates (parent polygon for a prefix token).
    pub corners: Vec<f32>,
    /// Literal label observation.
    pub literal: String,
    /// prefix, token, standalone or beside.
    pub origin: String,
    /// Exact observed digits; not a trusted phrase position.
    pub ordinal: Option<u32>,
    /// Mandatory dotted-label evidence.
    pub dotted: bool,
    /// No unique geometric owner was found.
    pub ambiguous: bool,
}

/// One word found on the page and read.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct WordReading {
    /// The word's four corners in photo pixels, clockwise from the top
    /// left, as x1, y1, ... x4, y4.
    pub corners: Vec<f32>,
    /// The model's five most probable list words, most probable first.
    pub ranked: Vec<RankedWord>,
    /// Whether the calibration accepts the top word; otherwise the
    /// person should look at the crop.
    pub accepted: bool,
    /// The word to show first: the hybrid pick over the recogniser's
    /// read and the model's five, or the model's top without a read.
    pub read: String,
    /// The recogniser's raw read of the crop, empty without one.
    pub raw: String,
    /// Original crop/read retained only when an ink cut was adopted.
    pub original_ocr: Option<OcrObservation>,
    /// Explicit source/transform used for word selection.
    pub matching: Option<MatchingEvidence>,
    /// Every label observation, including unknown ordinals.
    pub labels: Vec<LabelObservation>,
    /// Conflicting or duplicate exact label values.
    pub label_conflict: bool,
    /// Not a word: left out of both reading orders.
    pub stray: bool,
    /// The item number the page's numbering gives the word, when the
    /// page is numbered and this word has a place in the run.
    pub number: Option<u32>,
    /// A word by its read, but far from the phrase's region: a stray
    /// for that reason, not for what it read as.
    pub apart: bool,
    /// The level crop the model read, as PNG bytes, for the person to
    /// compare the reading against.
    pub crop_png: Vec<u8>,
    /// Original pieces replaced by this joined word, indexed into the page.
    /// Restoring those pieces must deactivate this word, and vice versa.
    pub joined_from: Vec<u32>,
    /// One or two original observations replaced by this expanded crop.
    /// Switching back restores their prior keep state and clears user checks.
    pub expanded_from: Vec<ExtentParent>,
}

impl TryFrom<&bitcoin_vision::phrase::WordBox> for WordReading {
    type Error = VisionError;

    fn try_from(w: &bitcoin_vision::phrase::WordBox) -> Result<Self, Self::Error> {
        let chosen = w
            .pick()
            .and_then(|p| Word::from_index(p as u16))
            .ok_or_else(|| VisionError::Scan {
                detail: "word observation has no selection".into(),
            })?;
        let _png = bitcoin_vision::timing::span("mobile.crop_png");
        let mut crop_png = Vec::new();
        w.crop
            .write_to(
                &mut std::io::Cursor::new(&mut crop_png),
                bitcoin_vision::image::ImageFormat::Png,
            )
            .map_err(|e| VisionError::Scan {
                detail: e.to_string(),
            })?;
        drop(_png);
        Ok(Self {
            corners: w.quad.0.iter().flat_map(|&(x, y)| [x, y]).collect(),
            ranked: w
                .ranked
                .iter()
                .take(5)
                .filter_map(|&(i, p)| {
                    Word::from_index(i as u16).map(|word| RankedWord {
                        word: word.as_str().to_owned(),
                        probability: p,
                    })
                })
                .collect(),
            accepted: w.verdict() == Verdict::Accept,
            read: chosen.as_str().to_owned(),
            raw: w.raw.as_ref().map_or(String::new(), |r| r.text.clone()),
            original_ocr: w
                .evidence
                .original
                .as_ref()
                .map(|r| {
                    let mut crop_png = Vec::new();
                    r.crop
                        .write_to(
                            &mut std::io::Cursor::new(&mut crop_png),
                            bitcoin_vision::image::ImageFormat::Png,
                        )
                        .map_err(|e| VisionError::Scan {
                            detail: e.to_string(),
                        })?;
                    Ok::<_, VisionError>(OcrObservation {
                        corners: r.quad.0.iter().flat_map(|&(x, y)| [x, y]).collect(),
                        crop_png,
                        raw: r.raw.text.clone(),
                    })
                })
                .transpose()?,
            matching: w.evidence.matching.as_ref().map(|m| MatchingEvidence {
                text: m.text.clone(),
                original: m.original,
                removed_prefix_bytes: m.removed_prefix_bytes as u32,
            }),
            labels: w
                .evidence
                .labels
                .iter()
                .map(|l| LabelObservation {
                    corners: l.corners.iter().flat_map(|&(x, y)| [x, y]).collect(),
                    literal: l.literal.clone(),
                    origin: l.origin.clone(),
                    ordinal: l.ordinal,
                    dotted: l.dotted,
                    ambiguous: l.ambiguous,
                })
                .collect(),
            label_conflict: w.evidence.conflict,
            stray: w.stray,
            number: w.number,
            apart: w.apart,
            crop_png,
            joined_from: w
                .joined_from
                .into_iter()
                .flatten()
                .map(|i| i as u32)
                .collect(),
            expanded_from: w
                .expanded_from
                .iter()
                .map(|p| ExtentParent {
                    word_index: p.word_index as u32,
                    stray: p.stray,
                })
                .collect(),
        })
    }
}

/// What the item numbers written beside the words made, over the
/// whole page.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum Numbering {
    /// Too few numbers to trust a sequence; list_numbered is independent.
    None,
    /// The numbers read as a run 1…count: `missing` are the numbers no
    /// word carries, each a word not found; `out_of_place` are numbers
    /// whose place on the page is not their place in the run.
    Held {
        /// The highest number the run reaches.
        count: u32,
        /// The numbers no word carries.
        missing: Vec<u32>,
        /// The numbers out of their place on the page.
        out_of_place: Vec<u32>,
        /// Indices into `words` that carry a label the run gave no
        /// number to.
        unresolved: Vec<u32>,
    },
    /// The numbers contradict one another, so none is trusted.
    Inconsistent {
        /// What contradicted what.
        detail: String,
    },
}

/// How a photo was read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum Layout {
    /// A recovery phrase sheet, found by its marks and read field by
    /// field: the twelve fields in order, both orders the same, a
    /// blank field a stray at its place.
    WordSheet,
    /// Any other page: words found by the detector, in either order.
    Page,
}

/// The shared Rust stage's initial traversal, mapped without re-deciding it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum InitialOrder {
    /// Held numbers take precedence.
    Numbers,
    /// The stage chose rows.
    Rows,
    /// The stage chose columns.
    Columns,
}

impl From<bitcoin_vision::phrase::InitialOrder> for InitialOrder {
    fn from(value: bitcoin_vision::phrase::InitialOrder) -> Self {
        match value {
            bitcoin_vision::phrase::InitialOrder::Numbers => Self::Numbers,
            bitcoin_vision::phrase::InitialOrder::Rows => Self::Rows,
            bitcoin_vision::phrase::InitialOrder::Columns => Self::Columns,
        }
    }
}

/// A photographed page read: every word, the two orders the words can
/// be read in, and whether either passes the BIP39 checksum. Passing
/// means the phrase is well formed, never that it is right: a wrong
/// twelve-word phrase passes about one time in sixteen.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct PhraseScan {
    /// Positive numbered-list mode, even without a trusted numeric order.
    pub list_numbered: bool,
    /// How the photo was read.
    pub layout: Layout,
    /// The photo's width in pixels.
    pub width: u32,
    /// The photo's height in pixels.
    pub height: u32,
    /// The words found.
    pub words: Vec<WordReading>,
    /// Indices into `words` reading down columns, left to right, strays
    /// included at their places.
    pub columns: Vec<u32>,
    /// Indices into `words` reading along rows, top to bottom.
    pub rows: Vec<u32>,
    /// Whether the kept words' picks in column order pass the checksum.
    pub columns_pass: bool,
    /// Whether the kept words' picks in row order pass the checksum.
    pub rows_pass: bool,
    /// What the page's item numbers made.
    pub numbering: Numbering,
    /// Indices into `words` in the numbers' order: the numbered words
    /// by number, then the rest in column order; the column order on a
    /// page without numbers.
    pub numbered: Vec<u32>,
    /// Whether the kept words' picks in the numbers' order pass the
    /// checksum.
    pub numbered_pass: bool,
    /// The stage's initial choice; consumers must not reimplement its policy.
    pub initial_order: InitialOrder,
    /// Initial traversal lacks independent layout/number support; review positions.
    pub order_requires_review: bool,
    /// Why the shared native traversal was suggested.
    pub order_basis: String,
    /// Raw detector geometry and outcomes, including ink with no word reading.
    pub sources: Vec<DetectionSource>,
}

impl PhraseScanner {
    /// Read one synthetic exact-width crop for resident-cache diagnostics.
    /// Deliberately outside the UniFFI-exported implementation.
    #[cfg(feature = "scan-profile")]
    pub fn profile_blank_ocr(&self, width: u32) -> Result<(), VisionError> {
        self.recogniser
            .profile_blank_width(width)
            .map_err(|detail| VisionError::Model { detail })
    }

    fn build(
        detector: Vec<u8>,
        model: Vec<u8>,
        calibration: String,
        recogniser: Vec<u8>,
        dictionary: String,
        preparation: Preparation,
    ) -> Result<Self, VisionError> {
        let model_error = |detail| VisionError::Model { detail };
        Ok(PhraseScanner {
            detector: Detector::from_bytes(&detector).map_err(model_error)?,
            reader: Reader::from_bytes(&model, &calibration).map_err(model_error)?,
            recogniser: Recogniser::from_bytes_with_preparation(
                &recogniser,
                &dictionary,
                preparation,
            )
            .map_err(model_error)?,
        })
    }

    /// Diagnostic constructor only, deliberately not exported to UniFFI. The
    /// app's normal constructor has only the symbolic preparation path.
    #[cfg(feature = "scan-profile")]
    pub fn new_for_profile(
        detector: Vec<u8>,
        model: Vec<u8>,
        calibration: String,
        recogniser: Vec<u8>,
        dictionary: String,
        preparation: Preparation,
    ) -> Result<Self, VisionError> {
        Self::build(
            detector,
            model,
            calibration,
            recogniser,
            dictionary,
            preparation,
        )
    }
}

#[uniffi::export]
impl PhraseScanner {
    /// Builds the reader from the bytes of `word-detector.onnx`,
    /// `word-reference.onnx` with the text of `word-reference.calibration`
    /// (refusing a model the calibration was not selected on), and
    /// `text-recogniser.onnx` with the text of its dictionary.
    #[uniffi::constructor]
    pub fn new(
        detector: Vec<u8>,
        model: Vec<u8>,
        calibration: String,
        recogniser: Vec<u8>,
        dictionary: String,
    ) -> Result<Self, VisionError> {
        let _init = bitcoin_vision::timing::span("mobile.init");
        Self::build(
            detector,
            model,
            calibration,
            recogniser,
            dictionary,
            Preparation::Symbolic,
        )
    }

    /// Reads every word in one photo of a written phrase (JPEG, PNG or
    /// WebP bytes).
    pub fn scan(&self, photo: Vec<u8>) -> Result<PhraseScan, VisionError> {
        self.scan_oriented(photo, 1)
    }

    /// Reads in the app's canonical frame. All returned coordinates and crops
    /// refer to the decoded image after this EXIF-numbered transform, not raw bytes.
    pub fn scan_oriented(
        &self,
        photo: Vec<u8>,
        orientation: u8,
    ) -> Result<PhraseScan, VisionError> {
        self.scan_with_photo_up(photo, orientation, PhotoUpSource::Unknown)
    }

    /// Canonical-frame scan with independent caller provenance. Orientation
    /// applies once to pixels; photo-up is only a page writing-direction hint.
    pub fn scan_with_photo_up(
        &self,
        photo: Vec<u8>,
        orientation: u8,
        photo_up: PhotoUpSource,
    ) -> Result<PhraseScan, VisionError> {
        self.scan_with_observer(photo, orientation, photo_up, None)
    }

    /// The same scan/result with live, per-call geometry and phase callbacks.
    /// Callback failure or detachment does not cancel native recognition.
    pub fn scan_oriented_with_progress(
        &self,
        photo: Vec<u8>,
        orientation: u8,
        observer: Box<dyn ScanProgressObserver>,
    ) -> Result<PhraseScan, VisionError> {
        self.scan_with_photo_up_and_progress(photo, orientation, PhotoUpSource::Unknown, observer)
    }

    /// The same provenance-aware scan with per-call progress.
    pub fn scan_with_photo_up_and_progress(
        &self,
        photo: Vec<u8>,
        orientation: u8,
        photo_up: PhotoUpSource,
        observer: Box<dyn ScanProgressObserver>,
    ) -> Result<PhraseScan, VisionError> {
        let observer = progress::CallbackObserver::new(observer);
        self.scan_with_observer(photo, orientation, photo_up, Some(&observer))
    }

    /// Explicit per-call decision recording for a future saved bug export.
    /// Shares the loaded immutable classifier plan; does not change the resident
    /// reader, rerun recognition, write files, or enable tracing on later scans.
    pub fn scan_oriented_with_diagnostics(
        &self,
        photo: Vec<u8>,
        orientation: u8,
        observer: Box<dyn ScanProgressObserver>,
    ) -> Result<DiagnosedScan, VisionError> {
        self.scan_with_photo_up_and_diagnostics(
            photo,
            orientation,
            PhotoUpSource::Unknown,
            observer,
        )
    }

    /// Explicit decision recording, with provenance and the actual pixel
    /// transform recorded separately. The resident reader is never mutated.
    pub fn scan_with_photo_up_and_diagnostics(
        &self,
        photo: Vec<u8>,
        orientation: u8,
        photo_up: PhotoUpSource,
        observer: Box<dyn ScanProgressObserver>,
    ) -> Result<DiagnosedScan, VisionError> {
        let observer = progress::CallbackObserver::new(observer);
        let reader = self
            .reader
            .clone()
            .with_decision_trace(true)
            .with_photo_up(photo_up.into());
        let mut files = Vec::new();
        let scan = progress::complete(Some(&observer), || {
            let (layout, scan, regions) = self.scan_page_using(
                photo,
                orientation,
                Some(&observer),
                &reader,
                #[cfg(feature = "extent-experiment")]
                None,
            )?;
            let result = pack_scan(layout, &scan, Some(&observer))?;
            files = diagnostics::pack(&scan, &result, &regions, orientation, photo_up)?;
            Ok((result, regions))
        })?;
        Ok(DiagnosedScan { scan, files })
    }

    /// Return final selected boxes from an image, without exposing words.
    /// Uses the same pipeline (including internal recognition) and canonical
    /// EXIF-numbered orientation as scan_oriented. No second scan is performed.
    pub fn scan_boxes(&self, photo: Vec<u8>, orientation: u8) -> Result<BoxScan, VisionError> {
        self.scan_boxes_observed(photo, orientation, None)
    }

    /// Boxes-only scan with live provisional geometry and an authoritative
    /// final set. Callback detachment does not cancel native computation.
    pub fn scan_boxes_with_progress(
        &self,
        photo: Vec<u8>,
        orientation: u8,
        observer: Box<dyn BoxObserver>,
    ) -> Result<BoxScan, VisionError> {
        self.scan_boxes_observed(photo, orientation, Some(observer))
    }
}

#[cfg(feature = "extent-experiment")]
enum ExtentOutput<'a> {
    Baseline,
    Diagnostic(&'a mut String),
    Lean(&'a mut Option<bitcoin_vision::extent::Report>),
}

impl PhraseScanner {
    fn scan_boxes_observed(
        &self,
        photo: Vec<u8>,
        orientation: u8,
        callback: Option<Box<dyn BoxObserver>>,
    ) -> Result<BoxScan, VisionError> {
        let _total = bitcoin_vision::timing::span("mobile.total");
        let observer = boxes::Callback::new(callback);
        observer.complete(|| {
            // Some(observer) keeps the stage-owned ID mapping even without a
            // foreign callback. It neither selects boxes nor retains readings.
            let (_, scan, regions) = self.scan_page(
                photo,
                orientation,
                Some(&observer),
                #[cfg(feature = "extent-experiment")]
                None,
            )?;
            work(Some(&observer), Phase::Packing, 0, None);
            bitcoin_vision::boxes::BoxScan::from_page(&scan, &regions)
                .map_err(|detail| VisionError::Scan { detail })
        })
    }

    /// Historical baseline/E3 cost comparison through the shared scan owner.
    /// Deliberately outside UniFFI; normal app calls always use lean E3.
    #[cfg(feature = "extent-experiment")]
    pub fn scan_extent_for_profile(
        &self,
        photo: Vec<u8>,
        use_extent: bool,
    ) -> Result<(PhraseScan, String), VisionError> {
        let _total = bitcoin_vision::timing::span("mobile.total");
        let mut report = String::new();
        let (scan, _) = self.scan_inner(
            photo,
            1,
            None,
            Some(if use_extent {
                ExtentOutput::Diagnostic(&mut report)
            } else {
                ExtentOutput::Baseline
            }),
        )?;
        Ok((scan, report))
    }

    /// Same lean E3 arm as normal scans, retaining its compact report for the
    /// benchmark only. False explicitly selects the historical baseline.
    #[cfg(feature = "extent-experiment")]
    pub fn scan_extent_lean_for_profile(
        &self,
        photo: Vec<u8>,
        use_extent: bool,
    ) -> Result<(PhraseScan, Option<bitcoin_vision::extent::Report>), VisionError> {
        let _total = bitcoin_vision::timing::span("mobile.total");
        let mut report = None;
        let (scan, _) = self.scan_inner(
            photo,
            1,
            None,
            Some(if use_extent {
                ExtentOutput::Lean(&mut report)
            } else {
                ExtentOutput::Baseline
            }),
        )?;
        Ok((scan, report))
    }

    fn scan_with_observer(
        &self,
        photo: Vec<u8>,
        orientation: u8,
        photo_up: PhotoUpSource,
        observer: Option<&dyn Observer>,
    ) -> Result<PhraseScan, VisionError> {
        let _total = bitcoin_vision::timing::span("mobile.total");
        let reader = self.reader.clone().with_photo_up(photo_up.into());
        progress::complete(observer, || {
            let (layout, scan, regions) = self.scan_page_using(
                photo,
                orientation,
                observer,
                &reader,
                #[cfg(feature = "extent-experiment")]
                None,
            )?;
            Ok((pack_scan(layout, &scan, observer)?, regions))
        })
    }

    fn scan_page(
        &self,
        photo: Vec<u8>,
        orientation: u8,
        observer: Option<&dyn Observer>,
        #[cfg(feature = "extent-experiment")] extent_report: Option<ExtentOutput<'_>>,
    ) -> Result<(Layout, bitcoin_vision::phrase::PageScan, Vec<FinalRegion>), VisionError> {
        self.scan_page_using(
            photo,
            orientation,
            observer,
            &self.reader,
            #[cfg(feature = "extent-experiment")]
            extent_report,
        )
    }

    fn scan_page_using(
        &self,
        photo: Vec<u8>,
        orientation: u8,
        observer: Option<&dyn Observer>,
        reader: &Reader,
        #[cfg(feature = "extent-experiment")] extent_report: Option<ExtentOutput<'_>>,
    ) -> Result<(Layout, bitcoin_vision::phrase::PageScan, Vec<FinalRegion>), VisionError> {
        work(observer, Phase::Preparing, 0, None);
        let photo = photo::decode(&photo, orientation)?;
        emit(observer, || Event::Photo {
            width: photo.width(),
            height: photo.height(),
        });
        work(observer, Phase::Finding, 0, None);
        let scan_error = |detail| VisionError::Scan { detail };
        // a recovery phrase sheet is read by its layout; anything else by
        // the detector. A sheet that is found but cannot be read is an
        // error, never a page for the detector.
        let _sheet = bitcoin_vision::timing::span("mobile.sheet_probe_read");
        let sheet = penlock_scan::wordsheet::read_observed(
            &photo,
            reader,
            Some(&self.recogniser),
            CROP_MARGIN,
            observer,
        )
        .map_err(scan_error)?;
        drop(_sheet);
        let (layout, scan, regions) = match sheet {
            Some((scan, regions)) => (Layout::WordSheet, scan, regions),
            None => {
                let _detect = bitcoin_vision::timing::span("mobile.detect");
                let quads = self.detector.detect(&photo).map_err(scan_error)?;
                drop(_detect);
                let read_normal = || {
                    read_words_observed(
                        &photo,
                        &quads,
                        reader,
                        Some(&self.recogniser),
                        CROP_MARGIN,
                        observer,
                    )
                };
                #[cfg(feature = "extent-experiment")]
                let (scan, regions) = if let Some(output) = extent_report {
                    if matches!(output, ExtentOutput::Baseline) {
                        bitcoin_vision::phrase::read_words_baseline_observed(
                            &photo,
                            &quads,
                            reader,
                            Some(&self.recogniser),
                            CROP_MARGIN,
                            observer,
                        )
                        .map_err(scan_error)?
                    } else {
                        let audit = if matches!(output, ExtentOutput::Diagnostic(_)) {
                            bitcoin_vision::extent::Audit::Components
                        } else {
                            bitcoin_vision::extent::Audit::None
                        };
                        let (experiment, regions) =
                            bitcoin_vision::phrase::read_words_with_extent_e3_observed(
                                &photo,
                                &quads,
                                reader,
                                &self.recogniser,
                                audit,
                                observer,
                            )
                            .map_err(scan_error)?;
                        match output {
                            ExtentOutput::Diagnostic(output) => {
                                let value = experiment.report.to_json();
                                let _serialize =
                                    bitcoin_vision::timing::span("phrase.extent.serialize");
                                *output = value.to_string();
                            }
                            ExtentOutput::Lean(output) => *output = Some(experiment.report),
                            ExtentOutput::Baseline => unreachable!("baseline handled before E3"),
                        }
                        (experiment.scan, regions)
                    }
                } else {
                    read_normal().map_err(scan_error)?
                };
                #[cfg(not(feature = "extent-experiment"))]
                let (scan, regions) = read_normal().map_err(scan_error)?;
                (Layout::Page, scan, regions)
            }
        };
        Ok((layout, scan, regions))
    }

    #[cfg(feature = "extent-experiment")]
    fn scan_inner(
        &self,
        photo: Vec<u8>,
        orientation: u8,
        observer: Option<&dyn Observer>,
        #[cfg(feature = "extent-experiment")] extent_report: Option<ExtentOutput<'_>>,
    ) -> Result<(PhraseScan, Vec<FinalRegion>), VisionError> {
        let (layout, scan, regions) = self.scan_page(
            photo,
            orientation,
            observer,
            #[cfg(feature = "extent-experiment")]
            extent_report,
        )?;
        Ok((pack_scan(layout, &scan, observer)?, regions))
    }
}

fn pack_scan(
    layout: Layout,
    scan: &bitcoin_vision::phrase::PageScan,
    observer: Option<&dyn Observer>,
) -> Result<PhraseScan, VisionError> {
    work(observer, Phase::Packing, 0, None);
    let _pack = bitcoin_vision::timing::span("mobile.pack_result");
    let mut words = Vec::with_capacity(scan.words.len());
    for w in &scan.words {
        words.push(WordReading::try_from(w)?);
    }
    // The stage owns the policy; all orders retain strays for restoration.
    let initial = scan.initial_traversal();
    let columns: Vec<usize> = (0..scan.words.len()).collect();
    let rows = scan.by_rows();
    let numbered = scan.by_numbers();
    let numbering = match &scan.numbering {
        bitcoin_vision::numbering::Numbering::None => Numbering::None,
        bitcoin_vision::numbering::Numbering::Held {
            count,
            missing,
            out_of_place,
            unresolved,
            ..
        } => Numbering::Held {
            count: *count,
            missing: missing.clone(),
            out_of_place: out_of_place.clone(),
            unresolved: unresolved.iter().map(|&i| i as u32).collect(),
        },
        bitcoin_vision::numbering::Numbering::Inconsistent { detail } => Numbering::Inconsistent {
            detail: detail.clone(),
        },
    };
    Ok(PhraseScan {
        list_numbered: scan.list_mode() == bitcoin_vision::numbering::dotted::ListMode::Numbered,
        layout,
        width: scan.width,
        height: scan.height,
        columns_pass: initial.columns_pass,
        rows_pass: initial.rows_pass,
        numbered_pass: initial.numbered_pass,
        initial_order: initial.order.into(),
        order_requires_review: initial.requires_review,
        order_basis: initial.basis.to_owned(),
        columns: columns.iter().map(|&i| i as u32).collect(),
        rows: rows.iter().map(|&i| i as u32).collect(),
        numbered: numbered.iter().map(|&i| i as u32).collect(),
        numbering,
        words,
        sources: sources::pack(&scan.sources),
    })
}

/// Numeric diagnostics for the isolated app timing build. Drain on the same
/// calling thread after the synchronous constructor/scan, outside their spans.
/// No photo, recognized text or ranked word is included, and normal builds do
/// not export this function or collect these timings.
#[cfg(feature = "scan-profile")]
#[uniffi::export]
pub fn take_scan_profile() -> String {
    bitcoin_vision::timing::take().to_string()
}

fn parse_words(words: &[String]) -> Result<Vec<Word>, VisionError> {
    words
        .iter()
        .map(|w| {
            Word::from_bip39(w).ok_or_else(|| VisionError::Phrase {
                detail: format!("{w:?} is not a word of the list"),
            })
        })
        .collect()
}

/// Whether the words pass the BIP39 checksum: well formed, not
/// confirmed, since a wrong twelve-word phrase passes about one time in
/// sixteen. A word outside the list or a count the standard does not
/// allow fails.
#[uniffi::export]
pub fn checksum_passes(words: Vec<String>) -> bool {
    parse_words(&words).is_ok_and(|w| Bip39Checksum.passes(&w))
}

/// The list words starting with `prefix`, in list order, for a
/// correction field; the whole list for an empty prefix.
#[uniffi::export]
pub fn list_words(prefix: String) -> Vec<String> {
    let prefix = prefix.trim().to_lowercase();
    Word::all()
        .map(|w| w.as_str())
        .filter(|w| w.starts_with(&prefix))
        .map(str::to_owned)
        .collect()
}

/// One share as it is written onto its worksheet strip.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct ShareText {
    /// The share number, 1 to 3.
    pub number: u8,
    /// One row per word in phrase order, the body of the worksheet's
    /// numbered line: the two checksum symbols, a space, the four
    /// letter symbols.
    pub rows: Vec<String>,
}

/// Splits a confirmed phrase into the 2-of-3 backup with the OS random
/// generator. Refuses a word outside the list or a phrase that fails the
/// checksum.
#[uniffi::export]
pub fn split_phrase(words: Vec<String>) -> Result<Vec<ShareText>, VisionError> {
    let words = parse_words(&words)?;
    if !Bip39Checksum.passes(&words) {
        return Err(VisionError::Phrase {
            detail: "the phrase does not pass the BIP39 checksum".into(),
        });
    }
    let mut rng = rand::rngs::OsRng.unwrap_err();
    Ok(penlock::split(&words, ShareCount::THREE, &mut rng)
        .iter()
        .map(|share| ShareText {
            number: share.index().number(),
            rows: share
                .words()
                .iter()
                .map(|&w| {
                    let s = symbols_to_string(w);
                    format!("{} {}", &s[..2], &s[2..])
                })
                .collect(),
        })
        .collect())
}

#[cfg(test)]
mod phrase_tests {
    use super::*;
    use penlock::share::parse_words as parse_rows;
    use penlock::{Share, ShareIndex};

    #[test]
    fn extent_provenance_and_original_read_material_survive_mobile_packing() {
        use bitcoin_vision::phrase::{ExtentParent as Parent, WordBox};
        let mut word = WordBox {
            quad: bitcoin_vision::detect::Quad([(10., 20.), (30., 20.), (30., 40.), (10., 40.)]),
            crop: bitcoin_vision::image::GrayImage::from_raw(2, 3, vec![0, 10, 20, 30, 40, 255])
                .unwrap(),
            ranked: vec![(0, 0.7), (1, 0.3)],
            selection: Some(
                bitcoin_vision::hybrid::select(
                    Some("abamdon"),
                    None,
                    &[(0, 0.7), (1, 0.3)],
                    0.85,
                    0.75,
                )
                .unwrap(),
            ),
            column_rank: 1,
            row_rank: 3,
            turned: false,
            raw: Some(bitcoin_vision::recogniser::LineRead {
                text: "abamdon".into(),
                confidence: 0.6,
            }),
            evidence: Default::default(),
            narrowed: false,
            stray: false,
            number: None,
            label: None,
            apart: false,
            joined_from: None,
            expanded_from: vec![Parent {
                word_index: 0,
                stray: false,
            }],
        };
        for pair in [false, true] {
            if pair {
                word.expanded_from.push(Parent {
                    word_index: 2,
                    stray: true,
                });
            }
            let packed = WordReading::try_from(&word).unwrap();
            assert!(!packed.accepted && !packed.stray);
            assert!(packed.joined_from.is_empty());
            assert_eq!(packed.expanded_from.len(), if pair { 2 } else { 1 });
            assert_eq!(
                packed.expanded_from[0],
                ExtentParent {
                    word_index: 0,
                    stray: false
                }
            );
            if pair {
                assert_eq!(
                    packed.expanded_from[1],
                    ExtentParent {
                        word_index: 2,
                        stray: true
                    }
                );
            }
            assert_eq!(packed.read, "abandon");
            assert_eq!(packed.raw, "abamdon");
            assert_eq!(packed.ranked[0].probability, 0.7);
            assert_eq!(packed.corners, vec![10., 20., 30., 20., 30., 40., 10., 40.]);
            assert_eq!(
                bitcoin_vision::image::load_from_memory(&packed.crop_png)
                    .unwrap()
                    .to_luma8(),
                word.crop
            );
        }
        // Normal native joins retain their own ABI field, not extent metadata.
        word.expanded_from.clear();
        // An unreadable occupied cell remains one uncertain app entry at its
        // geometric position, carrying exactly the native box and read pixels.
        word.evidence.cell = Some(bitcoin_vision::phrase::CellSupport {
            column: Some(0),
            row: Some(5),
            ordinal: Some(6),
            quad: word.quad.0,
            ink_pixels: 80,
            basis: "label_track_cell_and_word_ink",
        });
        word.number = Some(6);
        let cell = WordReading::try_from(&word).unwrap();
        assert_eq!(cell.number, Some(6));
        assert!(!cell.stray && !cell.accepted);
        assert!(cell.joined_from.is_empty() && cell.expanded_from.is_empty());
        assert_eq!(cell.corners, vec![10., 20., 30., 20., 30., 40., 10., 40.]);
        assert_eq!(
            bitcoin_vision::image::load_from_memory(&cell.crop_png)
                .unwrap()
                .to_luma8(),
            word.crop
        );
        word.evidence.cell = None;
        word.number = None;
        word.joined_from = Some([0, 2]);
        let packed = WordReading::try_from(&word).unwrap();
        assert_eq!(packed.joined_from, vec![0, 2]);
        assert!(packed.expanded_from.is_empty());

        // Trimming is metadata, not another replacement/phrase entry. Each
        // verbatim read survives beside its own pixels through the ABI.
        word.raw.as_mut().unwrap().text = ". Hobby".into();
        word.evidence.original = Some(bitcoin_vision::phrase::OriginalRead {
            quad: bitcoin_vision::detect::Quad([(0., 20.), (30., 20.), (30., 40.), (0., 40.)]),
            crop: bitcoin_vision::image::GrayImage::new(4, 3),
            raw: bitcoin_vision::recogniser::LineRead {
                text: "9.Hobby".into(),
                confidence: 0.8,
            },
        });
        word.evidence.matching = Some(bitcoin_vision::phrase::MatchingRead {
            text: "Hobby".into(),
            original: true,
            removed_prefix_bytes: 2,
        });
        word.evidence
            .labels
            .push(bitcoin_vision::phrase::LabelRead {
                corners: word.quad.0,
                literal: "9.Hobby".into(),
                origin: "prefix".into(),
                ordinal: Some(9),
                dotted: true,
                ambiguous: false,
            });
        let packed = WordReading::try_from(&word).unwrap();
        assert_eq!(packed.raw, ". Hobby");
        let original = packed.original_ocr.unwrap();
        assert_eq!(original.raw, "9.Hobby");
        assert_eq!(original.corners[0], 0.);
        assert_eq!(packed.corners[0], 10.);
        assert_eq!(
            bitcoin_vision::image::load_from_memory(&original.crop_png)
                .unwrap()
                .width(),
            4
        );
        assert_eq!(
            bitcoin_vision::image::load_from_memory(&packed.crop_png)
                .unwrap()
                .width(),
            2
        );
        let matching = packed.matching.unwrap();
        assert_eq!(matching.text, "Hobby");
        assert!(matching.original);
        assert_eq!(matching.removed_prefix_bytes, 2);
        assert_eq!(packed.labels[0].ordinal, Some(9));
        assert_eq!(packed.number, None);
    }

    #[cfg(feature = "scan-profile")]
    #[test]
    fn app_profile_export_drains_only_numeric_thread_samples() {
        take_scan_profile();
        {
            let _span = bitcoin_vision::timing::span("mobile.init");
        }
        let report: serde_json::Value = serde_json::from_str(&take_scan_profile()).unwrap();
        assert_eq!(report["spans"][0]["name"], "mobile.init");
        assert_eq!(report["spans"][0]["calls"], 1);
        assert!(report["ocr_widths"].as_array().unwrap().is_empty());
        let empty: serde_json::Value = serde_json::from_str(&take_scan_profile()).unwrap();
        assert!(empty["spans"].as_array().unwrap().is_empty());
    }

    /// BIP39's own published test vector, so no page's phrase is in this
    /// repository that the photographer has not cleared for publication.
    fn phrase() -> Vec<String> {
        "legal winner thank year wave sausage worth useful legal winner thank yellow"
            .split(' ')
            .map(str::to_owned)
            .collect()
    }

    fn share_of(text: &ShareText) -> Share {
        let rows = parse_rows(text.rows.iter().map(String::as_str)).unwrap();
        Share::new(ShareIndex::new(text.number).unwrap(), rows)
    }

    #[test]
    fn the_checksum_is_a_filter() {
        assert!(checksum_passes(phrase()));
        let mut wrong = phrase();
        wrong[0] = "zoo".into();
        assert!(!checksum_passes(wrong));
        assert!(!checksum_passes(phrase()[..11].to_vec()));
        let mut off_list = phrase();
        off_list[3] = "years".into();
        assert!(!checksum_passes(off_list));
    }

    #[test]
    fn the_list_is_searched_by_prefix() {
        assert_eq!(list_words("zon".into()), vec!["zone".to_owned()]);
        assert_eq!(
            list_words(" ZO".into()),
            vec!["zone".to_owned(), "zoo".to_owned()]
        );
        assert_eq!(list_words(String::new()).len(), Word::COUNT);
    }

    #[test]
    fn any_two_shares_of_a_split_recover_the_phrase() {
        let shares = split_phrase(phrase()).unwrap();
        assert_eq!(
            shares.iter().map(|s| s.number).collect::<Vec<_>>(),
            [1, 2, 3]
        );
        assert_eq!(shares[0].rows.len(), 12);
        assert_eq!(shares[0].rows[0].len(), 7);
        assert_eq!(shares[0].rows[0].as_bytes()[2], b' ');
        let parsed: Vec<Share> = shares.iter().map(share_of).collect();
        for (a, b) in [(0, 1), (1, 2), (0, 2)] {
            let recovered = penlock::recover(&parsed[a], &parsed[b]).unwrap();
            assert_eq!(recovered.phrase().split(' ').collect::<Vec<_>>(), phrase());
        }
        let again = split_phrase(phrase()).unwrap();
        assert_ne!(again[0].rows, shares[0].rows);
    }

    #[test]
    fn a_phrase_failing_the_checksum_is_not_split() {
        let mut wrong = phrase();
        wrong[0] = "zoo".into();
        assert!(matches!(
            split_phrase(wrong),
            Err(VisionError::Phrase { .. })
        ));
        let mut off_list = phrase();
        off_list[0] = "legals".into();
        assert!(matches!(
            split_phrase(off_list),
            Err(VisionError::Phrase { .. })
        ));
    }
}

/// Twelve new words from the OS random generator, made the way bdk
/// makes a wallet's mnemonic.
#[uniffi::export]
pub fn new_phrase() -> Result<Vec<String>, VisionError> {
    let generated: GeneratedKey<Mnemonic, bdk_wallet::miniscript::Tap> =
        Mnemonic::generate((WordCount::Words12, Language::English)).map_err(|e| {
            VisionError::Phrase {
                detail: e.map_or_else(|| "no entropy".to_owned(), |e| e.to_string()),
            }
        })?;
    Ok(generated.words().map(str::to_owned).collect())
}

/// A wallet as it is saved and exported: public data only.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct WalletDescriptor {
    /// The master key fingerprint, eight hex digits.
    pub fingerprint: String,
    /// The BIP86 multipath descriptor with its checksum, as Sparrow
    /// imports it.
    pub descriptor: String,
}

/// The BIP86 wallet of `words` with no passphrase, derived the way
/// bdk's template does. Refuses a phrase that fails the checksum.
#[uniffi::export]
pub fn descriptor(words: Vec<String>) -> Result<WalletDescriptor, VisionError> {
    let phrase = |detail: String| VisionError::Phrase { detail };
    let words = parse_words(&words)?;
    let seed = penlock::oracle::seed(&words, "")
        .ok_or_else(|| phrase("the phrase does not pass the BIP39 checksum".into()))?;
    let master = Xpriv::new_master(Network::Bitcoin, &seed).map_err(|e| phrase(e.to_string()))?;
    let fingerprint = master.fingerprint(&Secp256k1::new()).to_string();
    let (external, _, _) = Bip86(master, KeychainKind::External)
        .build(NetworkKind::Main)
        .map_err(|e| phrase(e.to_string()))?;
    // bdk's template is one chain at a time; Sparrow takes both chains
    // in one descriptor, so the external one is rewritten multipath
    // and reparsed for its own checksum.
    let single = external.to_string();
    let bare = single.split('#').next().unwrap_or(&single);
    let multipath: Descriptor<DescriptorPublicKey> = bare
        .replacen("/0/*)", "/<0;1>/*)", 1)
        .parse()
        .map_err(|e: bdk_wallet::miniscript::Error| phrase(e.to_string()))?;
    Ok(WalletDescriptor {
        fingerprint,
        descriptor: multipath.to_string(),
    })
}
