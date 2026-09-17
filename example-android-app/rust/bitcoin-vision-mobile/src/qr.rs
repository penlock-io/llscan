//! QR transport for PSBTs and the descriptor: animated `ur:crypto-psbt`
//! in and out, and any text as a module grid for the app to draw.
//! Transport only — decoded bytes go to the same review the file path
//! uses; nothing here reads a wallet or signs.
//!
//! Wire contract (BCR-2020-005): the UR message is one untagged,
//! definite-length CBOR byte string holding the whole PSBT. Tag 310
//! names a `crypto-psbt` only when it is embedded in another structure,
//! never at the top level of a UR.

use std::sync::{Arc, Mutex};

use bdk_wallet::bitcoin::base64::Engine;
use bdk_wallet::bitcoin::base64::prelude::BASE64_STANDARD;
use minicbor::data::Type;

use crate::VisionError;

const PSBT_MAGIC: &[u8; 5] = b"psbt\xff";
/// Sparrow's canonical type; `bytes` is accepted from older senders.
const PSBT_TYPE: &str = "crypto-psbt";
const MAX_MESSAGE: usize = 1 << 20;
const MAX_FRAGMENTS: usize = 4096;
const MAX_FRAGMENT: usize = 4096;
const MAX_SIDE: usize = 4096;
/// The largest QR (version 40, byte mode) holds 2953 bytes; a payload
/// past this never came from a code and is not decoded further.
const MAX_PAYLOAD: usize = 8192;
/// A fountain stream needs more parts than its nominal count; this ratio
/// makes the ring move honestly and is a display heuristic, not a bound.
const EXPECTED_OVERHEAD: f32 = 1.75;

/// What one fed frame or payload amounts to.
#[derive(Clone, Debug, PartialEq, uniffi::Enum)]
pub enum QrScan {
    /// An animated code is still being collected.
    Progress {
        /// Distinct parts received so far.
        parts_seen: u32,
        /// Parts the sender announced; zero until the first part.
        parts_expected: u32,
        /// `parts_seen / (parts_expected × 1.75)`, capped below one.
        fraction: f32,
    },
    /// A whole PSBT, unvalidated beyond its magic; hand it to the review.
    Decoded {
        /// The PSBT bytes.
        psbt: Vec<u8>,
    },
    /// The code in view cannot become a PSBT; keep scanning.
    Rejected {
        /// Why, naming what was seen.
        reason: String,
    },
}

fn reject(reason: impl Into<String>) -> QrScan {
    QrScan::Rejected {
        reason: reason.into(),
    }
}

/// Collects QR payloads until a PSBT emerges.
#[derive(uniffi::Object)]
pub struct PsbtQrDecoder {
    stream: Mutex<Stream>,
}

#[uniffi::export]
impl PsbtQrDecoder {
    /// A decoder with no stream in progress.
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            stream: Mutex::new(Stream::default()),
        })
    }

    /// Reads every QR in one luma plane. `row_stride` is the byte
    /// distance between rows and may exceed `width`; the buffer must
    /// cover `row_stride × (height − 1) + width` bytes. A frame with no
    /// code reports the stream's current progress.
    pub fn feed_luma(&self, width: u32, height: u32, row_stride: u32, luma: Vec<u8>) -> QrScan {
        let payloads = match qr_payloads(width, height, row_stride, &luma) {
            Ok(payloads) => payloads,
            Err(reason) => return reject(reason),
        };
        let mut stream = self.stream.lock().unwrap();
        let mut last = stream.progress();
        for payload in payloads {
            last = stream.ingest(&payload);
            if matches!(last, QrScan::Decoded { .. }) {
                *stream = Stream::default();
                break;
            }
        }
        last
    }

    /// The raw bytes one QR carried, for callers with their own reader.
    pub fn feed_payload(&self, payload: Vec<u8>) -> QrScan {
        let mut stream = self.stream.lock().unwrap();
        let scan = stream.ingest(&payload);
        if matches!(scan, QrScan::Decoded { .. }) {
            *stream = Stream::default();
        }
        scan
    }

    /// Forgets the stream in progress.
    pub fn reset(&self) {
        *self.stream.lock().unwrap() = Stream::default();
    }
}

/// One animated code being collected. The fountain decoder is driven
/// directly so that its acceptance verdict, not the part's sequence
/// number, is what counts as progress, and so the announced checksum is
/// kept: the pinned decoder joins fragments without verifying it.
#[derive(Default)]
struct Stream {
    fountain: ur::fountain::Decoder,
    expected: usize,
    checksum: u32,
    useful: usize,
}

impl Stream {
    fn progress(&self) -> QrScan {
        let fraction = if self.expected == 0 {
            0.0
        } else {
            (self.useful as f32 / (self.expected as f32 * EXPECTED_OVERHEAD)).min(0.99)
        };
        QrScan::Progress {
            parts_seen: self.useful as u32,
            parts_expected: self.expected as u32,
            fraction,
        }
    }

    fn ingest(&mut self, payload: &[u8]) -> QrScan {
        if payload.len() > MAX_PAYLOAD {
            return reject(format!(
                "QR payload is {} bytes; at most {MAX_PAYLOAD} are read",
                payload.len()
            ));
        }
        if payload.starts_with(PSBT_MAGIC) {
            return bounded_psbt(payload);
        }
        let text = match std::str::from_utf8(payload) {
            Ok(text) => text.trim(),
            Err(_) => return reject("QR holds binary data that is not a PSBT"),
        };
        if text
            .get(..3)
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case("ur:"))
        {
            return self.ingest_ur(&text.to_ascii_lowercase());
        }
        match BASE64_STANDARD.decode(text) {
            Ok(bytes) if bytes.starts_with(PSBT_MAGIC) => bounded_psbt(&bytes),
            _ => reject("QR text is not a PSBT"),
        }
    }

    fn ingest_ur(&mut self, ur: &str) -> QrScan {
        let ur_type = ur[3..].split('/').next().unwrap_or("");
        if ur_type != PSBT_TYPE && ur_type != "bytes" {
            return reject(format!("QR is a ur:{ur_type}, not a PSBT"));
        }
        let (kind, body) = match ur::decode(ur) {
            Ok(decoded) => decoded,
            Err(e) => return reject(format!("QR is not a well-formed UR: {e}")),
        };
        if kind == ur::ur::Kind::SinglePart {
            return psbt_from_message(&body);
        }
        let header = match PartHeader::parse(&body) {
            Ok(header) => header,
            Err(reason) => return reject(reason),
        };
        let part: ur::fountain::Part = match minicbor::decode(&body) {
            Ok(part) => part,
            Err(e) => return reject(format!("QR part could not be read: {e}")),
        };
        let useful = match self.fountain.receive(part) {
            Ok(useful) => useful,
            Err(ur::fountain::Error::InconsistentPart) => {
                return reject("another animated code is in view; show only one");
            }
            Err(e) => return reject(format!("QR part could not be read: {e}")),
        };
        if self.expected == 0 {
            self.expected = header.sequence_count;
            self.checksum = header.checksum;
        }
        if useful {
            self.useful += 1;
        }
        if !self.fountain.complete() {
            return self.progress();
        }
        let scan = match self.fountain.message() {
            Ok(Some(message)) if crc32(&message) != self.checksum => {
                reject("animated code failed its checksum")
            }
            Ok(Some(message)) => psbt_from_message(&message),
            Ok(None) => reject("animated code finished without a message"),
            Err(e) => reject(format!("animated code could not be assembled: {e}")),
        };
        // Whatever it was, this stream is spent; the next code starts clean.
        *self = Stream::default();
        scan
    }
}

/// The checksum a UR fountain part announces: CRC-32 (ISO-HDLC) over the
/// whole CBOR message.
fn crc32(message: &[u8]) -> u32 {
    crc::Crc::<u32>::new(&crc::CRC_32_ISO_HDLC).checksum(message)
}

/// The metadata of a fountain part, read before the part is admitted so
/// nothing is reserved for a stream that lies about its size.
struct PartHeader {
    sequence_count: usize,
    checksum: u32,
}

impl PartHeader {
    fn parse(cbor: &[u8]) -> Result<Self, String> {
        let mut d = minicbor::Decoder::new(cbor);
        let items = d
            .array()
            .map_err(|e| format!("QR part is not a fountain part: {e}"))?;
        if items != Some(5) {
            return Err("QR part is not a five-item fountain part".into());
        }
        let field = |d: &mut minicbor::Decoder, what: &str| {
            d.u32()
                .map_err(|e| format!("QR part {what} is unreadable: {e}"))
        };
        let sequence = field(&mut d, "sequence")?;
        let sequence_count = field(&mut d, "count")? as usize;
        let message_length = field(&mut d, "length")? as usize;
        let checksum = field(&mut d, "checksum")?;
        let fragment = d
            .bytes()
            .map_err(|e| format!("QR part data is unreadable: {e}"))?;
        if d.position() != cbor.len() {
            return Err("QR part carries extra items after its data".into());
        }
        if sequence == 0 || sequence_count == 0 || message_length == 0 || fragment.is_empty() {
            return Err("QR part announces an empty stream".into());
        }
        if sequence_count > MAX_FRAGMENTS {
            return Err(format!(
                "animated code has {sequence_count} parts; at most {MAX_FRAGMENTS} are read"
            ));
        }
        if message_length > MAX_MESSAGE {
            return Err(format!(
                "animated code carries {message_length} bytes; at most {MAX_MESSAGE} are read"
            ));
        }
        if fragment.len() > MAX_FRAGMENT {
            return Err(format!(
                "QR part holds {} bytes; at most {MAX_FRAGMENT} are read",
                fragment.len()
            ));
        }
        let covered = fragment.len().checked_mul(sequence_count).unwrap_or(0);
        if covered < message_length {
            return Err("QR part count and size cannot hold the announced message".into());
        }
        Ok(Self {
            sequence_count,
            checksum,
        })
    }
}

/// The PSBT inside a UR message, or why the message is not one.
fn psbt_from_message(cbor: &[u8]) -> QrScan {
    match message_bytes(cbor) {
        Ok(psbt) => bounded_psbt(psbt),
        Err(reason) => reject(reason),
    }
}

fn message_bytes(cbor: &[u8]) -> Result<&[u8], String> {
    let mut d = minicbor::Decoder::new(cbor);
    match d.datatype() {
        Ok(Type::Bytes) => {}
        Ok(Type::BytesIndef) => return Err("UR message is an indefinite-length byte string".into()),
        Ok(Type::Tag) => return Err("UR message is tagged; a top-level PSBT must not be".into()),
        Ok(other) => return Err(format!("UR message is CBOR {other}, not a byte string")),
        Err(e) => return Err(format!("UR message is not CBOR: {e}")),
    }
    let bytes = d
        .bytes()
        .map_err(|e| format!("UR message is not CBOR: {e}"))?;
    if d.position() != cbor.len() {
        return Err("UR message carries extra items after the PSBT".into());
    }
    if !bytes.starts_with(PSBT_MAGIC) {
        return Err("UR message is not a PSBT".into());
    }
    Ok(bytes)
}

fn bounded_psbt(psbt: &[u8]) -> QrScan {
    if psbt.len() > MAX_MESSAGE {
        return reject(format!(
            "PSBT is {} bytes; at most {MAX_MESSAGE} are read",
            psbt.len()
        ));
    }
    QrScan::Decoded {
        psbt: psbt.to_vec(),
    }
}

/// Every QR payload in one luma plane, in detection order.
fn qr_payloads(
    width: u32,
    height: u32,
    row_stride: u32,
    luma: &[u8],
) -> Result<Vec<Vec<u8>>, String> {
    let (w, h, stride) = (width as usize, height as usize, row_stride as usize);
    if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE {
        return Err(format!(
            "frame {width}×{height} is outside the accepted size"
        ));
    }
    if stride < w {
        return Err(format!(
            "frame row stride {row_stride} is narrower than its width {width}"
        ));
    }
    let needed = stride
        .checked_mul(h - 1)
        .and_then(|n| n.checked_add(w))
        .ok_or_else(|| "frame geometry overflows".to_string())?;
    if luma.len() < needed {
        return Err(format!(
            "frame buffer holds {} bytes; {needed} needed",
            luma.len()
        ));
    }
    let mut image = rqrr::PreparedImage::prepare_from_greyscale(w, h, |x, y| luma[y * stride + x]);
    let mut payloads = Vec::new();
    for grid in image.detect_grids() {
        let mut bytes = Vec::new();
        if grid.decode_to(&mut bytes).is_ok() {
            payloads.push(bytes);
        }
    }
    Ok(payloads)
}

/// Emits a PSBT as `ur:crypto-psbt` parts, one per animation frame.
#[derive(uniffi::Object)]
pub struct PsbtQrEncoder {
    parts: Mutex<Parts>,
    count: u32,
}

enum Parts {
    Single(String),
    Fountain(ur::Encoder<'static>),
}

/// The encoder for `psbt`, each part at most `max_fragment_len` bytes of
/// payload. A PSBT that fits one fragment becomes a single-part UR.
#[uniffi::export]
pub fn psbt_qr_parts(
    psbt: Vec<u8>,
    max_fragment_len: u32,
) -> Result<Arc<PsbtQrEncoder>, VisionError> {
    let fail = |detail: String| VisionError::Qr { detail };
    if !psbt.starts_with(PSBT_MAGIC) {
        return Err(fail("not a PSBT".into()));
    }
    let message =
        minicbor::to_vec(minicbor::bytes::ByteVec::from(psbt)).map_err(|e| fail(e.to_string()))?;
    let encoder = ur::Encoder::new(&message, max_fragment_len as usize, PSBT_TYPE)
        .map_err(|e| fail(e.to_string()))?;
    let count = encoder.fragment_count();
    let parts = if count == 1 {
        Parts::Single(ur::encode(&message, &ur::Type::Custom(PSBT_TYPE)))
    } else {
        Parts::Fountain(encoder)
    };
    Ok(Arc::new(PsbtQrEncoder {
        parts: Mutex::new(parts),
        count: count as u32,
    }))
}

#[uniffi::export]
impl PsbtQrEncoder {
    /// The next frame's text. UR is case-insensitive; upper case lets the
    /// QR use its alphanumeric mode, which holds far more per module.
    pub fn next_part(&self) -> String {
        let part = match &mut *self.parts.lock().unwrap() {
            Parts::Single(ur) => ur.clone(),
            // Encoder::new validated the message; serialising a part of it cannot fail.
            Parts::Fountain(encoder) => encoder
                .next_part()
                .expect("fountain parts always serialise"),
        };
        part.to_ascii_uppercase()
    }

    /// Nominal parts in the message; a fountain stream keeps going past it.
    pub fn part_count(&self) -> u32 {
        self.count
    }
}

/// A QR code as a square of modules, row-major, `true` where dark.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct QrModules {
    /// Modules per side, without a quiet zone.
    pub size: u32,
    /// `size × size` entries.
    pub dark: Vec<bool>,
}

/// `text` as a QR at error-correction level L (the most capacity per frame).
#[uniffi::export]
pub fn qr_modules(text: String) -> Result<QrModules, VisionError> {
    let code = qrcode::QrCode::with_error_correction_level(text.as_bytes(), qrcode::EcLevel::L)
        .map_err(|e| VisionError::Qr {
            detail: e.to_string(),
        })?;
    Ok(QrModules {
        size: code.width() as u32,
        dark: code
            .to_colors()
            .into_iter()
            .map(|c| c == qrcode::Color::Dark)
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    // Hummingbird (Sparrow's UR library), src/test/java/com/sparrowwallet/hummingbird/registry/CryptoPSBTTest.java
    const HUMMINGBIRD_PSBT: &str = "70736274ff01009a020000000258e87a21b56daf0c23be8e7070456c336f7cbaa5c8757924f545887bb2abdd750000000000ffffffff838d0427d0ec650a68aa46bb0b098aea4422c071b2ca78352a077959d07cea1d0100000000ffffffff0270aaf00800000000160014d85c2b71d0060b09c9886aeb815e50991dda124d00e1f5050000000016001400aea9a2e5f0f876a588df5546e8742d1d87008f000000000000000000";
    const HUMMINGBIRD_CBOR: &str = "58a770736274ff01009a020000000258e87a21b56daf0c23be8e7070456c336f7cbaa5c8757924f545887bb2abdd750000000000ffffffff838d0427d0ec650a68aa46bb0b098aea4422c071b2ca78352a077959d07cea1d0100000000ffffffff0270aaf00800000000160014d85c2b71d0060b09c9886aeb815e50991dda124d00e1f5050000000016001400aea9a2e5f0f876a588df5546e8742d1d87008f000000000000000000";
    const HUMMINGBIRD_UR: &str = "ur:crypto-psbt/hdosjojkidjyzmadaenyaoaeaeaeaohdvsknclrejnpebncnrnmnjojofejzeojlkerdonspkpkkdkykfelokgprpyutkpaeaeaeaeaezmzmzmzmlslgaaditiwpihbkispkfgrkbdaslewdfycprtjsprsgksecdratkkhktikewdcaadaeaeaeaezmzmzmzmaojopkwtayaeaeaeaecmaebbtphhdnjstiambdassoloimwmlyhygdnlcatnbggtaevyykahaeaeaeaecmaebbaeplptoevwwtyakoonlourgofgvsjydpcaltaemyaeaeaeaeaeaeaeaeaebkgdcarh";

    // BCR-2020-005's fountain vector: make_message("Wolf", 256) wrapped as a
    // CBOR byte string, split into nine 30-byte fragments; as reproduced in
    // ur-rs (src/ur.rs test_ur_encoder). The message's first bytes are the
    // spec's partition vector (src/fountain.rs test_partition_and_join).
    const BCR_PARTS: [&str; 9] = [
        "ur:bytes/1-9/lpadascfadaxcywenbpljkhdcahkadaemejtswhhylkepmykhhtsytsnoyoyaxaedsuttydmmhhpktpmsrjtdkgslpgh",
        "ur:bytes/2-9/lpaoascfadaxcywenbpljkhdcagwdpfnsboxgwlbaawzuefywkdplrsrjynbvygabwjldapfcsgmghhkhstlrdcxaefz",
        "ur:bytes/3-9/lpaxascfadaxcywenbpljkhdcahelbknlkuejnbadmssfhfrdpsbiegecpasvssovlgeykssjykklronvsjksopdzmol",
        "ur:bytes/4-9/lpaaascfadaxcywenbpljkhdcasotkhemthydawydtaxneurlkosgwcekonertkbrlwmplssjtammdplolsbrdzcrtas",
        "ur:bytes/5-9/lpahascfadaxcywenbpljkhdcatbbdfmssrkzmcwnezelennjpfzbgmuktrhtejscktelgfpdlrkfyfwdajldejokbwf",
        "ur:bytes/6-9/lpamascfadaxcywenbpljkhdcackjlhkhybssklbwefectpfnbbectrljectpavyrolkzczcpkmwidmwoxkilghdsowp",
        "ur:bytes/7-9/lpatascfadaxcywenbpljkhdcavszmwnjkwtclrtvaynhpahrtoxmwvwatmedibkaegdosftvandiodagdhthtrlnnhy",
        "ur:bytes/8-9/lpayascfadaxcywenbpljkhdcadmsponkkbbhgsoltjntegepmttmoonftnbuoiyrehfrtsabzsttorodklubbuyaetk",
        "ur:bytes/9-9/lpasascfadaxcywenbpljkhdcajskecpmdckihdyhphfotjojtfmlnwmadspaxrkytbztpbauotbgtgtaeaevtgavtny",
    ];
    /// The tenth part of the same vector: a fountain part that mixes the
    /// same fragments as part 1, so it teaches the decoder nothing.
    const BCR_PART_10: &str = "ur:bytes/10-9/lpbkascfadaxcywenbpljkhdcahkadaemejtswhhylkepmykhhtsytsnoyoyaxaedsuttydmmhhpktpmsrjtwdkiplzs";
    const BCR_MESSAGE_HEAD: &str = "916ec65cf77cadf55cd7f9cda1a1030026ddd42e905b77adc36e4f2d3ccba44f7f04f2de44f42d84c374a0e149136f25b01852545961d55f7f7a8cde6d0e2ec43f3b2dcb644a2209e8c9e34af5c4747984a5e873c9cf5f965e25ee29039f";

    fn decoded(scan: QrScan) -> Vec<u8> {
        match scan {
            QrScan::Decoded { psbt } => psbt,
            other => panic!("expected a PSBT, got {other:?}"),
        }
    }

    fn reason(scan: QrScan) -> String {
        match scan {
            QrScan::Rejected { reason } => reason,
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    fn seen(scan: &QrScan) -> (u32, u32) {
        match scan {
            QrScan::Progress {
                parts_seen,
                parts_expected,
                ..
            } => (*parts_seen, *parts_expected),
            other => panic!("expected progress, got {other:?}"),
        }
    }

    /// A PSBT-shaped payload of `len` bytes for transport tests.
    fn transport_psbt(len: usize) -> Vec<u8> {
        let mut out = PSBT_MAGIC.to_vec();
        let mut x = 0x2545_f491_4f6c_dd1du64;
        while out.len() < len {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            out.push(x as u8);
        }
        out
    }

    #[test]
    fn hummingbird_single_part_decodes_to_the_psbt() {
        let decoder = PsbtQrDecoder::new();
        assert_eq!(
            decoded(decoder.feed_payload(HUMMINGBIRD_UR.as_bytes().to_vec())),
            hex(HUMMINGBIRD_PSBT)
        );
        assert_eq!(
            decoded(decoder.feed_payload(HUMMINGBIRD_UR.to_ascii_uppercase().into_bytes())),
            hex(HUMMINGBIRD_PSBT)
        );
        assert_eq!(
            message_bytes(&hex(HUMMINGBIRD_CBOR)).unwrap(),
            &hex(HUMMINGBIRD_PSBT)[..]
        );
    }

    #[test]
    fn the_psbt_encodes_to_hummingbirds_ur_when_it_fits_one_part() {
        let encoder = psbt_qr_parts(hex(HUMMINGBIRD_PSBT), 400).unwrap();
        assert_eq!(encoder.part_count(), 1);
        assert_eq!(encoder.next_part(), HUMMINGBIRD_UR.to_ascii_uppercase());
        assert_eq!(encoder.next_part(), HUMMINGBIRD_UR.to_ascii_uppercase());
    }

    #[test]
    fn bcr_multipart_bytes_assemble_and_are_refused_as_a_psbt() {
        let mut spec = ur::Decoder::default();
        for part in BCR_PARTS {
            spec.receive(part).unwrap();
        }
        let message = spec.message().unwrap().unwrap();
        let mut d = minicbor::Decoder::new(&message);
        let body = d.bytes().unwrap();
        assert_eq!(body.len(), 256);
        let head = hex(BCR_MESSAGE_HEAD);
        assert_eq!(&body[..head.len()], &head[..]);
        assert_eq!(
            message_bytes(&message).unwrap_err(),
            "UR message is not a PSBT"
        );
        let (_, first) = ur::decode(BCR_PARTS[0]).unwrap();
        assert_eq!(
            PartHeader::parse(&first).unwrap().checksum,
            crc32(&message),
            "the vector's checksum is the message CRC-32"
        );

        let decoder = PsbtQrDecoder::new();
        assert_eq!(
            seen(&decoder.feed_payload(BCR_PARTS[0].as_bytes().to_vec())),
            (1, 9)
        );
        let same_equation = decoder.feed_payload(BCR_PART_10.as_bytes().to_vec());
        assert_eq!(
            seen(&same_equation),
            (1, 9),
            "a new sequence number carrying a known equation is not progress"
        );
        for (i, part) in BCR_PARTS[1..8].iter().enumerate() {
            let scan = decoder.feed_payload(part.as_bytes().to_vec());
            assert_eq!(seen(&scan), (i as u32 + 2, 9));
        }
        let again = decoder.feed_payload(BCR_PARTS[3].as_bytes().to_vec());
        assert_eq!(seen(&again), (8, 9), "a repeated part is not progress");
        assert_eq!(
            reason(decoder.feed_payload(BCR_PARTS[8].as_bytes().to_vec())),
            "UR message is not a PSBT"
        );
        assert_eq!(
            seen(&decoder.progress_for_test()),
            (0, 0),
            "the spent stream is forgotten"
        );
    }

    #[test]
    fn an_animated_psbt_survives_shuffled_dropped_and_repeated_parts() {
        let psbt = transport_psbt(3000);
        let encoder = psbt_qr_parts(psbt.clone(), 100).unwrap();
        assert_eq!(encoder.part_count(), 31);
        let decoder = PsbtQrDecoder::new();
        let mut frames = 0;
        loop {
            let part = encoder.next_part();
            frames += 1;
            assert!(frames < 200, "fountain never completed");
            if frames % 3 == 0 {
                continue;
            }
            let scan = decoder.feed_payload(part.clone().into_bytes());
            if frames % 5 == 0 {
                let repeat = decoder.feed_payload(part.into_bytes());
                if let QrScan::Progress { .. } = scan {
                    assert_eq!(
                        seen(&repeat),
                        seen(&scan),
                        "a repeated frame is not progress"
                    );
                }
            }
            match scan {
                QrScan::Progress {
                    parts_expected,
                    fraction,
                    ..
                } => {
                    assert_eq!(parts_expected, 31);
                    assert!(fraction < 1.0);
                }
                QrScan::Decoded { psbt: got } => {
                    assert_eq!(got, psbt);
                    break;
                }
                QrScan::Rejected { reason } => panic!("{reason}"),
            }
        }
        assert!(frames > 31, "dropped parts were made up by fountain parts");
    }

    #[test]
    fn a_bytes_ur_and_bare_forms_are_accepted() {
        let psbt = hex(HUMMINGBIRD_PSBT);
        let message = minicbor::to_vec(minicbor::bytes::ByteVec::from(psbt.clone())).unwrap();
        let decoder = PsbtQrDecoder::new();
        assert_eq!(
            decoded(decoder.feed_payload(ur::encode(&message, &ur::Type::Bytes).into_bytes())),
            psbt
        );
        assert_eq!(decoded(decoder.feed_payload(psbt.clone())), psbt);
        assert_eq!(
            decoded(decoder.feed_payload(BASE64_STANDARD.encode(&psbt).into_bytes())),
            psbt
        );
    }

    #[test]
    fn other_codes_are_refused_by_name() {
        let decoder = PsbtQrDecoder::new();
        let seed = ur::encode(b"\x45hello", &ur::Type::Custom("crypto-seed"));
        assert_eq!(
            reason(decoder.feed_payload(seed.into_bytes())),
            "QR is a ur:crypto-seed, not a PSBT"
        );
        assert_eq!(
            reason(decoder.feed_payload(b"bitcoin:bc1qxyz".to_vec())),
            "QR text is not a PSBT"
        );
        assert_eq!(
            reason(decoder.feed_payload(vec![0xff, 0xfe, 0x00])),
            "QR holds binary data that is not a PSBT"
        );

        let tagged = [&[0xd9, 0x01, 0x36][..], &hex(HUMMINGBIRD_CBOR)].concat();
        let tagged_ur = ur::encode(&tagged, &ur::Type::Custom(PSBT_TYPE));
        assert_eq!(
            reason(decoder.feed_payload(tagged_ur.into_bytes())),
            "UR message is tagged; a top-level PSBT must not be"
        );

        let trailing = [&hex(HUMMINGBIRD_CBOR)[..], &[0x00]].concat();
        let trailing_ur = ur::encode(&trailing, &ur::Type::Custom(PSBT_TYPE));
        assert_eq!(
            reason(decoder.feed_payload(trailing_ur.into_bytes())),
            "UR message carries extra items after the PSBT"
        );

        let text = ur::encode(
            &minicbor::to_vec("psbt").unwrap(),
            &ur::Type::Custom(PSBT_TYPE),
        );
        assert_eq!(
            reason(decoder.feed_payload(text.into_bytes())),
            "UR message is CBOR string, not a byte string"
        );
    }

    fn part_ur(sequence: u32, count: u32, length: u32, checksum: u32, data: &[u8]) -> String {
        let mut e = minicbor::Encoder::new(Vec::new());
        e.array(5)
            .unwrap()
            .u32(sequence)
            .unwrap()
            .u32(count)
            .unwrap()
            .u32(length)
            .unwrap()
            .u32(checksum)
            .unwrap()
            .bytes(data)
            .unwrap();
        let body = ur::bytewords::encode(&e.into_writer(), ur::bytewords::Style::Minimal);
        format!("ur:{PSBT_TYPE}/{sequence}-{count}/{body}")
    }

    #[test]
    fn a_wrong_checksum_rejects_the_stream_and_the_next_one_still_decodes() {
        let message = hex(HUMMINGBIRD_CBOR);
        let decoder = PsbtQrDecoder::new();
        let lying = part_ur(1, 1, message.len() as u32, crc32(&message) ^ 1, &message);
        assert_eq!(
            reason(decoder.feed_payload(lying.into_bytes())),
            "animated code failed its checksum"
        );
        assert_eq!(
            seen(&decoder.progress_for_test()),
            (0, 0),
            "the failed stream is forgotten"
        );
        let honest = part_ur(1, 1, message.len() as u32, crc32(&message), &message);
        assert_eq!(
            decoded(decoder.feed_payload(honest.into_bytes())),
            hex(HUMMINGBIRD_PSBT)
        );
    }

    #[test]
    fn oversize_payloads_are_refused_before_decoding() {
        let decoder = PsbtQrDecoder::new();
        let huge = format!("ur:{PSBT_TYPE}/{}", "a".repeat(MAX_PAYLOAD));
        assert_eq!(
            reason(decoder.feed_payload(huge.into_bytes())),
            format!(
                "QR payload is {} bytes; at most {MAX_PAYLOAD} are read",
                MAX_PAYLOAD + 15
            )
        );
    }

    #[test]
    fn a_lying_header_is_refused_before_anything_is_reserved() {
        let decoder = PsbtQrDecoder::new();
        assert_eq!(
            reason(decoder.feed_payload(part_ur(1, 2, 2 << 20, 7, &[1; 10]).into_bytes())),
            format!(
                "animated code carries {} bytes; at most {MAX_MESSAGE} are read",
                2 << 20
            )
        );
        assert_eq!(
            reason(decoder.feed_payload(part_ur(1, 5000, 100, 7, &[1; 10]).into_bytes())),
            format!("animated code has 5000 parts; at most {MAX_FRAGMENTS} are read")
        );
        assert_eq!(
            reason(decoder.feed_payload(part_ur(1, 2, 100, 7, &[1; 10]).into_bytes())),
            "QR part count and size cannot hold the announced message"
        );
        assert_eq!(
            seen(&decoder.progress_for_test()),
            (0, 0),
            "nothing was admitted"
        );
    }

    #[test]
    fn a_second_stream_in_view_is_refused_and_the_first_still_completes() {
        let first = psbt_qr_parts(transport_psbt(500), 100).unwrap();
        let second = psbt_qr_parts(transport_psbt(700), 100).unwrap();
        let decoder = PsbtQrDecoder::new();
        assert_eq!(
            seen(&decoder.feed_payload(first.next_part().into_bytes())),
            (1, 6)
        );
        assert_eq!(
            reason(decoder.feed_payload(second.next_part().into_bytes())),
            "another animated code is in view; show only one"
        );
        assert_eq!(
            seen(&decoder.progress_for_test()),
            (1, 6),
            "the first stream is untouched"
        );
        let mut scan = decoder.progress_for_test();
        while !matches!(scan, QrScan::Decoded { .. }) {
            scan = decoder.feed_payload(first.next_part().into_bytes());
        }
        assert_eq!(decoded(scan), transport_psbt(500));
    }

    /// Paints a code at `scale` px per module inside a frame whose rows are
    /// `pad` bytes longer than its width, as a camera plane's may be.
    fn luma_of(modules: &QrModules, scale: usize, pad: usize) -> (u32, u32, u32, Vec<u8>) {
        let quiet = 4;
        let side = (modules.size as usize + 2 * quiet) * scale;
        let stride = side + pad;
        let mut luma = vec![255u8; stride * side];
        for y in 0..side {
            for x in 0..side {
                let (mx, my) = (x / scale, y / scale);
                if mx >= quiet
                    && my >= quiet
                    && mx - quiet < modules.size as usize
                    && my - quiet < modules.size as usize
                {
                    let dark = modules.dark[(my - quiet) * modules.size as usize + (mx - quiet)];
                    luma[y * stride + x] = if dark { 0 } else { 255 };
                }
            }
        }
        (side as u32, side as u32, stride as u32, luma)
    }

    #[test]
    fn a_drawn_code_reads_back_through_the_luma_path() {
        let modules = qr_modules(HUMMINGBIRD_UR.to_ascii_uppercase()).unwrap();
        assert_eq!(modules.dark.len(), (modules.size * modules.size) as usize);
        let (w, h, stride, luma) = luma_of(&modules, 4, 13);
        let decoder = PsbtQrDecoder::new();
        assert_eq!(
            decoded(decoder.feed_luma(w, h, stride, luma)),
            hex(HUMMINGBIRD_PSBT)
        );

        let encoder = psbt_qr_parts(transport_psbt(800), 200).unwrap();
        let mut scan = decoder.progress_for_test();
        let mut frames = 0;
        while !matches!(scan, QrScan::Decoded { .. }) {
            frames += 1;
            assert!(frames < 40);
            let (w, h, stride, luma) = luma_of(&qr_modules(encoder.next_part()).unwrap(), 3, 0);
            scan = decoder.feed_luma(w, h, stride, luma);
        }
        assert_eq!(decoded(scan), transport_psbt(800));
    }

    #[test]
    fn frames_outside_the_accepted_geometry_are_refused() {
        let decoder = PsbtQrDecoder::new();
        assert_eq!(
            reason(decoder.feed_luma(0, 10, 10, vec![0; 100])),
            "frame 0×10 is outside the accepted size"
        );
        assert_eq!(
            reason(decoder.feed_luma(10, 10, 8, vec![0; 100])),
            "frame row stride 8 is narrower than its width 10"
        );
        assert_eq!(
            reason(decoder.feed_luma(10, 10, 16, vec![0; 100])),
            "frame buffer holds 100 bytes; 154 needed"
        );
        assert_eq!(seen(&decoder.feed_luma(10, 10, 16, vec![255; 154])), (0, 0));
    }

    #[test]
    fn the_descriptor_fits_a_static_code() {
        let descriptor = "tr([73c5da0a/86'/0'/0']xpub6BgBgsespWvERF3LHQu6CnqdvfEvtMcQjYrcRzx53QJjSxarj2afYWcLteoGVky7D3UKDP9QyrLprQ3VCECoY49yfdDEHGCtMMj92pReUsQ/0/*)#8qxtq4ss";
        let modules = qr_modules(descriptor.to_string()).unwrap();
        assert!(
            modules.size <= 65,
            "version ≤ 11 keeps the code readable on a phone screen: {}",
            modules.size
        );
        let (w, h, stride, luma) = luma_of(&modules, 4, 0);
        let mut image =
            rqrr::PreparedImage::prepare_from_greyscale(w as usize, h as usize, |x, y| {
                luma[y * stride as usize + x]
            });
        let grids = image.detect_grids();
        assert_eq!(grids.len(), 1);
        assert_eq!(grids[0].decode().unwrap().1, descriptor);
    }

    impl PsbtQrDecoder {
        fn progress_for_test(&self) -> QrScan {
            self.stream.lock().unwrap().progress()
        }
    }
}
