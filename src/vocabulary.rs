//! English BIP39 identities, independent of wallet and share-card code.

/// An index in the standard English BIP39 word list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Word(u16);

impl Word {
    /// Number of English BIP39 words.
    pub const COUNT: usize = 2048;
    /// Looks up an English word, ignoring ASCII case.
    pub fn from_bip39(word: &str) -> Option<Self> {
        bip39::Language::English
            .find_word(&word.to_ascii_lowercase())
            .map(Self)
    }
    /// Looks up a zero-based word-list index, rejecting indices >=2048.
    pub fn from_index(index: u16) -> Option<Self> {
        (usize::from(index) < Self::COUNT).then_some(Self(index))
    }
    /// Zero-based word-list index.
    pub fn index(self) -> u16 {
        self.0
    }
    /// Full lowercase English spelling.
    pub fn as_str(self) -> &'static str {
        bip39::Language::English.word_list()[usize::from(self.0)]
    }
    /// Every word in word-list order.
    pub fn all() -> impl Iterator<Item = Self> {
        (0..Self::COUNT as u16).map(Self)
    }
    /// Parses whitespace-separated BIP39 words without repairing misspellings.
    pub fn parse_phrase(phrase: &str) -> Result<Vec<Self>, String> {
        phrase
            .split_whitespace()
            .enumerate()
            .map(|(i, text)| {
                Self::from_bip39(text)
                    .ok_or_else(|| format!("word{} is not in the BIP39 vocabulary", i + 1))
            })
            .collect()
    }
}

/// Validates length and BIP39 checksum without changing order or identities.
/// A passing checksum does not establish that OCR recognized the right phrase.
pub fn checksum_ok(words: &[Word]) -> bool {
    let phrase = words
        .iter()
        .map(|w| w.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    bip39::Mnemonic::parse_in_normalized(bip39::Language::English, &phrase).is_ok()
}

impl std::fmt::Display for Word {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_index_roundtrips_without_wallet_types() {
        for w in Word::all() {
            assert_eq!(Word::from_bip39(w.as_str()), Some(w));
            assert_eq!(Word::from_bip39(&w.as_str().to_uppercase()), Some(w));
            assert_eq!(Word::from_index(w.index()), Some(w));
        }
        assert_eq!(Word::all().count(), 2048);
        assert!(Word::from_index(2048).is_none());
        assert!(Word::from_bip39("ensur").is_none());
    }
    #[test]
    fn checksum_observes_but_does_not_repair() {
        let words=Word::parse_phrase("abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about").unwrap();
        assert!(checksum_ok(&words));
        assert!(!checksum_ok(&words[..11]));
        let mut wrong = words.clone();
        wrong[11] = Word::from_bip39("abandon").unwrap();
        assert!(!checksum_ok(&wrong));
        assert_eq!(words[11].as_str(), "about");
    }
}
