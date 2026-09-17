//! The page read as a group before its words: the item numbers a hand
//! writes beside its words, found over the whole page and fitted to
//! the run 1…N. A number the recogniser read as a letter is still a
//! label by its place in the run, a number no box carries is a word
//! not found rather than a phrase silently short, and two boxes
//! carrying the same number are a page whose numbering cannot be
//! trusted at all.

use crate::detect::Quad;
use crate::page_frame::WritingFrame;
use crate::split::turn;
use std::collections::{HashMap, HashSet};

pub mod dotted;

pub(crate) const LABEL_ASPECT_LIMIT: f32 = 2.2;

/// Shared shape rule for a standalone printed/handwritten number token.
pub(crate) fn label_shape_evidence(
    quad: &Quad,
    trace: bool,
    writing: WritingFrame,
) -> (bool, Option<serde_json::Value>) {
    let f = writing.frame_of(quad);
    let aspect_limit = LABEL_ASPECT_LIMIT;
    let shaped = f.w < aspect_limit * f.h;
    (shaped, trace.then(|| serde_json::json!({"status":"evaluated", "width":f.w,
        "height":f.h, "aspect_limit":aspect_limit, "comparison":"width < aspect_limit * height", "accepted":shaped})))
}

/// A label in a word's leading position, evaluated along the supplied writing
/// axis. Returns the word's leading coordinate for the numbering owner's
/// nearest-right choice. Fragment protection asks the same question without
/// choosing an owner. `epsilon` accounts only for coordinate transform error.
pub(crate) fn leading_label(
    label: &Quad,
    word: &Quad,
    angle: f32,
    epsilon: f32,
    writing: WritingFrame,
) -> Option<f32> {
    leading_label_evidence(label, word, angle, epsilon, false, writing).0
}

pub(crate) fn leading_label_evidence(
    label: &Quad,
    word: &Quad,
    angle: f32,
    epsilon: f32,
    trace: bool,
    writing: WritingFrame,
) -> (Option<f32>, Option<serde_json::Value>) {
    let (shaped, shape) = label_shape_evidence(label, trace, writing);
    if !shaped {
        return (
            None,
            trace.then(|| {
                serde_json::json!({"shape":shape,
            "placement":{"status":"skipped", "reason":"label_shape_failed"}, "accepted":false})
            }),
        );
    }
    let bounds = |q: &Quad| {
        let points = q.0.map(|(x, y)| turn(x, y, -angle));
        [
            points.iter().map(|p| p.0).fold(f32::INFINITY, f32::min),
            points.iter().map(|p| p.1).fold(f32::INFINITY, f32::min),
            points.iter().map(|p| p.0).fold(f32::NEG_INFINITY, f32::max),
            points.iter().map(|p| p.1).fold(f32::NEG_INFINITY, f32::max),
        ]
    };
    let [_, top, right, bottom] = bounds(label);
    let [x0, y0, _, y1] = bounds(word);
    let (cy, h) = ((top + bottom) / 2.0, bottom - top);
    let (overlap_factor, gap_factor) = (0.2, 2.0);
    let (y_min, y_max) = (y0 - epsilon, y1 + epsilon);
    let (x_min, x_max) = (
        right - overlap_factor * h - epsilon,
        right + gap_factor * h + epsilon,
    );
    let accepted = y_min <= cy && cy <= y_max && x0 >= x_min && x0 <= x_max;
    (accepted.then_some(x0), trace.then(|| serde_json::json!({"shape":shape,
        "placement":{"status":"evaluated", "axis_angle_degrees":angle, "epsilon":epsilon,
            "label_cy":cy, "word_y_min":y_min, "word_y_max":y_max,
            "word_leading_x":x0, "leading_x_min":x_min, "leading_x_max":x_max,
            "label_height":h, "overlap_factor":overlap_factor, "gap_factor":gap_factor,
            "comparison":"word_y_min <= label_cy <= word_y_max and leading_x_min <= word_leading_x <= leading_x_max"},
        "accepted":accepted})))
}

/// What a leading token, or a small box on its own, read as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Label {
    /// Digits alone, with or without a trailing mark: the number read.
    Exact(u32),
    /// A label whose ordinal is unknown: digit-shaped ink in the legacy
    /// grammar, or a dotted head (including punctuation alone) in `dotted`.
    /// Its role does not itself establish a number in the sequence.
    Shaped,
}

/// The characters a handwritten digit reads as when the reader takes
/// it for a letter.
pub(crate) const SHAPED: &str = "lLIiSsGgOoBbZzqADT|";

/// The rest of a read past the label it begins with, when it begins
/// with one: one or two digits or digit-shaped characters, then at
/// least one mark (a dot, a colon, a bracket, a dash) or space, then
/// something. For a number glued to its word with no gap of ink to
/// cut at.
pub fn past_label(read: &str) -> Option<&str> {
    let head: String = read
        .chars()
        .take_while(|c| c.is_ascii_digit() || SHAPED.contains(*c))
        .take(2)
        .collect();
    if head.is_empty() {
        return None;
    }
    let after = &read[head.len()..];
    let rest = after.trim_start_matches(|c: char| ".:)]-_".contains(c) || c.is_whitespace());
    (rest.len() < after.len() && !rest.is_empty()).then_some(rest)
}

/// The label a token reads as, if any: one or two characters that are
/// digits or letters a handwritten digit reads as, then nothing but a
/// mark (a dot, a colon, a bracket, a dash) and space.
pub fn label_of(token: &str) -> Option<Label> {
    let token = token.trim();
    let body = token.trim_end_matches(|c: char| ".:)]-_ ".contains(c) || c.is_whitespace());
    if body.is_empty()
        || body.chars().count() > 2
        || body.chars().any(|c| !c.is_ascii_alphanumeric())
    {
        return None;
    }
    if body.chars().all(|c| c.is_ascii_digit()) {
        return body.parse().ok().filter(|&n| n >= 1).map(Label::Exact);
    }
    body.chars()
        .all(|c| c.is_ascii_digit() || SHAPED.contains(c))
        .then_some(Label::Shaped)
}

/// The numbering the page's labels make.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Numbering {
    /// No trusted ordinal sequence. Dotted evidence can independently
    /// establish list mode without changing this result.
    None,
    /// The labels read as a run: every box's number where it has one,
    /// the count the run reaches, the numbers no box carries, the
    /// numbers whose place in the reading order is not their place in
    /// the run, and the boxes that carry a label the run gave no
    /// number to.
    Held {
        /// Each box's number, where the run gives it one.
        numbers: Vec<Option<u32>>,
        /// The highest number the run reaches.
        count: u32,
        /// The numbers of the run no box carries.
        missing: Vec<u32>,
        /// The numbers whose place in the reading order is not their
        /// place in the run.
        out_of_place: Vec<u32>,
        /// The boxes with a label that took no number: a digit's shape
        /// where the run has no hole for it.
        unresolved: Vec<usize>,
    },
    /// The labels contradict one another, so no number is trusted.
    Inconsistent {
        /// What contradicted what.
        detail: String,
    },
}

impl Numbering {
    /// A trusted sequence, not merely evidence that the page is a list.
    pub fn has_held_sequence(&self) -> bool {
        matches!(self, Self::Held { .. })
    }

    /// The existing page-wide join/E3 guard. Contradictory sequences remain
    /// protected too; an independent list-mode signal must not change this.
    pub fn permits_word_repair(&self) -> bool {
        matches!(self, Self::None)
    }
}

/// How many boxes must read as exact numbers, in order, before a sequence
/// is held. This does not gate the independent dotted-list mode.
pub const MIN_RUN: usize = 8;

/// Fits the labels to the run 1…N. `labels[i]` is box i's label, or
/// `None` for a box with none; `columns` and `rows` are the two
/// reading orders over the boxes that can take a place in the run. A
/// box in neither order takes no part at all — its label is neither
/// a number of the run, nor a duplicate, nor reserved, nor unresolved
/// — so what the region or the keep rule left out cannot reach the
/// numbering by its label. The order with fewer inversions
/// among the exact numbers is the page's, and the longest rising run
/// through them must reach [`MIN_RUN`], or the page is not numbered
/// (too few numbers) or its numbers contradict one another (enough
/// numbers, out of order). Every exact number is reserved, in place
/// or not; the holes left between two numbers in place go to the
/// boxes between them when they are exactly as many, and a shaped
/// label directly after the last number in place continues the run.
pub fn fit(given: &[Option<Label>], columns: &[usize], rows: &[usize]) -> Numbering {
    let placed: HashSet<usize> = columns.iter().chain(rows).copied().collect();
    let labels: Vec<Option<Label>> = given
        .iter()
        .enumerate()
        .map(|(i, l)| if placed.contains(&i) { *l } else { None })
        .collect();
    let labels = labels.as_slice();
    let exact: Vec<(usize, u32)> = labels
        .iter()
        .enumerate()
        .filter_map(|(i, l)| match l {
            Some(Label::Exact(n)) => Some((i, *n)),
            _ => None,
        })
        .collect();
    if exact.len() < MIN_RUN {
        return Numbering::None;
    }
    let mut seen = HashMap::new();
    for &(i, n) in &exact {
        if let Some(first) = seen.insert(n, i) {
            return Numbering::Inconsistent {
                detail: format!("two boxes are numbered {n} (boxes {first} and {i})"),
            };
        }
    }
    let highest = exact.iter().map(|&(_, n)| n).max().unwrap_or(0);
    if highest as usize > 2 * labels.len().max(12) {
        return Numbering::Inconsistent {
            detail: format!("numbered up to {highest} with {} boxes", labels.len()),
        };
    }
    let values_in = |order: &[usize]| -> Vec<u32> {
        order
            .iter()
            .filter_map(|&i| match labels[i] {
                Some(Label::Exact(n)) => Some(n),
                _ => None,
            })
            .collect()
    };
    let inversions = |values: &[u32]| {
        (0..values.len())
            .flat_map(|a| (a + 1..values.len()).map(move |b| (a, b)))
            .filter(|&(a, b)| values[a] > values[b])
            .count()
    };
    let order: Vec<usize> = if inversions(&values_in(rows)) < inversions(&values_in(columns)) {
        rows.to_vec()
    } else {
        columns.to_vec()
    };
    let mut numbers: Vec<Option<u32>> = labels
        .iter()
        .map(|l| match l {
            Some(Label::Exact(n)) => Some(*n),
            _ => None,
        })
        .collect();

    // the exact numbers in reading order; a number that breaks the
    // longest rising run through them is out of place
    let placed: Vec<(usize, u32)> = order
        .iter()
        .filter_map(|&i| numbers[i].map(|n| (i, n)))
        .collect();
    let rising = longest_rising(&placed.iter().map(|&(_, n)| n).collect::<Vec<_>>());
    if rising.len() < MIN_RUN {
        return Numbering::Inconsistent {
            detail: format!(
                "{} numbers read but only {} of them in order",
                exact.len(),
                rising.len()
            ),
        };
    }
    let out_of_place: Vec<u32> = placed
        .iter()
        .enumerate()
        .filter(|(k, _)| !rising.contains(k))
        .map(|(_, &(_, n))| n)
        .collect();
    let reserved: HashSet<u32> = exact.iter().map(|&(_, n)| n).collect();

    // the holes between each pair of numbers in place, and before the
    // first, less any number carried elsewhere on the page: filled by
    // the boxes between them in the reading order when they are
    // exactly as many
    let anchors: Vec<(usize, u32)> = rising.iter().map(|&k| placed[k]).collect();
    let position: HashMap<usize, usize> = order.iter().enumerate().map(|(p, &i)| (i, p)).collect();
    let mut spans: Vec<(usize, usize, u32, u32)> = Vec::new();
    if let Some(&(first, n)) = anchors.first() {
        spans.push((0, position[&first], 0, n));
    }
    for pair in anchors.windows(2) {
        let ((a, na), (b, nb)) = (pair[0], pair[1]);
        spans.push((position[&a] + 1, position[&b], na, nb));
    }
    for (from, to, na, nb) in spans {
        let holes: Vec<u32> = (na + 1..nb).filter(|n| !reserved.contains(n)).collect();
        let between: Vec<usize> = order[from..to]
            .iter()
            .copied()
            .filter(|&i| numbers[i].is_none())
            .collect();
        if !holes.is_empty() && between.len() == holes.len() {
            for (&i, &n) in between.iter().zip(&holes) {
                numbers[i] = Some(n);
            }
        }
    }
    // a label's shape directly after the last number in place is the
    // next number: the run goes on by what is written, not by the
    // count of boxes after it
    let last = anchors.last().map_or(0, |&(_, n)| n);
    let mut count = last.max(highest);
    if let Some(&(last_box, _)) = anchors.last() {
        for (next, &i) in (last + 1..).zip(&order[position[&last_box] + 1..]) {
            if numbers[i].is_some() || labels[i] != Some(Label::Shaped) || reserved.contains(&next)
            {
                break;
            }
            numbers[i] = Some(next);
            count = count.max(next);
        }
    }
    let mut carried = HashSet::new();
    for (i, n) in numbers.iter().enumerate() {
        if let Some(n) = n
            && !carried.insert(*n)
        {
            return Numbering::Inconsistent {
                detail: format!("box {i} would be numbered {n} twice over"),
            };
        }
    }
    let missing: Vec<u32> = (1..=count).filter(|n| !carried.contains(n)).collect();
    let unresolved: Vec<usize> = (0..labels.len())
        .filter(|&i| labels[i].is_some() && numbers[i].is_none())
        .collect();
    Numbering::Held {
        numbers,
        count,
        missing,
        out_of_place,
        unresolved,
    }
}

/// The indices of one longest strictly rising subsequence of `values`.
fn longest_rising(values: &[u32]) -> Vec<usize> {
    let n = values.len();
    let mut best = vec![1usize; n];
    let mut prev = vec![usize::MAX; n];
    for b in 0..n {
        for a in 0..b {
            if values[a] < values[b] && best[a] + 1 > best[b] {
                best[b] = best[a] + 1;
                prev[b] = a;
            }
        }
    }
    let Some(mut k) = (0..n).max_by_key(|&k| (best[k], std::cmp::Reverse(k))) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    loop {
        out.push(k);
        if prev[k] == usize::MAX {
            break;
        }
        k = prev[k];
    }
    out.reverse();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exact(n: u32) -> Option<Label> {
        Some(Label::Exact(n))
    }

    struct Held {
        numbers: Vec<Option<u32>>,
        count: u32,
        missing: Vec<u32>,
        out_of_place: Vec<u32>,
        unresolved: Vec<usize>,
    }

    fn held(numbering: Numbering) -> Held {
        match numbering {
            Numbering::Held {
                numbers,
                count,
                missing,
                out_of_place,
                unresolved,
            } => Held {
                numbers,
                count,
                missing,
                out_of_place,
                unresolved,
            },
            other => panic!("not held: {other:?}"),
        }
    }

    fn straight(n: usize) -> Vec<usize> {
        (0..n).collect()
    }

    #[test]
    fn a_label_is_one_or_two_digits_or_digit_shapes_and_a_mark() {
        assert_eq!(label_of("7."), exact(7));
        assert_eq!(label_of("12"), exact(12));
        assert_eq!(label_of(" 3 ) "), exact(3));
        assert_eq!(label_of("l."), Some(Label::Shaped));
        assert_eq!(label_of("s."), Some(Label::Shaped));
        assert_eq!(label_of("G."), Some(Label::Shaped));
        assert_eq!(label_of("lO"), Some(Label::Shaped));
        assert_eq!(label_of("0."), None);
        assert_eq!(label_of("123"), None);
        assert_eq!(label_of("st"), None);
        assert_eq!(label_of("walk"), None);
        assert_eq!(label_of(""), None);
        assert_eq!(label_of("."), None);
    }

    #[test]
    fn a_glued_label_is_stripped_from_the_read_only_past_a_mark() {
        assert_eq!(past_label("l.dice"), Some("dice"));
        assert_eq!(past_label("s.agree"), Some("agree"));
        assert_eq!(past_label("7.s+ate"), Some("s+ate"));
        assert_eq!(past_label("24.+wice"), Some("+wice"));
        assert_eq!(past_label("12 sunny"), Some("sunny"));
        assert_eq!(past_label("9EDIT"), None);
        assert_eq!(past_label("state"), None);
        assert_eq!(past_label("7."), None);
        assert_eq!(past_label(""), None);
    }

    #[test]
    fn a_run_with_letters_where_digits_were_holds_and_the_letters_take_their_places() {
        // page 19's shape: l.dice s.riot G.corn among exact numbers
        let labels: Vec<Option<Label>> = [
            Some(Label::Shaped),
            exact(2),
            exact(3),
            exact(4),
            Some(Label::Shaped),
            Some(Label::Shaped),
            exact(7),
            exact(8),
            exact(9),
            exact(10),
            exact(11),
            exact(12),
        ]
        .to_vec();
        let rows: Vec<usize> = vec![0, 6, 1, 7, 2, 8, 3, 9, 4, 10, 5, 11];
        let h = held(fit(&labels, &straight(12), &rows));
        assert_eq!(
            h.numbers.iter().map(|n| n.unwrap()).collect::<Vec<_>>(),
            (1..=12).collect::<Vec<_>>()
        );
        assert_eq!(
            (h.count, h.missing, h.out_of_place, h.unresolved),
            (12, vec![], vec![], vec![])
        );
    }

    #[test]
    fn a_number_no_box_carries_is_missing_and_a_box_without_a_label_fills_a_hole_only_alone() {
        let mut labels: Vec<Option<Label>> = (1..=12).map(exact).collect();
        labels[6] = None;
        labels.remove(3);
        let h = held(fit(&labels, &straight(11), &straight(11)));
        assert_eq!((h.count, h.missing), (12, vec![4]));
        assert_eq!(
            h.numbers[5],
            Some(7),
            "the one unlabelled box between 6 and 8 is 7"
        );
        // two unlabelled boxes for one hole: neither takes it
        let mut labels: Vec<Option<Label>> = (1..=12).map(exact).collect();
        labels[6] = None;
        labels.insert(6, None);
        let h = held(fit(&labels, &straight(13), &straight(13)));
        assert_eq!(h.missing, vec![7]);
        assert_eq!((h.numbers[6], h.numbers[7]), (None, None));
        // two shaped labels for one hole: both unresolved, the hole missing
        labels[6] = Some(Label::Shaped);
        labels[7] = Some(Label::Shaped);
        let h = held(fit(&labels, &straight(13), &straight(13)));
        assert_eq!((h.missing, h.unresolved), (vec![7], vec![6, 7]));
    }

    #[test]
    fn a_hole_is_never_a_number_a_box_elsewhere_carries() {
        // 5 written beside the eighth word: the unlabelled fifth box
        // does not become a second 5
        let labels: Vec<Option<Label>> = [
            exact(1),
            exact(2),
            exact(3),
            exact(4),
            None,
            exact(6),
            exact(7),
            exact(5),
            exact(9),
            exact(10),
            exact(11),
            exact(12),
        ]
        .to_vec();
        let h = held(fit(&labels, &straight(12), &straight(12)));
        assert_eq!(h.numbers[4], None);
        assert_eq!(h.numbers[7], Some(5));
        assert_eq!((h.missing, h.out_of_place), (vec![8], vec![5]));
        assert_eq!(h.numbers.iter().flatten().collect::<HashSet<_>>().len(), 11);
    }

    #[test]
    fn a_missing_first_number_is_taken_by_the_one_box_before_the_run() {
        let mut labels: Vec<Option<Label>> = (1..=12).map(exact).collect();
        labels[0] = Some(Label::Shaped);
        let h = held(fit(&labels, &straight(12), &straight(12)));
        assert_eq!(h.numbers[0], Some(1));
        assert!(h.missing.is_empty());
    }

    #[test]
    fn a_shaped_label_after_the_last_number_continues_the_run_and_an_unlabelled_box_does_not() {
        let labels: Vec<Option<Label>> = (1..=11).map(exact).chain([Some(Label::Shaped)]).collect();
        let h = held(fit(&labels, &straight(12), &straight(12)));
        assert_eq!(
            (h.count, h.numbers[11], h.missing, h.unresolved),
            (12, Some(12), vec![], vec![])
        );
        let labels: Vec<Option<Label>> = (1..=11).map(exact).chain([None]).collect();
        let h = held(fit(&labels, &straight(12), &straight(12)));
        assert_eq!((h.count, h.numbers[11]), (11, None));
        assert!(h.missing.is_empty());
    }

    #[test]
    fn the_order_with_fewer_inversions_is_the_page_s_and_a_swapped_pair_is_out_of_place() {
        // numbers written down two columns: column order reads 1..12
        let labels: Vec<Option<Label>> = (1..=12).map(exact).collect();
        let rows: Vec<usize> = vec![0, 6, 1, 7, 2, 8, 3, 9, 4, 10, 5, 11];
        assert!(
            held(fit(&labels, &straight(12), &rows))
                .out_of_place
                .is_empty()
        );
        // the same page with 5 and 6 swapped by the hand
        let mut swapped = labels.clone();
        swapped.swap(4, 5);
        let h = held(fit(&swapped, &straight(12), &rows));
        assert_eq!(h.out_of_place.len(), 1);
        assert!(h.missing.is_empty());
        assert_eq!(h.numbers[4], Some(6));
    }

    #[test]
    fn duplicates_and_numbers_out_of_order_are_inconsistent_and_too_few_are_none() {
        let mut labels: Vec<Option<Label>> = (1..=12).map(exact).collect();
        labels[9] = exact(5);
        let order = straight(12);
        assert!(matches!(
            fit(&labels, &order, &order),
            Numbering::Inconsistent { .. }
        ));
        // eight numbers, but written backwards: no run to trust
        let backwards: Vec<Option<Label>> = (1..=8).rev().map(exact).chain([None; 4]).collect();
        assert!(matches!(
            fit(&backwards, &order, &order),
            Numbering::Inconsistent { .. }
        ));
        let few: Vec<Option<Label>> = (1..=7).map(exact).chain([None; 5]).collect();
        assert_eq!(fit(&few, &order, &order), Numbering::None);
        let none: Vec<Option<Label>> = vec![None; 12];
        assert_eq!(fit(&none, &order, &order), Numbering::None);
        let far: Vec<Option<Label>> = (1..=7)
            .map(exact)
            .chain([exact(60), None, None, None, None])
            .collect();
        assert!(matches!(
            fit(&far, &order, &order),
            Numbering::Inconsistent { .. }
        ));
    }

    #[test]
    fn a_box_in_neither_order_takes_no_part_whatever_its_label() {
        // a run 1..12 and a thirteenth box, left out of both orders,
        // labelled 5: no duplicate, no number, nothing reserved by it
        let mut labels: Vec<Option<Label>> = (1..=12).map(exact).collect();
        labels.push(exact(5));
        let h = held(fit(&labels, &straight(12), &straight(12)));
        assert_eq!(h.numbers[12], None);
        assert_eq!((h.count, h.missing, h.unresolved), (12, vec![], vec![]));
        // the seventh label unread and the box left out labelled 7: the
        // seventh box takes 7 by its place, the box left out nothing
        labels[6] = None;
        labels[12] = exact(7);
        let h = held(fit(&labels, &straight(12), &straight(12)));
        assert_eq!((h.numbers[6], h.numbers[12]), (Some(7), None));
        assert!(h.missing.is_empty());
        // and with the seventh box shaped and unplaceable, 7 is missing,
        // not carried by the box left out
        labels[6] = Some(Label::Shaped);
        labels[5] = None;
        let h = held(fit(&labels, &straight(12), &straight(12)));
        assert_eq!(h.numbers[12], None);
        assert!(!h.numbers.contains(&Some(7)) || h.numbers[6] == Some(7));
    }

    #[test]
    fn a_shaped_token_with_no_hole_at_its_place_is_not_a_label_and_is_surfaced() {
        // "state" split at its s: shaped, between 6 and 7 with no room
        let mut labels: Vec<Option<Label>> = (1..=12).map(exact).collect();
        labels.insert(6, Some(Label::Shaped));
        let h = held(fit(&labels, &straight(13), &straight(13)));
        assert_eq!(h.numbers[6], None);
        assert!(h.missing.is_empty());
        assert_eq!(h.unresolved, vec![6]);
    }
}
