//! The sixteen-slot mark field between a strip's fiducials, split by
//! meaning: the top eight slots carry a project id byte — Penlock's is
//! the ASCII capital P — and the bottom eight an extended Hamming
//! (8,4) codeword over the 4-bit sheet number. Each row keeps its own
//! never-misroute guarantee: the id accepts at most one flipped mark
//! before it reads as another project's field, and the number code
//! corrects one mark and refuses two. The all-blank and all-dark
//! fields can never read as the id byte. The field is read off an
//! oriented strip — the orientation mark settles which way is up
//! before any slot is sampled. docs/marks.md records the convention.

/// The byte Penlock prints in the top row.
pub const PROJECT_ID: u8 = b'P';

/// XORed over the number row's codeword: sheet 0 would otherwise
/// print a blank bottom row, and a blank row must never decode — with
/// the mask it sits at least two marks from every valid codeword, so
/// a P-shaped smudge over an empty field refuses instead of reading
/// as the seed strip.
const NUMBER_MASK: u8 = 0b0000_0110;

/// What a present field says.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Field {
    /// A Penlock strip and its sheet number.
    Penlock {
        /// 0 the seed strip, 1–15 the shares.
        number: u8,
    },
    /// Another project's field: both raw bytes, untouched.
    Foreign {
        /// The top row's byte.
        id: u8,
        /// The bottom row's byte.
        bits: u8,
    },
    /// Penlock's id, but the number row fails its code: a damaged
    /// strip, refused rather than guessed at.
    Damaged,
}

/// What the mark field says a strip is: always legacy or a Penlock
/// field — anything else never becomes a `Found`, it is a locate
/// error instead.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    /// A strip carrying Penlock's field.
    Penlock {
        /// The sheet number: 0 the seed strip, 1–15 the shares.
        number: u8,
    },
    /// No field between the fiducials: a strip from before the field,
    /// identified by whatever marking its print carries.
    Legacy,
}

/// Data sits at Hamming positions 3, 5, 6 and 7; checks at 1, 2 and
/// 4; the extended parity at 0.
const DATA_BITS: [usize; 4] = [3, 5, 6, 7];

fn encode84(number: u8) -> u8 {
    debug_assert!(number < 16);
    let mut code = 0u8;
    for (k, bit) in DATA_BITS.iter().enumerate() {
        if number >> k & 1 == 1 {
            code |= 1 << bit;
        }
    }
    for check in [1u8, 2, 4] {
        let mut parity = 0u8;
        for bit in 1..8 {
            if bit & check != 0 && code >> bit & 1 == 1 {
                parity ^= 1;
            }
        }
        if parity == 1 {
            code |= 1 << check;
        }
    }
    if code.count_ones() % 2 == 1 {
        code |= 1;
    }
    code
}

fn decode84(byte: u8) -> Option<u8> {
    let mut code = byte;
    let mut syndrome = 0u8;
    for bit in 1..8 {
        if code >> bit & 1 == 1 {
            syndrome ^= bit;
        }
    }
    let parity = code.count_ones() % 2;
    match (syndrome, parity) {
        (0, 0) => {}
        (s, 1) => code ^= 1 << s,
        (_, 0) => return None,
        _ => unreachable!(),
    }
    let mut number = 0u8;
    for (k, bit) in DATA_BITS.iter().enumerate() {
        if code >> bit & 1 == 1 {
            number |= 1 << k;
        }
    }
    Some(number)
}

/// The byte the recovery phrase sheet prints in the top row: another id
/// than the strips', so a strip reader sees a word sheet as foreign
/// and a word-sheet reader sees a strip as foreign, each refusing the
/// other by the same rule that refuses any other project.
pub const WORD_SHEET_ID: u8 = b'W';

/// The printed field for sheet `number` (0 the seed strip, 1–15 the
/// shares). Bit `i` of the result is slot `i`; within each row the
/// leftmost slot is the byte's most significant bit.
pub fn encode(number: u8) -> u16 {
    encode_with(PROJECT_ID, number)
}

/// [`encode`] under another id byte: the word sheet's, with its
/// version as the number.
pub fn encode_with(id: u8, number: u8) -> u16 {
    u16::from(id.reverse_bits()) | u16::from((encode84(number) ^ NUMBER_MASK).reverse_bits()) << 8
}

/// Decodes a printed field, correcting at most one mark per row.
pub fn decode(field: u16) -> Field {
    decode_with(PROJECT_ID, field)
}

/// [`decode`] for a reader whose own id is `own`: `Penlock { number }`
/// then means a field under that id, the word sheet's version when
/// `own` is [`WORD_SHEET_ID`].
pub fn decode_with(own: u8, field: u16) -> Field {
    let id = ((field & 0xFF) as u8).reverse_bits();
    let bits = ((field >> 8) as u8).reverse_bits();
    if (id ^ own).count_ones() > 1 {
        return Field::Foreign { id, bits };
    }
    match decode84(bits ^ NUMBER_MASK) {
        Some(number) => Field::Penlock { number },
        None => Field::Damaged,
    }
}
