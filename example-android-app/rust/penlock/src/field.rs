//! GF(29) arithmetic and the worksheet alphabet.

use std::fmt;
use std::ops::{Add, Mul, Neg, Sub};

/// An element of GF(29), the alphabet every Penlock component is written in.
///
/// Characters map as `=`→0, `#`→1, `A`..`Z`→2..27, `-`→28, which is the
/// clockwise order of the wheel's outer ring.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Symbol(u8);

impl Symbol {
    /// The field order; every value is below it.
    pub const MODULUS: u8 = 29;
    /// The additive identity, written `=`.
    pub const ZERO: Symbol = Symbol(0);
    /// The multiplicative identity, written `#`.
    pub const ONE: Symbol = Symbol(1);

    const CHARS: [u8; 29] = *b"=#ABCDEFGHIJKLMNOPQRSTUVWXYZ-";

    /// The symbol with residue `value`, or `None` if `value >= 29`.
    pub fn new(value: u8) -> Option<Symbol> {
        (value < Self::MODULUS).then_some(Symbol(value))
    }

    /// Reduces any integer into the field.
    pub fn from_residue(value: u32) -> Symbol {
        Symbol((value % u32::from(Self::MODULUS)) as u8)
    }

    /// The residue, in `0..29`.
    pub fn value(self) -> u8 {
        self.0
    }

    /// Parses one worksheet character; letters may be in either case.
    pub fn from_char(c: char) -> Option<Symbol> {
        let c = c.to_ascii_uppercase();
        let pos = Self::CHARS.iter().position(|&x| char::from(x) == c)?;
        Some(Symbol(pos as u8))
    }

    /// The upper-case worksheet character for this symbol.
    pub fn to_char(self) -> char {
        char::from(Self::CHARS[usize::from(self.0)])
    }

    /// Every symbol in value order: `=`, `#`, `A`..`Z`, `-`.
    pub fn all() -> impl Iterator<Item = Symbol> {
        (0..Self::MODULUS).map(Symbol)
    }

    /// Multiplicative inverse; `None` for zero.
    pub fn inv(self) -> Option<Symbol> {
        if self.0 == 0 {
            return None;
        }
        // 29 is prime, so a^27 = a^-1 by Fermat.
        let mut result = Symbol::ONE;
        for _ in 0..Self::MODULUS - 2 {
            result = result * self;
        }
        Some(result)
    }

    /// Whether this is `=`, the symbol with no multiplicative inverse.
    pub fn is_zero(self) -> bool {
        self.0 == 0
    }

    /// A uniformly random symbol; the software equivalent of drawing a tile.
    pub fn random<R: rand::RngCore>(rng: &mut R) -> Symbol {
        Symbol(rand::Rng::random_range(rng, 0..Self::MODULUS))
    }
}

impl Add for Symbol {
    type Output = Symbol;
    fn add(self, rhs: Symbol) -> Symbol {
        Symbol((self.0 + rhs.0) % Self::MODULUS)
    }
}

impl Sub for Symbol {
    type Output = Symbol;
    fn sub(self, rhs: Symbol) -> Symbol {
        self + (-rhs)
    }
}

impl Neg for Symbol {
    type Output = Symbol;
    fn neg(self) -> Symbol {
        Symbol((Self::MODULUS - self.0) % Self::MODULUS)
    }
}

impl Mul for Symbol {
    type Output = Symbol;
    fn mul(self, rhs: Symbol) -> Symbol {
        Symbol::from_residue(u32::from(self.0) * u32::from(rhs.0))
    }
}

impl fmt::Display for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_char())
    }
}

impl fmt::Debug for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}[{}]", self.to_char(), self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn char_roundtrip_covers_the_field() {
        for s in Symbol::all() {
            assert_eq!(Symbol::from_char(s.to_char()), Some(s));
            assert_eq!(Symbol::from_char(s.to_char().to_ascii_lowercase()), Some(s));
        }
        assert_eq!(Symbol::from_char('='), Some(Symbol::ZERO));
        assert_eq!(Symbol::from_char('#'), Some(Symbol::ONE));
        assert_eq!(Symbol::from_char('A'), Symbol::new(2));
        assert_eq!(Symbol::from_char('Z'), Symbol::new(27));
        assert_eq!(Symbol::from_char('-'), Symbol::new(28));
        for c in ['0', ' ', '*', 'é', '_'] {
            assert_eq!(Symbol::from_char(c), None);
        }
        assert_eq!(Symbol::new(29), None);
    }

    #[test]
    fn field_axioms() {
        for a in Symbol::all() {
            assert_eq!(a + Symbol::ZERO, a);
            assert_eq!(a * Symbol::ONE, a);
            assert_eq!(a + (-a), Symbol::ZERO);
            assert_eq!(a - a, Symbol::ZERO);
            for b in Symbol::all() {
                assert_eq!(a + b, b + a);
                assert_eq!(a * b, b * a);
                assert_eq!((a + b) - b, a);
                for c in Symbol::all() {
                    assert_eq!((a + b) + c, a + (b + c));
                    assert_eq!((a * b) * c, a * (b * c));
                    assert_eq!(a * (b + c), a * b + a * c);
                }
            }
        }
    }

    #[test]
    fn inverses() {
        assert_eq!(Symbol::ZERO.inv(), None);
        for a in Symbol::all().filter(|a| !a.is_zero()) {
            assert_eq!(a * a.inv().unwrap(), Symbol::ONE);
        }
        assert_eq!(Symbol::new(2).unwrap().inv(), Symbol::new(15));
    }
}
