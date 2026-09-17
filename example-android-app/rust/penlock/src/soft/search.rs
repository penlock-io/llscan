//! Recovery as search: candidate phrases in order of cost under the
//! model, each put to a reconstruction oracle until one is accepted.
//!
//! Every word slot ranks the whole wordlist by an integer cost (millinats
//! of negative log-probability after smoothing over the symbols that can
//! occur at each position); phrases are enumerated best-first over the
//! product of those lists, exhaustively and without repeats; free
//! [`Filter`]s (the BIP39 checksum) screen candidates before the counted
//! [`Oracle`] is asked. The truth's position in that order is bracketed
//! exactly, without enumerating, by convolving the slots' cost
//! histograms — the number the bench rates a model's ordering by.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, HashSet};
use std::fmt;

use crate::backup::bip39_checksum_ok;
use crate::field::Symbol;
use crate::share::ShareIndex;
use crate::soft::{Distribution, PLAUSIBLE};
use crate::word::{LETTERS, WORD_LEN, Word};

/// Millinats: costs are `round(1000 · −ln p)`.
const SCALE: f64 = 1000.0;

/// The smoothing mass `ε` credited to every allowed symbol before a
/// position's probabilities are renormalised: what a confident misread
/// costs relative to a certain match is `ln((1 + ε) / ε)`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Floor(f32);

impl Floor {
    /// The plausibility threshold, [`PLAUSIBLE`].
    pub const DEFAULT: Floor = Floor(PLAUSIBLE);

    /// `None` unless `epsilon` is finite and in `0 < ε ≤ 1`.
    pub fn new(epsilon: f32) -> Option<Floor> {
        (epsilon.is_finite() && epsilon > 0.0 && epsilon <= 1.0).then_some(Floor(epsilon))
    }

    /// The value.
    pub fn get(self) -> f32 {
        self.0
    }
}

impl Default for Floor {
    fn default() -> Self {
        Floor::DEFAULT
    }
}

/// The symbols that can occur at `position` of a worksheet word: any at
/// the two checksum positions, letters at the four letter positions,
/// with `-` (the padding of three-letter words) only at the last.
pub fn allowed(position: usize) -> impl Iterator<Item = Symbol> {
    Symbol::all().filter(move |s| {
        position < WORD_LEN - LETTERS
            || s.to_char().is_ascii_uppercase()
            || (position == WORD_LEN - 1 && s.to_char() == '-')
    })
}

/// Cost in millinats of each symbol at `position`: smoothed by `floor`,
/// renormalised over [`allowed`] symbols. Disallowed symbols get
/// `u32::MAX`; no wordlist word uses them there.
pub fn position_costs(q: &Distribution, position: usize, floor: Floor) -> [u32; 29] {
    let eps = f64::from(floor.get());
    let allowed: Vec<Symbol> = allowed(position).collect();
    let z: f64 = allowed.iter().map(|&s| f64::from(q.p(s)) + eps).sum();
    let mut costs = [u32::MAX; 29];
    for &s in &allowed {
        let p = (f64::from(q.p(s)) + eps) / z;
        costs[usize::from(s.value())] = (-p.ln() * SCALE).round() as u32;
    }
    costs
}

/// One word slot's ranking of distinct words by cost; never empty.
#[derive(Clone, PartialEq, Debug)]
pub struct SlotRanking {
    /// `(cost, word)` sorted by cost, then wordlist index.
    ranked: Vec<(u32, Word)>,
}

/// Why a list of costs is not a ranking.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RankingError {
    /// No words at all.
    Empty,
    /// A word listed twice; each phrase must be enumerable exactly once.
    Duplicate(Word),
}

impl fmt::Display for RankingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("a ranking needs at least one word"),
            Self::Duplicate(w) => write!(f, "{w} is listed twice"),
        }
    }
}

impl std::error::Error for RankingError {}

impl SlotRanking {
    /// Ranks every wordlist word by the sum of its symbols' costs under
    /// `q`.
    pub fn from_distributions(q: &[Distribution; WORD_LEN], floor: Floor) -> SlotRanking {
        let per_position: Vec<[u32; 29]> = (0..WORD_LEN)
            .map(|p| position_costs(&q[p], p, floor))
            .collect();
        SlotRanking::from_costs(Word::all().map(|w| {
            let cost = w
                .symbols()
                .iter()
                .zip(&per_position)
                .map(|(s, costs)| costs[usize::from(s.value())])
                .sum();
            (cost, w)
        }))
        .expect("the wordlist is non-empty and has no duplicates")
    }

    /// A slot that can only be `word`: the user settled it.
    pub fn fixed(word: Word) -> SlotRanking {
        SlotRanking {
            ranked: vec![(0, word)],
        }
    }

    /// Ranks the given `(cost, word)` pairs, which must name at least one
    /// word and no word twice.
    pub fn from_costs(
        costs: impl IntoIterator<Item = (u32, Word)>,
    ) -> Result<SlotRanking, RankingError> {
        let mut ranked: Vec<(u32, Word)> = costs.into_iter().collect();
        if ranked.is_empty() {
            return Err(RankingError::Empty);
        }
        let mut seen = HashSet::new();
        for &(_, w) in &ranked {
            if !seen.insert(w) {
                return Err(RankingError::Duplicate(w));
            }
        }
        ranked.sort_by_key(|&(c, w)| (c, w.index()));
        Ok(SlotRanking { ranked })
    }

    /// The ranking, cheapest first.
    pub fn ranked(&self) -> &[(u32, Word)] {
        &self.ranked
    }

    /// The cost of `word` in this slot, if listed.
    pub fn cost_of(&self, word: Word) -> Option<u32> {
        self.ranked
            .iter()
            .find(|&&(_, w)| w == word)
            .map(|&(c, _)| c)
    }
}

/// A phrase yielded by the enumeration.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Candidate {
    /// The words.
    pub words: Vec<Word>,
    /// Total cost in millinats.
    pub cost: u64,
    /// Which rank each slot contributed.
    pub ranks: Vec<u16>,
}

/// Best-first enumeration of every phrase over the slots' rankings, in
/// non-decreasing cost, ties by rank vector, each phrase once.
pub struct Enumeration<'a> {
    slots: &'a [SlotRanking],
    heap: BinaryHeap<Reverse<(u64, Vec<u16>)>>,
    seen: HashSet<Vec<u16>>,
}

impl<'a> Enumeration<'a> {
    /// Starts at the cheapest phrase.
    pub fn new(slots: &'a [SlotRanking]) -> Enumeration<'a> {
        let start = vec![0u16; slots.len()];
        let cost = slots.iter().map(|s| u64::from(s.ranked[0].0)).sum();
        let mut heap = BinaryHeap::new();
        let mut seen = HashSet::new();
        heap.push(Reverse((cost, start.clone())));
        seen.insert(start);
        Enumeration { slots, heap, seen }
    }
}

impl Iterator for Enumeration<'_> {
    type Item = Candidate;

    fn next(&mut self) -> Option<Candidate> {
        let Reverse((cost, ranks)) = self.heap.pop()?;
        for (i, slot) in self.slots.iter().enumerate() {
            let r = usize::from(ranks[i]);
            if r + 1 >= slot.ranked.len() {
                continue;
            }
            let mut next = ranks.clone();
            next[i] += 1;
            if self.seen.insert(next.clone()) {
                let next_cost =
                    cost - u64::from(slot.ranked[r].0) + u64::from(slot.ranked[r + 1].0);
                self.heap.push(Reverse((next_cost, next)));
            }
        }
        let words = ranks
            .iter()
            .zip(self.slots)
            .map(|(&r, slot)| slot.ranked[usize::from(r)].1)
            .collect();
        Some(Candidate { words, cost, ranks })
    }
}

/// An unsigned integer wide enough for `2048¹² = 2¹³²` phrases. Limbs
/// are little-endian.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct U192([u64; 3]);

impl PartialOrd for U192 {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for U192 {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.iter().rev().cmp(other.0.iter().rev())
    }
}

impl U192 {
    /// Zero.
    pub const ZERO: U192 = U192([0; 3]);

    /// `self · m`; panics on overflow.
    pub fn mul_u64(self, m: u64) -> U192 {
        let mut out = [0u64; 3];
        let mut carry = 0u128;
        for (slot, limb) in out.iter_mut().zip(self.0) {
            let v = u128::from(limb) * u128::from(m) + carry;
            *slot = v as u64;
            carry = v >> 64;
        }
        assert_eq!(carry, 0, "U192 overflow");
        U192(out)
    }

    /// As `u128`, if it fits.
    pub fn to_u128(self) -> Option<u128> {
        (self.0[2] == 0).then(|| u128::from(self.0[1]) << 64 | u128::from(self.0[0]))
    }

    /// Approximate value as a float.
    pub fn to_f64(self) -> f64 {
        self.0[2] as f64 * 2f64.powi(128) + self.0[1] as f64 * 2f64.powi(64) + self.0[0] as f64
    }
}

impl std::ops::Add for U192 {
    type Output = U192;

    /// Panics on overflow, which `2¹³²` phrases cannot reach.
    fn add(self, other: U192) -> U192 {
        let mut out = [0u64; 3];
        let mut carry = 0u64;
        for (slot, (a, b)) in out.iter_mut().zip(self.0.into_iter().zip(other.0)) {
            let (s, c1) = a.overflowing_add(b);
            let (s, c2) = s.overflowing_add(carry);
            *slot = s;
            carry = u64::from(c1) + u64::from(c2);
        }
        assert_eq!(carry, 0, "U192 overflow");
        U192(out)
    }
}

impl From<u64> for U192 {
    fn from(v: u64) -> Self {
        U192([v, 0, 0])
    }
}

impl fmt::Display for U192 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.to_u128() {
            Some(v) => write!(f, "{v}"),
            None => write!(f, "{:.3e}", self.to_f64()),
        }
    }
}

/// Where a phrase sits in the enumeration, without enumerating: `lower =
/// 1 + #{phrases cheaper}`, `upper = #{phrases no dearer}`; the actual
/// position is within, and the two differ only by exact ties. Exact
/// integer counts from convolving the slots' cost histograms.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RankBounds {
    /// Position at best.
    pub lower: U192,
    /// Position at worst.
    pub upper: U192,
}

/// Why a phrase has no rank under a set of slots.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RankError {
    /// Not one word per slot.
    WordCount {
        /// Slots.
        expected: usize,
        /// Words given.
        got: usize,
    },
    /// The word is not in its slot's ranking.
    Unlisted {
        /// Zero-based slot.
        slot: usize,
        /// The word.
        word: Word,
    },
}

impl fmt::Display for RankError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WordCount { expected, got } => {
                write!(f, "{got} words for {expected} slots")
            }
            Self::Unlisted { slot, word } => write!(f, "word {}: {word} is not ranked", slot + 1),
        }
    }
}

impl std::error::Error for RankError {}

/// The rank bounds of `words` (one per slot) under `slots`.
pub fn rank_bounds(slots: &[SlotRanking], words: &[Word]) -> Result<RankBounds, RankError> {
    if words.len() != slots.len() {
        return Err(RankError::WordCount {
            expected: slots.len(),
            got: words.len(),
        });
    }
    let mut target = 0u64;
    for (slot, (s, &w)) in slots.iter().zip(words).enumerate() {
        target += u64::from(s.cost_of(w).ok_or(RankError::Unlisted { slot, word: w })?);
    }
    let base: u64 = slots.iter().map(|s| u64::from(s.ranked[0].0)).sum();
    // Every phrase costs at least `base`; only relative costs up to the
    // target's can matter, which keeps the tables small for a truth the
    // model liked.
    let span = (target - base) as usize;
    let mut counts = vec![U192::ZERO; span + 1];
    counts[0] = U192::from(1);
    for slot in slots {
        let min = slot.ranked[0].0;
        let mut histogram: BTreeMap<usize, u64> = BTreeMap::new();
        for &(c, _) in &slot.ranked {
            let rel = (c - min) as usize;
            if rel > span {
                break;
            }
            *histogram.entry(rel).or_insert(0) += 1;
        }
        let mut next = vec![U192::ZERO; span + 1];
        for (total, &count) in counts.iter().enumerate() {
            if count == U192::ZERO {
                continue;
            }
            for (&rel, &n) in histogram.range(..=span - total) {
                next[total + rel] = next[total + rel] + count.mul_u64(n);
            }
        }
        counts = next;
    }
    let cheaper = counts[..span].iter().fold(U192::ZERO, |acc, &c| acc + c);
    Ok(RankBounds {
        lower: cheaper + U192::from(1),
        upper: cheaper + counts[span],
    })
}

/// A free, local screen applied before the oracle is asked.
pub trait Filter {
    /// Whether `words` may be put to the oracle.
    fn passes(&self, words: &[Word]) -> bool;
    /// For reports.
    fn name(&self) -> String;
}

/// BIP39's own checksum: rejects a wrong phrase with probability about
/// 15/16 at 12 words, so it is a filter, not an oracle.
pub struct Bip39Checksum;

impl Filter for Bip39Checksum {
    fn passes(&self, words: &[Word]) -> bool {
        bip39_checksum_ok(words)
    }
    fn name(&self) -> String {
        "BIP39 checksum".into()
    }
}

/// An authority on whether a phrase is the one that was backed up.
pub trait Oracle {
    /// Whether `words` is it.
    fn accepts(&self, words: &[Word]) -> bool;
    /// For reports.
    fn name(&self) -> String;
}

/// The bench's and tests' oracle: it knows the phrase.
pub struct Known(pub Vec<Word>);

impl Oracle for Known {
    fn accepts(&self, words: &[Word]) -> bool {
        words == self.0.as_slice()
    }
    fn name(&self) -> String {
        "known phrase".into()
    }
}

/// How a search ended.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Outcome {
    /// The oracle accepted a phrase.
    Accepted {
        /// The phrase.
        words: Vec<Word>,
        /// Its position in the enumeration, from 1.
        rank: u64,
        /// Oracle queries spent, this one included.
        queries: usize,
        /// The oracle's name.
        oracle: String,
    },
    /// No oracle: the first phrase every filter passed.
    Unconfirmed {
        /// The phrase.
        words: Vec<Word>,
        /// Its position in the enumeration, from 1.
        rank: u64,
        /// Cheaper phrases a filter rejected; when this is not zero the
        /// phrase is only as trustworthy as the model's ordering.
        skipped: u64,
    },
    /// The query budget ran out.
    Exhausted {
        /// Oracle queries spent.
        queries: usize,
        /// The cheapest phrase, unconfirmed.
        best: Vec<Word>,
    },
}

/// Enumerates phrases over `slots`, screens each with `filters`, and puts
/// survivors to `oracle` — at most `budget` of them, returning
/// [`Outcome::Exhausted`] the moment the budget is spent (at once when
/// it is zero), so a filter that rejects almost everything cannot keep
/// the enumeration running. Without an oracle the first survivor is
/// returned as unconfirmed.
pub fn search(
    slots: &[SlotRanking],
    filters: &[&dyn Filter],
    oracle: Option<&dyn Oracle>,
    budget: usize,
) -> Outcome {
    let mut enumeration = Enumeration::new(slots);
    let best = enumeration.next().map(|c| c.words).unwrap_or_default();
    let mut queries = 0;
    let mut skipped = 0u64;
    if oracle.is_some() && budget == 0 {
        return Outcome::Exhausted { queries, best };
    }
    for (index, candidate) in Enumeration::new(slots).enumerate() {
        let rank = index as u64 + 1;
        if !filters.iter().all(|f| f.passes(&candidate.words)) {
            skipped += 1;
            continue;
        }
        let Some(oracle) = oracle else {
            return Outcome::Unconfirmed {
                words: candidate.words,
                rank,
                skipped,
            };
        };
        queries += 1;
        if oracle.accepts(&candidate.words) {
            return Outcome::Accepted {
                words: candidate.words,
                rank,
                queries,
                oracle: oracle.name(),
            };
        }
        if queries == budget {
            break;
        }
    }
    Outcome::Exhausted { queries, best }
}

/// Rankings for every word from the combined per-position distributions
/// of two cards, with the user's settled words fixed.
pub fn slots(
    combined: &[[Distribution; WORD_LEN]],
    fixed: &[Option<Word>],
    floor: Floor,
) -> Vec<SlotRanking> {
    combined
        .iter()
        .enumerate()
        .map(|(i, q)| match fixed.get(i).copied().flatten() {
            Some(word) => SlotRanking::fixed(word),
            None => SlotRanking::from_distributions(q, floor),
        })
        .collect()
}

/// One hypothesis about two strips' indices that recovery can act on:
/// recovery uses only the difference `i_b − i_a`, so every assignment
/// with the same difference reconstructs identically and shares a class.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Class {
    /// The difference `i_b − i_a` in GF(29), as a symbol.
    pub difference: Symbol,
    /// Every `(index_a, index_b)` consistent with what was read that
    /// produces this difference.
    pub assignments: Vec<(ShareIndex, ShareIndex)>,
}

impl Class {
    /// A representative assignment.
    pub fn representative(&self) -> (ShareIndex, ShareIndex) {
        self.assignments[0]
    }
}

/// The classes consistent with what was read of each strip's index:
/// one class when both are read, two when one is, four when neither.
pub fn classes(read_a: Option<ShareIndex>, read_b: Option<ShareIndex>) -> Vec<Class> {
    let mut classes: Vec<Class> = Vec::new();
    for a in ShareIndex::all() {
        for b in ShareIndex::all() {
            if a == b || read_a.is_some_and(|r| r != a) || read_b.is_some_and(|r| r != b) {
                continue;
            }
            let difference = b.symbol() - a.symbol();
            match classes.iter_mut().find(|c| c.difference == difference) {
                Some(class) => class.assignments.push((a, b)),
                None => classes.push(Class {
                    difference,
                    assignments: vec![(a, b)],
                }),
            }
        }
    }
    classes
}

/// A search over several reconstruction classes at once.
pub struct Merged<'a> {
    streams: Vec<std::iter::Peekable<Enumeration<'a>>>,
}

impl<'a> Merged<'a> {
    /// One enumeration per class's slots.
    pub fn new(per_class: &'a [Vec<SlotRanking>]) -> Merged<'a> {
        Merged {
            streams: per_class
                .iter()
                .map(|slots| Enumeration::new(slots).peekable())
                .collect(),
        }
    }
}

/// A phrase yielded by a merged enumeration, with the class it came from.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MergedCandidate {
    /// The phrase and its cost under the class.
    pub candidate: Candidate,
    /// Index of the class among those merged.
    pub class: usize,
}

/// Yields phrases from all classes in non-decreasing cost, ties by class
/// then rank vector. The same phrase can surface from several classes;
/// [`search_classes`] keeps each to one filter pass and one oracle query.
impl Iterator for Merged<'_> {
    type Item = MergedCandidate;

    fn next(&mut self) -> Option<MergedCandidate> {
        let mut best: Option<(u64, usize)> = None;
        for (class, stream) in self.streams.iter_mut().enumerate() {
            if let Some(c) = stream.peek() {
                let key = (c.cost, class);
                if best.is_none_or(|b| key < b) {
                    best = Some(key);
                }
            }
        }
        let (_, class) = best?;
        let candidate = self.streams[class].next()?;
        Some(MergedCandidate { candidate, class })
    }
}

/// How a search over classes ended: the phrase is the oracle's finding;
/// the class is the model's, the cheapest reconstruction the phrase came
/// from, with its cost under every class so the caller can see how far
/// the others were.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ClassOutcome {
    /// The oracle accepted a phrase.
    Accepted {
        /// The phrase.
        words: Vec<Word>,
        /// The class it first surfaced from: a model inference.
        class: usize,
        /// The phrase's cost under every class, in class order.
        costs: Vec<u64>,
        /// Its position in the merged enumeration, from 1.
        rank: u64,
        /// Oracle queries spent, this one included.
        queries: usize,
        /// The oracle's name.
        oracle: String,
    },
    /// No oracle: the first phrase every filter passed.
    Unconfirmed {
        /// The phrase.
        words: Vec<Word>,
        /// The class it first surfaced from.
        class: usize,
        /// The phrase's cost under every class.
        costs: Vec<u64>,
        /// Its position in the merged enumeration, from 1.
        rank: u64,
        /// Cheaper distinct phrases a filter rejected.
        skipped: u64,
    },
    /// The query budget ran out.
    Exhausted {
        /// Oracle queries spent.
        queries: usize,
        /// The cheapest phrase across classes, unconfirmed.
        best: Vec<Word>,
        /// The class it came from.
        class: usize,
    },
}

/// The classes at which `costs` is minimal: the cheapest reconstruction
/// and every class tied with it. A phrase's emitting class is only an
/// inference when this is a singleton.
pub fn cheapest_classes(costs: &[u64]) -> Vec<usize> {
    let Some(&min) = costs.iter().min() else {
        return Vec::new();
    };
    costs
        .iter()
        .enumerate()
        .filter(|&(_, &c)| c == min)
        .map(|(i, _)| i)
        .collect()
}

/// [`search`] over several classes: one merged enumeration, a global set
/// of phrases already seen so a phrase is filtered and queried once
/// however many classes surface it, one shared oracle budget.
pub fn search_classes(
    per_class: &[Vec<SlotRanking>],
    filters: &[&dyn Filter],
    oracle: Option<&dyn Oracle>,
    budget: usize,
) -> ClassOutcome {
    let costs_of = |words: &[Word]| -> Vec<u64> {
        per_class
            .iter()
            .map(|slots| {
                slots
                    .iter()
                    .zip(words)
                    .map(|(s, &w)| s.cost_of(w).map_or(u64::MAX, u64::from))
                    .sum()
            })
            .collect()
    };
    let first = Merged::new(per_class)
        .next()
        .expect("every class has a cheapest phrase");
    let (best, best_class) = (first.candidate.words, first.class);
    let mut queries = 0;
    let mut skipped = 0u64;
    let mut rank = 0u64;
    let mut seen: HashSet<Vec<Word>> = HashSet::new();
    if oracle.is_some() && budget == 0 {
        return ClassOutcome::Exhausted {
            queries,
            best,
            class: best_class,
        };
    }
    for MergedCandidate { candidate, class } in Merged::new(per_class) {
        if !seen.insert(candidate.words.clone()) {
            continue;
        }
        rank += 1;
        if !filters.iter().all(|f| f.passes(&candidate.words)) {
            skipped += 1;
            continue;
        }
        let Some(oracle) = oracle else {
            let costs = costs_of(&candidate.words);
            return ClassOutcome::Unconfirmed {
                words: candidate.words,
                class,
                costs,
                rank,
                skipped,
            };
        };
        queries += 1;
        if oracle.accepts(&candidate.words) {
            let costs = costs_of(&candidate.words);
            return ClassOutcome::Accepted {
                words: candidate.words,
                class,
                costs,
                rank,
                queries,
                oracle: oracle.name(),
            };
        }
        if queries == budget {
            break;
        }
    }
    ClassOutcome::Exhausted {
        queries,
        best,
        class: best_class,
    }
}
