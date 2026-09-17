//! The 2-of-3 backup: splitting a phrase into shares, recovering it from any
//! two, and checking a hand-made split.
//!
//! Every worksheet symbol is split on its own. For secret symbol `s` and a
//! fresh random `c`, share `i` holds `c + s·i`: the shares are points on a
//! line whose slope is the secret. This is what reading three consecutive
//! wheel windows produces (see [`crate::wheel`]), and any two points give
//! the slope back as `(share_b − share_a) / (b − a)`.

use std::fmt;

use rand::RngCore;

use crate::field::Symbol;
use crate::share::{Share, ShareIndex};
use crate::word::{Decoded, WORD_LEN, Word, decode};

/// Splits `words` into `count` shares (two or three) with fresh randomness
/// for every symbol.
///
/// `rng` must be cryptographically secure (the OS RNG, or a CSPRNG seeded
/// from it): a share is only secret if its intercepts are unpredictable.
pub fn split<R: RngCore>(words: &[Word], count: ShareCount, rng: &mut R) -> Vec<Share> {
    let mut shares: Vec<Vec<[Symbol; WORD_LEN]>> =
        vec![Vec::with_capacity(words.len()); count.get()];
    for word in words {
        let mut groups = [[Symbol::ZERO; WORD_LEN]; ShareIndex::MAX as usize];
        for (position, secret) in word.symbols().into_iter().enumerate() {
            let intercept = Symbol::random(rng);
            for (group, index) in groups.iter_mut().zip(ShareIndex::all()) {
                group[position] = intercept + secret * index.symbol();
            }
        }
        for (share, group) in shares.iter_mut().zip(groups) {
            share.push(group);
        }
    }
    shares
        .into_iter()
        .zip(ShareIndex::all())
        .map(|(words, index)| Share::new(index, words))
        .collect()
}

/// How many shares to produce. The guide allows stopping at two.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ShareCount(usize);

impl ShareCount {
    /// Shares 1 and 2 only.
    pub const TWO: ShareCount = ShareCount(2);
    /// The full 2-of-3 backup.
    pub const THREE: ShareCount = ShareCount(3);

    /// `count` shares for 2 or 3; `None` otherwise.
    pub fn new(count: usize) -> Option<ShareCount> {
        (2..=usize::from(ShareIndex::MAX))
            .contains(&count)
            .then_some(ShareCount(count))
    }

    /// The count, 2 or 3.
    pub fn get(self) -> usize {
        self.0
    }
}

/// Recovers the worksheet symbols of every word from two shares, before
/// any word decoding.
pub fn recover_symbols(a: &Share, b: &Share) -> Result<Vec<[Symbol; WORD_LEN]>, ShareError> {
    check_pair(a, b)?;
    let scale = (b.index().symbol() - a.index().symbol())
        .inv()
        .expect("distinct indices differ by a non-zero symbol");
    Ok(a.words()
        .iter()
        .zip(b.words())
        .map(|(wa, wb)| {
            let mut out = [Symbol::ZERO; WORD_LEN];
            for (o, (&sa, &sb)) in out.iter_mut().zip(wa.iter().zip(wb)) {
                *o = (sb - sa) * scale;
            }
            out
        })
        .collect())
}

fn check_pair(a: &Share, b: &Share) -> Result<(), ShareError> {
    if a.index() == b.index() {
        return Err(ShareError::SameIndex(a.index()));
    }
    if a.words().len() != b.words().len() {
        return Err(ShareError::LengthMismatch {
            a: (a.index(), a.words().len()),
            b: (b.index(), b.words().len()),
        });
    }
    Ok(())
}

/// Recovers and decodes the phrase from two shares.
///
/// Succeeds only when every word decodes to exactly one wordlist word by
/// the policy in [`decode`]; `corrections` lists every symbol that was
/// changed on the way. Otherwise every undecided word is returned with its
/// candidates so the caller can ask for a decision.
pub fn recover(a: &Share, b: &Share) -> Result<Recovery, RecoverError> {
    resolve(decode_words(&recover_symbols(a, b)?))
}

/// Decodes each recovered word independently (see [`decode`]), one entry
/// per word in phrase order.
pub fn decode_words(symbols: &[[Symbol; WORD_LEN]]) -> Vec<Decoded> {
    symbols.iter().map(|&s| decode(s)).collect()
}

/// Turns per-word decodings into a phrase. A caller that has settled an
/// undecided word replaces its entry with [`Decoded::Exact`] first.
pub fn resolve(decoded: Vec<Decoded>) -> Result<Recovery, RecoverError> {
    let mut words = Vec::with_capacity(decoded.len());
    let mut corrections = Vec::new();
    let mut undecided = Vec::new();
    for (index, decoded) in decoded.into_iter().enumerate() {
        match decoded {
            Decoded::Exact(word) => words.push(word),
            Decoded::Corrected {
                word,
                position,
                from,
                to,
            } => {
                words.push(word);
                corrections.push(Correction {
                    word_index: index,
                    word,
                    position,
                    from,
                    to,
                });
            }
            other => undecided.push((index, other)),
        }
    }
    if !undecided.is_empty() {
        return Err(RecoverError::Undecodable(undecided));
    }
    let bip39_checksum_ok = bip39_checksum_ok(&words);
    Ok(Recovery {
        words,
        corrections,
        bip39_checksum_ok,
    })
}

/// Whether `words` form a valid BIP39 mnemonic (length and checksum).
pub fn bip39_checksum_ok(words: &[Word]) -> bool {
    bip39::Mnemonic::parse_in_normalized(bip39::Language::English, &phrase(words)).is_ok()
}

/// The words joined by single spaces, as a wallet expects them.
pub fn phrase(words: &[Word]) -> String {
    words
        .iter()
        .map(|w| w.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

/// A phrase recovered from two shares, with everything that was changed
/// to get there.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Recovery {
    /// The recovered words in phrase order.
    pub words: Vec<Word>,
    /// Every symbol the decoder changed, in phrase order; empty when the
    /// shares were error-free.
    pub corrections: Vec<Correction>,
    /// Whether `words` pass BIP39's own length and checksum rules. `false`
    /// means an undetected error or a phrase that was never valid.
    pub bip39_checksum_ok: bool,
}

impl Recovery {
    /// The recovered phrase as a wallet expects it.
    pub fn phrase(&self) -> String {
        phrase(&self.words)
    }
}

/// One symbol the decoder changed to reach a wordlist word.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Correction {
    /// Zero-based position of the word in the phrase.
    pub word_index: usize,
    /// The word the symbols were corrected to.
    pub word: Word,
    /// Zero-based position of the symbol within the six-symbol word.
    pub position: usize,
    /// The symbol the shares produced.
    pub from: Symbol,
    /// The symbol `word` has there.
    pub to: Symbol,
}

impl fmt::Display for Correction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "word {}: symbol {} read {} but should be {}, giving {}",
            self.word_index + 1,
            self.position + 1,
            self.from,
            self.to,
            self.word
        )
    }
}

/// Why [`recover`] produced no phrase.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RecoverError {
    /// The two shares cannot be combined at all.
    Shares(ShareError),
    /// Every word that did not decode to a single wordlist word, with its
    /// [`Decoded::Candidates`] or [`Decoded::Unrecognized`] outcome.
    Undecodable(Vec<(usize, Decoded)>),
}

impl From<ShareError> for RecoverError {
    fn from(e: ShareError) -> Self {
        RecoverError::Shares(e)
    }
}

impl fmt::Display for RecoverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Shares(e) => e.fmt(f),
            Self::Undecodable(words) => {
                write!(
                    f,
                    "{} word(s) could not be decoded unambiguously",
                    words.len()
                )
            }
        }
    }
}

impl std::error::Error for RecoverError {}

/// Why a set of shares does not belong together.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ShareError {
    /// Two shares carry the same number; recovery needs two different
    /// points on the line.
    SameIndex(ShareIndex),
    /// Two shares have different word counts, so they are not from the
    /// same split.
    LengthMismatch {
        /// One share's number and word count.
        a: (ShareIndex, usize),
        /// The other's.
        b: (ShareIndex, usize),
    },
}

impl fmt::Display for ShareError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SameIndex(i) => write!(
                f,
                "both shares are share {i}; two different ones are needed"
            ),
            Self::LengthMismatch { a, b } => write!(
                f,
                "share {} has {} words but share {} has {}",
                a.0, a.1, b.0, b.1
            ),
        }
    }
}

impl std::error::Error for ShareError {}

/// Checks hand-made shares against the phrase they were split from, the
/// way section C of the split guide does: at every position the shares
/// must lie on one line with slope equal to the phrase symbol.
///
/// A single share is always consistent, so at least two are required.
pub fn verify(words: &[Word], shares: &[Share]) -> Result<(), VerifyError> {
    let [first, rest @ ..] = shares else {
        return Err(VerifyError::TooFewShares);
    };
    if rest.is_empty() {
        return Err(VerifyError::TooFewShares);
    }
    for (i, a) in shares.iter().enumerate() {
        for b in &shares[i + 1..] {
            check_pair(a, b)?;
        }
    }
    if first.words().len() != words.len() {
        return Err(VerifyError::PhraseLengthMismatch {
            phrase: words.len(),
            shares: first.words().len(),
        });
    }

    let mut mismatches = Vec::new();
    for (word_index, word) in words.iter().enumerate() {
        for (position, secret) in word.symbols().into_iter().enumerate() {
            let found: Vec<(ShareIndex, Symbol)> = shares
                .iter()
                .map(|s| (s.index(), s.words()[word_index][position]))
                .collect();
            let mut intercepts = found
                .iter()
                .map(|(index, symbol)| *symbol - secret * index.symbol());
            let expected = intercepts.next().expect("at least two shares");
            if !intercepts.all(|c| c == expected) {
                mismatches.push(Mismatch {
                    word_index,
                    position,
                    secret,
                    found,
                });
            }
        }
    }
    if mismatches.is_empty() {
        Ok(())
    } else {
        Err(VerifyError::Inconsistent(mismatches))
    }
}

/// A position where the shares do not agree with the phrase. As on paper,
/// the fix is to regenerate that position on every share.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Mismatch {
    /// Zero-based position of the word in the phrase.
    pub word_index: usize,
    /// Zero-based position of the symbol within the six-symbol word.
    pub position: usize,
    /// What the phrase has at that position.
    pub secret: Symbol,
    /// What each share has there, in the order the shares were given.
    pub found: Vec<(ShareIndex, Symbol)>,
}

impl fmt::Display for Mismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "word {}, symbol {} ({}): shares read",
            self.word_index + 1,
            self.position + 1,
            self.secret
        )?;
        for (index, symbol) in &self.found {
            write!(f, " {index}:{symbol}")?;
        }
        Ok(())
    }
}

/// Why [`verify`] did not pass.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum VerifyError {
    /// Fewer than two shares; one share alone is consistent with anything.
    TooFewShares,
    /// The shares do not belong together.
    Shares(ShareError),
    /// The phrase and the shares have different word counts.
    PhraseLengthMismatch {
        /// Words in the phrase.
        phrase: usize,
        /// Words in each share.
        shares: usize,
    },
    /// The shares belong together but disagree with the phrase at these
    /// positions.
    Inconsistent(Vec<Mismatch>),
}

impl From<ShareError> for VerifyError {
    fn from(e: ShareError) -> Self {
        VerifyError::Shares(e)
    }
}

impl fmt::Display for VerifyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooFewShares => write!(f, "at least two shares are needed to verify"),
            Self::Shares(e) => e.fmt(f),
            Self::PhraseLengthMismatch { phrase, shares } => {
                write!(f, "phrase has {phrase} words but the shares have {shares}")
            }
            Self::Inconsistent(m) => write!(f, "{} position(s) do not match the phrase", m.len()),
        }
    }
}

impl std::error::Error for VerifyError {}
