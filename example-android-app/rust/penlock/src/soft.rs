//! Recovery from uncertain readings: every symbol is a probability
//! distribution over the 29 symbols rather than a single guess.
//!
//! Two shares' distributions are combined position by position into a
//! distribution over the recovered symbol, and each word is then decoded
//! against the wordlist by *soft distance*: a symbol is plausible at a
//! position when its probability reaches [`PLAUSIBLE`], and a word's
//! distance is the number of positions where its symbol is not plausible.
//! The acceptance rule is the hard decoder's: a word is accepted only when
//! it is the sole wordlist word within distance 2 and its own distance is
//! at most 1. Likelihood only ranks. On one-hot input every outcome here
//! corresponds exactly to [`crate::word::decode`]'s.
//!
//! What is promised: if the true word is within soft distance 2 of the
//! reading, it is among the candidates and no other word is accepted
//! automatically. Beyond radius 2 nothing is promised.

pub mod search;

use std::fmt;

use crate::backup::{ShareError, bip39_checksum_ok};
use crate::field::Symbol;
use crate::share::{Share, ShareIndex};
use crate::word::{WORD_LEN, Word};

/// Probability at or above which a symbol counts as plausible at a
/// position. Below `1/29`, so a uniform (no-evidence) cell makes nothing
/// plausible.
pub const PLAUSIBLE: f32 = 0.05;
/// Floor applied to probabilities inside the log-likelihood used for
/// ranking, so a zero never produces `-inf`.
const LIKELIHOOD_FLOOR: f64 = 1e-6;
/// How close to 1 a distribution's sum must be to be accepted.
const SUM_TOLERANCE: f32 = 1e-3;

/// A probability distribution over the 29 symbols. Only constructible
/// through [`Distribution::new`], [`Distribution::one_hot`] and
/// [`Distribution::uniform`], so it is always finite, non-negative and
/// normalised.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Distribution([f32; 29]);

/// Why an array of probabilities is not a distribution.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum InvalidDistribution {
    /// An entry is NaN or infinite.
    NotFinite,
    /// An entry is below zero.
    Negative,
    /// The entries do not sum to 1 (within tolerance); carries the sum.
    Sum(f32),
}

impl fmt::Display for InvalidDistribution {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFinite => write!(f, "probabilities must be finite"),
            Self::Negative => write!(f, "probabilities must not be negative"),
            Self::Sum(s) => write!(f, "probabilities sum to {s}, not 1"),
        }
    }
}

impl std::error::Error for InvalidDistribution {}

impl Distribution {
    /// Validates and renormalises `probabilities`, indexed by symbol value.
    pub fn new(probabilities: [f32; 29]) -> Result<Distribution, InvalidDistribution> {
        if probabilities.iter().any(|p| !p.is_finite()) {
            return Err(InvalidDistribution::NotFinite);
        }
        if probabilities.iter().any(|&p| p < 0.0) {
            return Err(InvalidDistribution::Negative);
        }
        let sum: f32 = probabilities.iter().sum();
        if (sum - 1.0).abs() > SUM_TOLERANCE {
            return Err(InvalidDistribution::Sum(sum));
        }
        Ok(Distribution(probabilities.map(|p| p / sum)))
    }

    /// All probability on `symbol`.
    pub fn one_hot(symbol: Symbol) -> Distribution {
        let mut p = [0.0; 29];
        p[usize::from(symbol.value())] = 1.0;
        Distribution(p)
    }

    /// No evidence: every symbol equally likely.
    pub fn uniform() -> Distribution {
        Distribution([1.0 / 29.0; 29])
    }

    /// Probability of `symbol`.
    pub fn p(&self, symbol: Symbol) -> f32 {
        self.0[usize::from(symbol.value())]
    }

    /// The most probable symbol; ties go to the lowest symbol value.
    pub fn argmax(&self) -> Symbol {
        let (i, _) = self
            .0
            .iter()
            .enumerate()
            .fold((0, f32::NEG_INFINITY), |best, (i, &p)| {
                if p > best.1 { (i, p) } else { best }
            });
        Symbol::new(i as u8).expect("index below 29")
    }

    /// Whether `symbol` reaches [`PLAUSIBLE`].
    pub fn plausible(&self, symbol: Symbol) -> bool {
        self.p(symbol) >= PLAUSIBLE
    }

    /// The probabilities, indexed by symbol value.
    pub fn as_array(&self) -> &[f32; 29] {
        &self.0
    }
}

/// A text share as one-hot distributions, so it can be combined with a
/// scanned one.
pub fn one_hot_share(share: &Share) -> Vec<[Distribution; WORD_LEN]> {
    share
        .words()
        .iter()
        .map(|word| word.map(Distribution::one_hot))
        .collect()
}

/// Combines one word's readings from two shares into distributions over
/// the recovered symbols: at each position, every pair of share symbols
/// `(x, y)` contributes `P_a(x)·P_b(y)` to the secret `(y − x)/(i_b − i_a)`.
/// On one-hot input this is [`crate::backup::recover_symbols`] for that
/// word.
pub fn combine(
    a: &[Distribution; WORD_LEN],
    index_a: ShareIndex,
    b: &[Distribution; WORD_LEN],
    index_b: ShareIndex,
) -> Result<[Distribution; WORD_LEN], ShareError> {
    if index_a == index_b {
        return Err(ShareError::SameIndex(index_a));
    }
    let scale = (index_b.symbol() - index_a.symbol())
        .inv()
        .expect("distinct indices differ by a non-zero symbol");
    Ok(std::array::from_fn(|position| {
        let mut q = [0.0f32; 29];
        for x in Symbol::all() {
            let px = a[position].p(x);
            if px == 0.0 {
                continue;
            }
            for y in Symbol::all() {
                let py = b[position].p(y);
                if py == 0.0 {
                    continue;
                }
                let secret = (y - x) * scale;
                q[usize::from(secret.value())] += px * py;
            }
        }
        Distribution::new(q).expect("a product of distributions is a distribution")
    }))
}

/// A wordlist word near a soft reading.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SoftCandidate {
    /// The wordlist word.
    pub word: Word,
    /// Positions where the word's symbol is not plausible, `0..=2`.
    pub distance: u8,
    /// Log-likelihood of the word under the reading, for ranking only.
    pub likelihood: f64,
}

/// What decoding did at one position of an accepted word.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Evidence {
    /// The reading's most probable symbol was overruled by the word's.
    Substituted {
        /// Zero-based position within the six-symbol word.
        position: usize,
        /// The reading's most probable symbol.
        from: Symbol,
        /// The accepted word's symbol there.
        to: Symbol,
        /// Probability the reading gave `from`.
        p_from: f32,
        /// Probability the reading gave `to`.
        p_to: f32,
    },
    /// The word's symbol was the most probable but below [`PLAUSIBLE`]:
    /// nothing was overruled, the cell was just too uncertain to count.
    LowConfidence {
        /// Zero-based position within the six-symbol word.
        position: usize,
        /// Probability the reading gave the accepted word's symbol.
        p_target: f32,
    },
}

impl Evidence {
    /// The position this evidence is about.
    pub fn position(&self) -> usize {
        match self {
            Self::Substituted { position, .. } | Self::LowConfidence { position, .. } => *position,
        }
    }
}

impl fmt::Display for Evidence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Substituted {
                position,
                from,
                to,
                p_from,
                p_to,
            } => write!(
                f,
                "symbol {} read {from} (p = {p_from:.2}) but should be {to} (p = {p_to:.2})",
                position + 1
            ),
            Self::LowConfidence { position, p_target } => write!(
                f,
                "accepted with low confidence at symbol {}: best reading at p = {p_target:.2}",
                position + 1
            ),
        }
    }
}

/// Outcome of decoding one word's recovered-symbol distributions.
///
/// On one-hot input: `Exact` ↔ [`crate::word::Decoded::Exact`];
/// `Resolved` with one `Substituted` ↔ `Decoded::Corrected`;
/// `Candidates` ↔ `Decoded::Candidates` with the same words and distances;
/// `Unrecognized` ↔ `Unrecognized`. `LowConfidence` cannot occur on
/// one-hot input.
#[derive(Clone, PartialEq, Debug)]
pub enum SoftDecoded {
    /// Accepted, and the word agrees with the most probable symbol at
    /// every position with each above [`PLAUSIBLE`].
    Exact {
        /// The word.
        word: Word,
        /// Its log-likelihood.
        likelihood: f64,
    },
    /// Accepted with at least one inference, every one of which is listed.
    Resolved {
        /// The word.
        word: Word,
        /// What was overruled or accepted on weak evidence, by position.
        evidence: Vec<Evidence>,
        /// Its log-likelihood.
        likelihood: f64,
    },
    /// More than one wordlist word within soft distance 2, or a lone one
    /// at distance 2: the caller decides. Nearest and most likely first;
    /// never empty.
    Candidates(Vec<SoftCandidate>),
    /// No wordlist word within soft distance 2.
    Unrecognized,
}

impl SoftDecoded {
    /// The accepted word, if any.
    pub fn accepted(&self) -> Option<Word> {
        match self {
            Self::Exact { word, .. } | Self::Resolved { word, .. } => Some(*word),
            _ => None,
        }
    }
}

fn likelihood(q: &[Distribution; WORD_LEN], word: Word) -> f64 {
    word.symbols()
        .iter()
        .zip(q)
        .map(|(&s, d)| f64::from(d.p(s)).max(LIKELIHOOD_FLOOR).ln())
        .sum()
}

/// Every wordlist word within soft distance 2 of `q`, nearest and most
/// likely first.
pub fn candidates(q: &[Distribution; WORD_LEN]) -> Vec<SoftCandidate> {
    let mut found: Vec<SoftCandidate> = Word::all()
        .filter_map(|word| {
            let distance = word
                .symbols()
                .iter()
                .zip(q)
                .filter(|(s, d)| !d.plausible(**s))
                .count();
            (distance <= 2).then(|| SoftCandidate {
                word,
                distance: distance as u8,
                likelihood: likelihood(q, word),
            })
        })
        .collect();
    found.sort_by(|a, b| {
        a.distance
            .cmp(&b.distance)
            .then(b.likelihood.total_cmp(&a.likelihood))
            .then(a.word.cmp(&b.word))
    });
    found
}

/// Decodes one word's recovered-symbol distributions.
pub fn decode_soft(q: &[Distribution; WORD_LEN]) -> SoftDecoded {
    let found = candidates(q);
    let accepted = match found.as_slice() {
        [] => return SoftDecoded::Unrecognized,
        [only] if only.distance <= 1 => *only,
        _ => return SoftDecoded::Candidates(found),
    };
    let word = accepted.word;
    let evidence: Vec<Evidence> = word
        .symbols()
        .iter()
        .zip(q)
        .enumerate()
        .filter_map(|(position, (&target, d))| {
            let best = d.argmax();
            if best != target {
                Some(Evidence::Substituted {
                    position,
                    from: best,
                    to: target,
                    p_from: d.p(best),
                    p_to: d.p(target),
                })
            } else if !d.plausible(target) {
                Some(Evidence::LowConfidence {
                    position,
                    p_target: d.p(target),
                })
            } else {
                None
            }
        })
        .collect();
    if evidence.is_empty() {
        SoftDecoded::Exact {
            word,
            likelihood: accepted.likelihood,
        }
    } else {
        SoftDecoded::Resolved {
            word,
            evidence,
            likelihood: accepted.likelihood,
        }
    }
}

/// A phrase recovered from soft readings.
#[derive(Clone, PartialEq, Debug)]
pub struct SoftRecovery {
    /// The recovered words in phrase order.
    pub words: Vec<Word>,
    /// Every inference made, as `(word_index, evidence)` in phrase order.
    pub evidence: Vec<(usize, Evidence)>,
    /// Each word's log-likelihood, in phrase order.
    pub likelihoods: Vec<f64>,
    /// Whether the words pass BIP39's own length and checksum rules.
    pub bip39_checksum_ok: bool,
}

impl SoftRecovery {
    /// The recovered phrase as a wallet expects it.
    pub fn phrase(&self) -> String {
        crate::backup::phrase(&self.words)
    }
}

/// Why soft recovery produced no phrase.
#[derive(Clone, PartialEq, Debug)]
pub enum SoftRecoverError {
    /// The two readings cannot be combined.
    Shares(ShareError),
    /// Every word that was not accepted, with its outcome.
    Undecodable(Vec<(usize, SoftDecoded)>),
}

impl From<ShareError> for SoftRecoverError {
    fn from(e: ShareError) -> Self {
        SoftRecoverError::Shares(e)
    }
}

impl fmt::Display for SoftRecoverError {
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

impl std::error::Error for SoftRecoverError {}

/// Combines and decodes every word of two readings.
pub fn recover_soft(
    a: &[[Distribution; WORD_LEN]],
    index_a: ShareIndex,
    b: &[[Distribution; WORD_LEN]],
    index_b: ShareIndex,
) -> Result<SoftRecovery, SoftRecoverError> {
    if index_a == index_b {
        return Err(ShareError::SameIndex(index_a).into());
    }
    if a.len() != b.len() {
        return Err(ShareError::LengthMismatch {
            a: (index_a, a.len()),
            b: (index_b, b.len()),
        }
        .into());
    }
    let decoded = a
        .iter()
        .zip(b)
        .map(|(wa, wb)| combine(wa, index_a, wb, index_b).map(|q| decode_soft(&q)))
        .collect::<Result<Vec<_>, _>>()?;
    resolve_soft(decoded)
}

/// Turns per-word outcomes into a phrase. A caller that has settled an
/// undecided word replaces its entry with an `Exact` first.
pub fn resolve_soft(decoded: Vec<SoftDecoded>) -> Result<SoftRecovery, SoftRecoverError> {
    let mut words = Vec::with_capacity(decoded.len());
    let mut evidence = Vec::new();
    let mut likelihoods = Vec::with_capacity(decoded.len());
    let mut undecided = Vec::new();
    for (index, outcome) in decoded.into_iter().enumerate() {
        match outcome {
            SoftDecoded::Exact { word, likelihood } => {
                words.push(word);
                likelihoods.push(likelihood);
            }
            SoftDecoded::Resolved {
                word,
                evidence: found,
                likelihood,
            } => {
                words.push(word);
                likelihoods.push(likelihood);
                evidence.extend(found.into_iter().map(|e| (index, e)));
            }
            other => undecided.push((index, other)),
        }
    }
    if !undecided.is_empty() {
        return Err(SoftRecoverError::Undecodable(undecided));
    }
    let bip39_checksum_ok = bip39_checksum_ok(&words);
    Ok(SoftRecovery {
        words,
        evidence,
        likelihoods,
        bip39_checksum_ok,
    })
}
