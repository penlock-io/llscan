//! Penlock v1 in software: the arithmetic behind the paper wheel that splits a
//! BIP39 seed phrase into a 2-of-3 backup, and the per-word checksum used to
//! catch mistakes on recovery. See `docs/penlock/` in the repository for the
//! upstream guides this reproduces.
//!
//! Positions are zero-based throughout: `word_index` counts words of the
//! phrase from 0, `position` counts a word's six worksheet symbols from 0
//! (the two checksum symbols first). Share numbers ([`ShareIndex`]) are the
//! 1, 2, 3 written on the worksheet.

#![deny(missing_docs)]

pub mod backup;
pub mod field;
pub mod oracle;
pub mod share;
pub mod soft;
pub mod wheel;
pub mod word;

pub use backup::{
    Correction, Mismatch, RecoverError, Recovery, ShareCount, ShareError, VerifyError,
    decode_words, recover, recover_symbols, resolve, split, verify,
};
pub use field::Symbol;
pub use share::{ParseShareError, Share, ShareIndex};
pub use soft::search::{
    Bip39Checksum, Class, ClassOutcome, Enumeration, Filter, Floor, Known, Merged, Oracle, Outcome,
    RankBounds, RankError, RankingError, SlotRanking, U192, cheapest_classes, classes, rank_bounds,
    search, search_classes,
};
pub use soft::{
    Distribution, Evidence, SoftCandidate, SoftDecoded, SoftRecoverError, SoftRecovery, combine,
    decode_soft, recover_soft, resolve_soft,
};
pub use wheel::Window;
pub use word::{Candidate, Decoded, UnknownWord, Word};
