//! Oracles of correct reconstruction beyond the search's own
//! [`Known`](crate::soft::search::Known): what a user can hold that only
//! the right phrase reproduces.

#[cfg(feature = "wallet")]
use std::fmt;

use crate::word::Word;

/// The BIP39 seed of `words` with `passphrase` (empty for none). `None`
/// if the words are not a valid mnemonic: PBKDF2 is defined on the
/// sentence, but a phrase failing its own checksum has no wallet.
pub fn seed(words: &[Word], passphrase: &str) -> Option<[u8; 64]> {
    let sentence: Vec<&str> = words.iter().map(|w| w.as_str()).collect();
    let mnemonic =
        bip39::Mnemonic::parse_in_normalized(bip39::Language::English, &sentence.join(" ")).ok()?;
    Some(mnemonic.to_seed(passphrase))
}

/// The BIP32 master key fingerprint (`hash160` of the master public
/// key's first four bytes) that every descriptor's key origin starts
/// with: `[deadbeef/84h/0h/0h]`. Accepts a phrase whose seed derives it.
#[cfg(feature = "wallet")]
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Fingerprint {
    bytes: [u8; 4],
    passphrase: String,
}

#[cfg(feature = "wallet")]
impl Fingerprint {
    /// From eight hex digits, as printed in a descriptor.
    pub fn parse(hex: &str, passphrase: &str) -> Option<Fingerprint> {
        if hex.len() != 8 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let mut bytes = [0u8; 4];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok()?;
        }
        Some(Fingerprint {
            bytes,
            passphrase: passphrase.to_owned(),
        })
    }

    /// The fingerprint of `seed`'s master key.
    pub fn of_seed(seed: &[u8; 64]) -> [u8; 4] {
        use bitcoin::bip32::Xpriv;
        use bitcoin::secp256k1::Secp256k1;
        let master = Xpriv::new_master(bitcoin::Network::Bitcoin, seed)
            .expect("a 64-byte seed always yields a master key");
        master.fingerprint(&Secp256k1::new()).to_bytes()
    }

    /// The fingerprint of the phrase `words` with this oracle's
    /// passphrase, if the words are a valid mnemonic.
    pub fn of_phrase(words: &[Word], passphrase: &str) -> Option<[u8; 4]> {
        seed(words, passphrase).map(|s| Fingerprint::of_seed(&s))
    }

    /// The four bytes.
    pub fn bytes(&self) -> [u8; 4] {
        self.bytes
    }
}

#[cfg(feature = "wallet")]
impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for b in self.bytes {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

#[cfg(feature = "wallet")]
impl crate::soft::search::Oracle for Fingerprint {
    fn accepts(&self, words: &[Word]) -> bool {
        Fingerprint::of_phrase(words, &self.passphrase) == Some(self.bytes)
    }
    fn name(&self) -> String {
        format!("fingerprint {self}")
    }
}

#[cfg(feature = "wallet")]
#[cfg(test)]
mod tests {
    /// BIP32's first test vector: seed `000102…0f`, master fingerprint
    /// `3442193e`.
    #[test]
    fn bip32_vector_fingerprint() {
        let seed: Vec<u8> = (0u8..16).collect();
        use bitcoin::bip32::Xpriv;
        use bitcoin::secp256k1::Secp256k1;
        let master = Xpriv::new_master(bitcoin::Network::Bitcoin, &seed).unwrap();
        assert_eq!(
            master.fingerprint(&Secp256k1::new()).to_bytes(),
            [0x34, 0x42, 0x19, 0x3e]
        );
    }
}
