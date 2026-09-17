//! Candidate-score arithmetic shared by the finite experiment and selection.
//! Scores rank evidence; they are not calibrated correctness probabilities.
use super::Weights;

#[derive(Clone, Copy, Debug, PartialEq)]
/// Finite experimental score families; the payload is an exponent or edit weight.
pub enum Rule {
    /// log10(p) minus lambda times edit distance.
    Log(f64),
    /// Negative edit distance times (1-p)^n.
    PowerComplement(f64),
    /// Negative edit distance times (1-p^n).
    ComplementPower(f64),
    /// log10(p) minus lambda times squared edit distance.
    SquaredLog(f64),
}

#[derive(Clone, Copy, Debug, PartialEq)]
/// Fixed arithmetic configuration, not a confidence calibration.
pub struct Config {
    /// Probability/edit score family.
    pub rule: Rule,
    /// Weighted edit operations; budget is not a candidate filter here.
    pub weights: Weights,
    /// Divide edit distance by the longer spelling length.
    pub length_normalized: bool,
}

impl Config {
    /// Divisor for the separately declared length-normalization ablation.
    pub fn distance_scale(self, raw_len: usize, word_len: usize) -> f64 {
        if self.length_normalized {
            raw_len.max(word_len).max(1) as f64
        } else {
            1.
        }
    }

    /// Higher wins. Probability zero stays zero, never an epsilon floor.
    pub fn score(self, probability: f32, distance: f32, scale: f64) -> f64 {
        let p = f64::from(probability);
        assert!(p.is_finite() && (0. ..=1.).contains(&p));
        let d = f64::from(distance) / scale;
        match self.rule {
            Rule::Log(lambda) => p.log10() - lambda * d,
            Rule::SquaredLog(lambda) => p.log10() - lambda * d * d,
            Rule::PowerComplement(n) => -d * (1. - p).powf(n),
            Rule::ComplementPower(n) => -d * (1. - p.powf(n)),
        }
    }
}

/// One minimum-cost alignment under the existing edit definition.
/// Excludes unchanged characters; costs sum to `hybrid::distance`.
pub fn edits(raw: &str, word: &str, w: &Weights) -> Vec<serde_json::Value> {
    use serde_json::json;
    let a: Vec<_> = raw.chars().collect();
    let b: Vec<_> = word.chars().collect();
    let mut d = vec![vec![0f32; b.len() + 1]; a.len() + 1];
    for j in 1..=b.len() {
        d[0][j] = w.insert * w.first + (j - 1) as f32 * w.insert;
    }
    for i in 1..=a.len() {
        d[i][0] = w.delete * w.first + (i - 1) as f32 * w.delete;
        let f = if i == 1 { w.first } else { 1. };
        for j in 1..=b.len() {
            d[i][j] = (d[i - 1][j] + w.delete * f)
                .min(d[i][j - 1] + w.insert)
                .min(
                    d[i - 1][j - 1]
                        + if a[i - 1] == b[j - 1] {
                            0.
                        } else {
                            w.substitute * f
                        },
                );
        }
    }
    let (mut i, mut j) = (a.len(), b.len());
    let mut out = Vec::new();
    while i > 0 || j > 0 {
        let f = if i == 1 { w.first } else { 1. };
        let sub = if i > 0 && j > 0 && a[i - 1] == b[j - 1] {
            0.
        } else {
            w.substitute * f
        };
        if i > 0 && j > 0 && d[i][j] == d[i - 1][j - 1] + sub {
            if sub > 0. {
                out.push(json!({"operation":"substitute","raw_index":i-1,"word_index":j-1,"from":a[i-1].to_string(),"to":b[j-1].to_string(),"cost":sub}));
            }
            i -= 1;
            j -= 1;
        } else if i > 0 && d[i][j] == d[i - 1][j] + w.delete * f {
            out.push(json!({"operation":"delete","raw_index":i-1,"from":a[i-1].to_string(),"cost":w.delete*f}));
            i -= 1;
        } else {
            let cost = if i == 0 && j == 1 {
                w.insert * w.first
            } else {
                w.insert
            };
            assert!(j > 0 && d[i][j] == d[i][j - 1] + cost);
            out.push(
                json!({"operation":"insert","word_index":j-1,"to":b[j-1].to_string(),"cost":cost}),
            );
            j -= 1;
        }
    }
    out.reverse();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn powers_are_distinct_and_boundary_probabilities_are_not_floored() {
        let c = Config {
            rule: Rule::PowerComplement(2.),
            weights: super::super::CURRENT,
            length_normalized: false,
        };
        assert_eq!(c.score(0.5, 2., 1.), -0.5);
        assert_eq!(
            Config {
                rule: Rule::ComplementPower(2.),
                ..c
            }
            .score(0.5, 2., 1.),
            -1.5
        );
        assert_eq!(c.score(1., 5., 1.), 0.);
        assert_eq!(c.score(0., 5., 1.), -5.);
        assert_eq!(
            Config {
                rule: Rule::Log(1.),
                ..c
            }
            .score(0., 1., 1.),
            f64::NEG_INFINITY
        );
        assert_eq!(
            Config {
                length_normalized: true,
                ..c
            }
            .distance_scale(3, 6),
            6.
        );
    }

    #[test]
    fn explanations_sum_to_the_authoritative_distance() {
        for raw in ["", "exf", "wn", "sate", "word", "zzzzzzzz"] {
            for word in crate::vocabulary::Word::all() {
                let costs: f64 = edits(raw, word.as_str(), &super::super::CURRENT)
                    .iter()
                    .map(|e| e["cost"].as_f64().unwrap())
                    .sum();
                assert_eq!(
                    costs,
                    f64::from(super::super::distance(
                        raw,
                        word.as_str(),
                        &super::super::CURRENT
                    ))
                );
            }
        }
    }
}
