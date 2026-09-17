//! BIP39 words as Penlock writes them: two checksum symbols followed by the
//! first four letters, dash-padded (`abandon` → `SSABAN`, `zoo` → `SMZOO-`).

use std::fmt;
use std::sync::LazyLock;

use crate::field::Symbol;

/// Symbols per word on the worksheet.
pub const WORD_LEN: usize = 6;
/// Letters of a word written on the worksheet: its first four,
/// dash-padded.
pub const LETTERS: usize = 4;

const CHECKSUM_WEIGHTS: [[u8; LETTERS]; 2] = [[14, 2, 11, 15], [6, 20, 12, 22]];

/// A word of the English BIP39 wordlist.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Word(u16);

impl Word {
    /// Size of the wordlist.
    pub const COUNT: usize = 2048;

    /// Looks a word up in the wordlist; case-insensitive.
    pub fn from_bip39(word: &str) -> Option<Word> {
        let word = word.to_ascii_lowercase();
        bip39::Language::English.find_word(&word).map(Word)
    }

    /// Splits a phrase on whitespace and looks every word up.
    pub fn parse_phrase(phrase: &str) -> Result<Vec<Word>, UnknownWord> {
        phrase
            .split_whitespace()
            .enumerate()
            .map(|(index, text)| {
                Word::from_bip39(text).ok_or_else(|| UnknownWord {
                    index,
                    text: text.to_owned(),
                })
            })
            .collect()
    }

    /// The word at wordlist position `index` (`0..2048`); `None` beyond it.
    pub fn from_index(index: u16) -> Option<Word> {
        (usize::from(index) < Self::COUNT).then_some(Word(index))
    }

    /// Position in the wordlist, `0..2048`.
    pub fn index(self) -> u16 {
        self.0
    }

    /// The full lower-case word.
    pub fn as_str(self) -> &'static str {
        bip39::Language::English.word_list()[usize::from(self.0)]
    }

    /// The wordlist in order.
    pub fn all() -> impl Iterator<Item = Word> {
        (0..Self::COUNT as u16).map(Word)
    }

    /// The first four letters, dash-padded, as field symbols.
    pub fn letters(self) -> [Symbol; LETTERS] {
        let mut out = [Symbol::from_char('-').unwrap(); LETTERS];
        for (slot, c) in out.iter_mut().zip(self.as_str().chars()) {
            *slot = Symbol::from_char(c).expect("wordlist is ASCII letters");
        }
        out
    }

    /// The six symbols written on the worksheet for this word.
    pub fn symbols(self) -> [Symbol; WORD_LEN] {
        CODEWORDS[usize::from(self.0)]
    }
}

impl fmt::Display for Word {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A token of a phrase that is not in the wordlist. `index` is zero-based.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct UnknownWord {
    /// Position of the token in the phrase.
    pub index: usize,
    /// The token as given.
    pub text: String,
}

impl fmt::Display for UnknownWord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "word {} ({:?}) is not in the BIP39 wordlist",
            self.index + 1,
            self.text
        )
    }
}

impl std::error::Error for UnknownWord {}

/// The two parity symbols Penlock prepends to a word's letters.
pub fn checksum(letters: [Symbol; LETTERS]) -> [Symbol; 2] {
    CHECKSUM_WEIGHTS.map(|weights| {
        letters
            .iter()
            .zip(weights)
            .fold(Symbol::ZERO, |acc, (&l, w)| {
                acc + l * Symbol::from_residue(u32::from(w))
            })
    })
}

fn codeword(word: Word) -> [Symbol; WORD_LEN] {
    let letters = word.letters();
    let [p1, p2] = checksum(letters);
    [p1, p2, letters[0], letters[1], letters[2], letters[3]]
}

static CODEWORDS: LazyLock<Vec<[Symbol; WORD_LEN]>> =
    LazyLock::new(|| Word::all().map(codeword).collect());

const PAIRS: [(usize, usize); 3] = [(0, 1), (2, 3), (4, 5)];

fn pair_key(symbols: [Symbol; WORD_LEN], (i, j): (usize, usize)) -> usize {
    usize::from(symbols[i].value()) * usize::from(Symbol::MODULUS) + usize::from(symbols[j].value())
}

// A word within distance 2 of the received one agrees with it on at least
// one of the three disjoint pairs, so only those buckets need scanning.
static PAIR_INDEX: LazyLock<[Vec<Vec<u16>>; 3]> = LazyLock::new(|| {
    let buckets = usize::from(Symbol::MODULUS).pow(2);
    let mut index = [(); 3].map(|_| vec![Vec::new(); buckets]);
    for (i, &cw) in CODEWORDS.iter().enumerate() {
        for (p, pair) in PAIRS.into_iter().enumerate() {
            index[p][pair_key(cw, pair)].push(i as u16);
        }
    }
    index
});

fn nearby(received: [Symbol; WORD_LEN]) -> Vec<Candidate> {
    let mut indices: Vec<u16> = PAIRS
        .into_iter()
        .enumerate()
        .flat_map(|(p, pair)| PAIR_INDEX[p][pair_key(received, pair)].iter().copied())
        .collect();
    indices.sort_unstable();
    indices.dedup();
    let mut candidates: Vec<Candidate> = indices
        .into_iter()
        .filter_map(|i| {
            let distance = distance(received, CODEWORDS[usize::from(i)]);
            (distance <= 2).then_some(Candidate {
                word: Word(i),
                distance,
            })
        })
        .collect();
    candidates.sort_by_key(|c| (c.distance, c.word));
    candidates
}

/// Number of positions in which two worksheet words differ.
pub fn distance(a: [Symbol; WORD_LEN], b: [Symbol; WORD_LEN]) -> u8 {
    a.iter().zip(b).filter(|(x, y)| **x != *y).count() as u8
}

/// A wordlist word near a received word.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Candidate {
    /// The wordlist word.
    pub word: Word,
    /// How many of the six symbols differ from what was received, `1..=2`.
    pub distance: u8,
}

/// Outcome of decoding six recovered symbols against the wordlist.
///
/// The wordlist codewords are at mutual distance ≥ 3, so a received word
/// with at most two errors always has its true word within distance 2. Only
/// a lone candidate at distance 1 is accepted automatically: a second error
/// would have put the true word at distance 2, where it would also have
/// been listed.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Decoded {
    /// The received symbols are this word.
    Exact(Word),
    /// One symbol was wrong and this is the only word within distance 2.
    Corrected {
        /// The word the symbols were corrected to.
        word: Word,
        /// Which of the six symbols was wrong.
        position: usize,
        /// The symbol received at `position`.
        from: Symbol,
        /// The symbol `word` has at `position`.
        to: Symbol,
    },
    /// Every wordlist word within distance 2, nearest first, whenever that
    /// set is neither a single exact match nor a single distance-1 match.
    /// Never empty. Deciding between them is the caller's job.
    Candidates(Vec<Candidate>),
    /// Nothing within distance 2: three or more errors.
    Unrecognized,
}

/// Decodes six recovered symbols by distance to the wordlist; see
/// [`Decoded`] for the policy.
pub fn decode(received: [Symbol; WORD_LEN]) -> Decoded {
    let candidates = nearby(received);
    match candidates.as_slice() {
        [] => Decoded::Unrecognized,
        [Candidate { word, distance: 0 }] => Decoded::Exact(*word),
        [Candidate { word, distance: 1 }] => {
            let position = word
                .symbols()
                .iter()
                .zip(received)
                .position(|(a, b)| *a != b)
                .expect("distance 1 has a differing position");
            Decoded::Corrected {
                word: *word,
                position,
                from: received[position],
                to: word.symbols()[position],
            }
        }
        _ => Decoded::Candidates(candidates),
    }
}

/// Parses exactly six worksheet characters, in either case; `None` for any
/// other length or character.
pub fn symbols_from_str(s: &str) -> Option<[Symbol; WORD_LEN]> {
    let mut out = [Symbol::ZERO; WORD_LEN];
    let mut chars = s.chars();
    for slot in &mut out {
        *slot = Symbol::from_char(chars.next()?)?;
    }
    chars.next().is_none().then_some(out)
}

/// The six symbols as upper-case worksheet characters, no separator.
pub fn symbols_to_string(symbols: [Symbol; WORD_LEN]) -> String {
    symbols.iter().map(|s| s.to_char()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sym(s: &str) -> [Symbol; WORD_LEN] {
        symbols_from_str(s).unwrap()
    }

    #[test]
    fn known_codewords() {
        assert_eq!(
            symbols_to_string(Word::from_bip39("abandon").unwrap().symbols()),
            "SSABAN"
        );
        assert_eq!(
            symbols_to_string(Word::from_bip39("ABSTRACT").unwrap().symbols()),
            "QSABST"
        );
        assert_eq!(
            symbols_to_string(Word::from_bip39("zoo").unwrap().symbols()),
            "SMZOO-"
        );
        assert_eq!(Word::from_bip39("penlock"), None);
    }

    #[test]
    fn decode_exact_and_single_error() {
        let abandon = Word::from_bip39("abandon").unwrap();
        assert_eq!(decode(sym("SSABAN")), Decoded::Exact(abandon));
        assert_eq!(
            decode(sym("SSABAQ")),
            Decoded::Corrected {
                word: abandon,
                position: 5,
                from: Symbol::from_char('Q').unwrap(),
                to: Symbol::from_char('N').unwrap(),
            }
        );
    }

    #[test]
    fn two_errors_can_land_next_to_another_word() {
        let abandon = Word::from_bip39("abandon").unwrap();
        let abstract_ = Word::from_bip39("abstract").unwrap();
        assert_eq!(
            decode(sym("QSABSN")),
            Decoded::Candidates(vec![
                Candidate {
                    word: abstract_,
                    distance: 1
                },
                Candidate {
                    word: abandon,
                    distance: 2
                },
            ])
        );
    }

    #[test]
    fn lone_distance_two_candidate_is_not_auto_corrected() {
        let abandon = Word::from_bip39("abandon").unwrap();
        assert_eq!(
            decode(sym("==ABAN")),
            Decoded::Candidates(vec![Candidate {
                word: abandon,
                distance: 2
            }])
        );
    }

    #[test]
    fn three_errors_are_unrecognized() {
        assert_eq!(decode(sym("===BAN")), Decoded::Unrecognized);
    }

    #[test]
    fn pair_index_finds_everything_a_full_scan_does() {
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        for _ in 0..3000 {
            let received = [Symbol::ZERO; WORD_LEN].map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                Symbol::from_residue((state >> 32) as u32)
            });
            let mut scanned: Vec<Candidate> = CODEWORDS
                .iter()
                .enumerate()
                .map(|(i, &cw)| Candidate {
                    word: Word(i as u16),
                    distance: distance(received, cw),
                })
                .filter(|c| c.distance <= 2)
                .collect();
            scanned.sort_by_key(|c| (c.distance, c.word));
            assert_eq!(nearby(received), scanned);
        }
    }

    #[test]
    fn symbol_string_roundtrip() {
        assert_eq!(symbols_from_str("ssaban"), Some(sym("SSABAN")));
        assert_eq!(symbols_from_str("SSABA"), None);
        assert_eq!(symbols_from_str("SSABANX"), None);
        assert_eq!(symbols_from_str("SSAB4N"), None);
    }
}
