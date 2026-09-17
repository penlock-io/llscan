//! A share as written on the worksheet: six symbols per word, numbered.
//!
//! ```text
//! penlock v1 share 2
//!  1. KV ABAN
//!  2. #= MOTH
//! ```
//!
//! The space after the two checksum symbols mirrors the grey/white boxes on
//! paper; parsing ignores whitespace, case and line numbering.

use std::fmt;
use std::str::FromStr;

use crate::field::Symbol;
use crate::word::{WORD_LEN, symbols_to_string};

/// Which of the three shares this is. Recovery needs it: it is the x
/// coordinate the share was evaluated at.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ShareIndex(u8);

impl ShareIndex {
    /// Share 1.
    pub const ONE: ShareIndex = ShareIndex(1);
    /// Share 2.
    pub const TWO: ShareIndex = ShareIndex(2);
    /// Share 3.
    pub const THREE: ShareIndex = ShareIndex(3);

    /// The highest share number; a v1 backup has at most three shares.
    pub const MAX: u8 = 3;

    /// Share `number` for 1, 2 or 3; `None` otherwise.
    pub fn new(number: u8) -> Option<ShareIndex> {
        (1..=Self::MAX)
            .contains(&number)
            .then_some(ShareIndex(number))
    }

    /// The number written on the worksheet, in `1..=3`.
    pub fn number(self) -> u8 {
        self.0
    }

    /// Shares 1, 2 and 3 in order.
    pub fn all() -> impl Iterator<Item = ShareIndex> {
        (1..=Self::MAX).map(ShareIndex)
    }

    /// The index as a field element: the x coordinate of this share's points.
    pub fn symbol(self) -> Symbol {
        Symbol::from_residue(u32::from(self.0))
    }
}

impl fmt::Display for ShareIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// One share of a backup: its number and six symbols per word of the phrase,
/// in phrase order.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Share {
    index: ShareIndex,
    words: Vec<[Symbol; WORD_LEN]>,
}

impl Share {
    /// A share from one six-symbol group per word.
    pub fn new(index: ShareIndex, words: Vec<[Symbol; WORD_LEN]>) -> Share {
        Share { index, words }
    }

    /// Which share this is.
    pub fn index(&self) -> ShareIndex {
        self.index
    }

    /// The six symbols of every word, indexed by word then by position.
    pub fn words(&self) -> &[[Symbol; WORD_LEN]] {
        &self.words
    }

    /// Mutable access to [`Share::words`], e.g. to model a copying error.
    pub fn words_mut(&mut self) -> &mut [[Symbol; WORD_LEN]] {
        &mut self.words
    }

    /// Parses the text format. A header line `penlock v1 share N` supplies
    /// the index; `index` is used when the header is absent, since a paper
    /// share is only labelled by the section it was written in.
    pub fn parse(text: &str, index: Option<ShareIndex>) -> Result<Share, ParseShareError> {
        let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
        let first = lines.next().ok_or(ParseShareError::Empty)?;

        let (header_index, body): (Option<ShareIndex>, Vec<&str>) =
            if first.to_ascii_lowercase().starts_with("penlock") {
                (Some(parse_header(first)?), lines.collect())
            } else {
                (None, std::iter::once(first).chain(lines).collect())
            };

        let index = match (header_index, index) {
            (Some(h), Some(i)) if h != i => return Err(ParseShareError::IndexConflict(h, i)),
            (Some(h), _) => h,
            (None, Some(i)) => i,
            (None, None) => return Err(ParseShareError::MissingIndex),
        };

        let words = parse_words(body.iter().copied())?;
        if words.is_empty() {
            return Err(ParseShareError::Empty);
        }
        Ok(Share { index, words })
    }
}

fn parse_header(line: &str) -> Result<ShareIndex, ParseShareError> {
    let tokens: Vec<String> = line
        .split_whitespace()
        .map(str::to_ascii_lowercase)
        .collect();
    let bad = || ParseShareError::BadHeader(line.to_owned());
    match tokens.as_slice() {
        [p, v, s, n] if p == "penlock" && v == "v1" && s == "share" => n
            .parse::<u8>()
            .ok()
            .and_then(ShareIndex::new)
            .ok_or_else(bad),
        _ => Err(bad()),
    }
}

/// Reads numbered lines of symbols, ignoring numbering, whitespace and
/// case. Digits can only be numbering because no symbol is a digit.
pub fn parse_words<'a>(
    lines: impl Iterator<Item = &'a str>,
) -> Result<Vec<[Symbol; WORD_LEN]>, ParseShareError> {
    let mut symbols = Vec::new();
    for line in lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let body = strip_numbering(line);
        for c in body.chars().filter(|c| !c.is_whitespace()) {
            symbols.push(Symbol::from_char(c).ok_or(ParseShareError::BadSymbol(c))?);
        }
    }
    if symbols.len() % WORD_LEN != 0 {
        return Err(ParseShareError::BadLength(symbols.len()));
    }
    Ok(symbols
        .chunks_exact(WORD_LEN)
        .map(|w| w.try_into().unwrap())
        .collect())
}

fn strip_numbering(line: &str) -> &str {
    let rest = line.trim_start_matches(|c: char| c.is_ascii_digit());
    if rest.len() == line.len() {
        return line;
    }
    rest.trim_start_matches(['.', ')', ':']).trim_start()
}

/// Formats words as numbered worksheet lines.
pub fn format_words(words: &[[Symbol; WORD_LEN]]) -> String {
    words
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let s = symbols_to_string(*w);
            format!("{:>2}. {} {}\n", i + 1, &s[..2], &s[2..])
        })
        .collect()
}

impl fmt::Display for Share {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "penlock v1 share {}", self.index)?;
        f.write_str(&format_words(&self.words))
    }
}

impl FromStr for Share {
    type Err = ParseShareError;

    fn from_str(s: &str) -> Result<Share, ParseShareError> {
        Share::parse(s, None)
    }
}

/// Why a share's text could not be read.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ParseShareError {
    /// No symbols at all, with or without a header.
    Empty,
    /// The first line starts with `penlock` but is not `penlock v1 share N`
    /// with `N` in `1..=3`.
    BadHeader(String),
    /// No header and no index was supplied.
    MissingIndex,
    /// The header's share number and the supplied index disagree; header
    /// first, supplied second.
    IndexConflict(ShareIndex, ShareIndex),
    /// A character that is not a worksheet symbol.
    BadSymbol(char),
    /// Total symbol count is not a multiple of six.
    BadLength(usize),
}

impl fmt::Display for ParseShareError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "share is empty"),
            Self::BadHeader(h) => write!(f, "bad header {h:?}, expected `penlock v1 share N`"),
            Self::MissingIndex => write!(f, "share index unknown: add a header or pass it"),
            Self::IndexConflict(h, i) => {
                write!(f, "header says share {h} but share {i} was requested")
            }
            Self::BadSymbol(c) => write!(f, "{c:?} is not a Penlock symbol"),
            Self::BadLength(n) => {
                write!(
                    f,
                    "{n} symbols is not a whole number of {WORD_LEN}-symbol words"
                )
            }
        }
    }
}

impl std::error::Error for ParseShareError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::word::symbols_from_str;

    fn share() -> Share {
        Share::new(
            ShareIndex::new(2).unwrap(),
            vec![
                symbols_from_str("KVABAN").unwrap(),
                symbols_from_str("#=MOTH").unwrap(),
                symbols_from_str("--ZOO-").unwrap(),
            ],
        )
    }

    #[test]
    fn display_matches_worksheet_layout() {
        assert_eq!(
            share().to_string(),
            "penlock v1 share 2\n 1. KV ABAN\n 2. #= MOTH\n 3. -- ZOO-\n"
        );
    }

    #[test]
    fn parse_is_lenient() {
        let expected = share();
        for text in [
            "penlock v1 share 2\n 1. KV ABAN\n 2. #= MOTH\n 3. -- ZOO-\n",
            "PENLOCK V1 SHARE 2\nkvaban #=moth --zoo-",
            "penlock v1 share 2\n\n1) KVABAN\n2: #=MOTH\n3 --ZOO-\n",
            "penlock v1 share 2\nKV AB AN #= MO TH -- ZO O-",
        ] {
            assert_eq!(text.parse::<Share>().unwrap(), expected, "{text:?}");
        }
        assert_eq!(
            Share::parse(" 1. KV ABAN\n 2. #= MOTH\n 3. -- ZOO-", ShareIndex::new(2)).unwrap(),
            expected
        );
    }

    #[test]
    fn parse_errors() {
        assert_eq!("".parse::<Share>(), Err(ParseShareError::Empty));
        assert_eq!(
            "KVABAN".parse::<Share>(),
            Err(ParseShareError::MissingIndex)
        );
        assert_eq!(
            "penlock v1 share 2\n".parse::<Share>(),
            Err(ParseShareError::Empty)
        );
        assert_eq!(
            "penlock v1 share 4\nKVABAN".parse::<Share>(),
            Err(ParseShareError::BadHeader("penlock v1 share 4".into()))
        );
        assert_eq!(
            "penlock v2 share 1\nKVABAN".parse::<Share>(),
            Err(ParseShareError::BadHeader("penlock v2 share 1".into()))
        );
        assert_eq!(
            Share::parse("penlock v1 share 1\nKVABAN", ShareIndex::new(3)),
            Err(ParseShareError::IndexConflict(
                ShareIndex::new(1).unwrap(),
                ShareIndex::new(3).unwrap()
            ))
        );
        assert_eq!(
            "penlock v1 share 1\nKVABA".parse::<Share>(),
            Err(ParseShareError::BadLength(5))
        );
        assert_eq!(
            "penlock v1 share 1\nKV*BAN".parse::<Share>(),
            Err(ParseShareError::BadSymbol('*'))
        );
    }
}
