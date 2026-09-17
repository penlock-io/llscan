//! Plane-to-plane projective mappings solved in `f64`.
//!
//! `imageproc`'s `Projection::from_control_points` solves in `f32` without
//! normalising its inputs and, with millimetre sources against
//! thousand-pixel targets, can miss the control points themselves by tens
//! of pixels. Everything here is solved with normalised coordinates in
//! `f64`; `imageproc` only ever receives the finished matrix, for warping.

use imageproc::geometric_transformations::Projection;

/// A point and where it maps to.
pub type Correspondence = ((f64, f64), (f64, f64));

/// A 3 × 3 projective matrix, row-major, mapping `(x, y, 1)` to
/// `(x', y', w')`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Homography([f64; 9]);

impl Homography {
    /// The mapping that takes each `from[i]` exactly to `to[i]`, or
    /// `None` if either quadrilateral is degenerate.
    pub fn from_points(from: [(f64, f64); 4], to: [(f64, f64); 4]) -> Option<Homography> {
        let (t_from, from_n) = normalise(from)?;
        let (t_to, to_n) = normalise(to)?;
        // Direct linear transform with h33 fixed at 1: eight equations
        // in the other eight entries.
        let mut a = [[0.0f64; 9]; 8];
        for (i, ((x, y), (u, v))) in from_n.iter().zip(to_n).enumerate() {
            a[2 * i] = [*x, *y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y, u];
            a[2 * i + 1] = [0.0, 0.0, 0.0, *x, *y, 1.0, -v * x, -v * y, v];
        }
        let h = solve(a)?;
        let normalised = Homography([h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7], 1.0]);
        Some(t_to.inverse()?.compose(&normalised).compose(&t_from))
    }

    /// The least-squares mapping over any number of `(from, to)` pairs
    /// (at least four, not all collinear), or `None` if degenerate.
    pub fn fit(pairs: &[Correspondence]) -> Option<Homography> {
        if pairs.len() < 4 {
            return None;
        }
        let from: Vec<(f64, f64)> = pairs.iter().map(|p| p.0).collect();
        let to: Vec<(f64, f64)> = pairs.iter().map(|p| p.1).collect();
        let (t_from, from_n) = normalise_all(&from)?;
        let (t_to, to_n) = normalise_all(&to)?;
        // Normal equations of the direct linear transform with h33 = 1.
        let mut ata = [[0.0f64; 9]; 8];
        let mut add = |row: [f64; 8], rhs: f64| {
            for i in 0..8 {
                for j in 0..8 {
                    ata[i][j] += row[i] * row[j];
                }
                ata[i][8] += row[i] * rhs;
            }
        };
        for ((x, y), (u, v)) in from_n.iter().zip(&to_n) {
            add([*x, *y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y], *u);
            add([0.0, 0.0, 0.0, *x, *y, 1.0, -v * x, -v * y], *v);
        }
        let h = solve(ata)?;
        let normalised = Homography([h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7], 1.0]);
        Some(t_to.inverse()?.compose(&normalised).compose(&t_from))
    }

    /// Applies the mapping.
    pub fn map(&self, x: f64, y: f64) -> (f64, f64) {
        let m = &self.0;
        let w = m[6] * x + m[7] * y + m[8];
        (
            (m[0] * x + m[1] * y + m[2]) / w,
            (m[3] * x + m[4] * y + m[5]) / w,
        )
    }

    /// The reverse mapping, or `None` if singular.
    pub fn inverse(&self) -> Option<Homography> {
        let m = &self.0;
        let cofactor = [
            m[4] * m[8] - m[5] * m[7],
            m[2] * m[7] - m[1] * m[8],
            m[1] * m[5] - m[2] * m[4],
            m[5] * m[6] - m[3] * m[8],
            m[0] * m[8] - m[2] * m[6],
            m[2] * m[3] - m[0] * m[5],
            m[3] * m[7] - m[4] * m[6],
            m[1] * m[6] - m[0] * m[7],
            m[0] * m[4] - m[1] * m[3],
        ];
        let det = m[0] * cofactor[0] + m[1] * cofactor[3] + m[2] * cofactor[6];
        if det.abs() < 1e-12 {
            return None;
        }
        Some(Homography(cofactor.map(|c| c / det)))
    }

    /// `self ∘ other`: applies `other` first.
    pub fn compose(&self, other: &Homography) -> Homography {
        let (a, b) = (&self.0, &other.0);
        let mut out = [0.0; 9];
        for r in 0..3 {
            for c in 0..3 {
                out[3 * r + c] = (0..3).map(|k| a[3 * r + k] * b[3 * k + c]).sum();
            }
        }
        Homography(out)
    }

    /// The same mapping as an `imageproc` projection, for warping.
    pub fn to_projection(&self) -> Projection {
        Projection::from_matrix(self.0.map(|v| v as f32))
            .expect("a solved homography is invertible")
    }
}

fn normalise_all(points: &[(f64, f64)]) -> Option<(Homography, Vec<(f64, f64)>)> {
    let n = points.len() as f64;
    let cx = points.iter().map(|p| p.0).sum::<f64>() / n;
    let cy = points.iter().map(|p| p.1).sum::<f64>() / n;
    let mean = points
        .iter()
        .map(|p| ((p.0 - cx).powi(2) + (p.1 - cy).powi(2)).sqrt())
        .sum::<f64>()
        / n;
    if mean < 1e-9 {
        return None;
    }
    let s = std::f64::consts::SQRT_2 / mean;
    let t = Homography([s, 0.0, -s * cx, 0.0, s, -s * cy, 0.0, 0.0, 1.0]);
    Some((
        t,
        points
            .iter()
            .map(|(x, y)| ((x - cx) * s, (y - cy) * s))
            .collect(),
    ))
}

/// Translates a point set's centroid to the origin and scales its mean
/// distance from it to √2; returns the transform and the moved points.
fn normalise(points: [(f64, f64); 4]) -> Option<(Homography, [(f64, f64); 4])> {
    let cx = points.iter().map(|p| p.0).sum::<f64>() / 4.0;
    let cy = points.iter().map(|p| p.1).sum::<f64>() / 4.0;
    let mean = points
        .iter()
        .map(|p| ((p.0 - cx).powi(2) + (p.1 - cy).powi(2)).sqrt())
        .sum::<f64>()
        / 4.0;
    if mean < 1e-9 {
        return None;
    }
    let s = std::f64::consts::SQRT_2 / mean;
    let t = Homography([s, 0.0, -s * cx, 0.0, s, -s * cy, 0.0, 0.0, 1.0]);
    Some((t, points.map(|(x, y)| ((x - cx) * s, (y - cy) * s))))
}

/// Solves `a[..8] · h = a[8]` by Gaussian elimination with partial
/// pivoting; `None` if the system is singular.
fn solve(mut a: [[f64; 9]; 8]) -> Option<[f64; 8]> {
    for col in 0..8 {
        let pivot = (col..8).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[pivot][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, pivot);
        let pivot_row = a[col];
        for (row, entries) in a.iter_mut().enumerate() {
            if row == col {
                continue;
            }
            let factor = entries[col] / pivot_row[col];
            for (entry, p) in entries.iter_mut().zip(pivot_row).skip(col) {
                *entry -= factor * p;
            }
        }
    }
    let mut h = [0.0; 8];
    for (i, row) in a.iter().enumerate() {
        h[i] = row[8] / row[i];
    }
    Some(h)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEMPLATE: [(f64, f64); 4] = [(13.0, 13.0), (127.0, 13.0), (13.0, 187.0), (127.0, 187.0)];

    fn close(a: (f64, f64), b: (f64, f64), tol: f64) -> bool {
        (a.0 - b.0).abs() < tol && (a.1 - b.1).abs() < tol
    }

    #[test]
    fn control_points_round_trip_exactly() {
        let photo = [
            (291.74988723500223, 126.54578258908435),
            (1066.2777777777778, 143.67471819645732),
            (206.07050209205022, 1311.4085774058578),
            (997.737811685895, 1375.8974692966133),
        ];
        let h = Homography::from_points(TEMPLATE, photo).unwrap();
        for (f, t) in TEMPLATE.iter().zip(photo) {
            assert!(
                close(h.map(f.0, f.1), t, 1e-6),
                "{f:?} -> {:?}",
                h.map(f.0, f.1)
            );
        }
        let mark = h.map(27.0, 11.0);
        assert!(close(mark, (384.9, 115.6), 0.5), "{mark:?}");
        let inv = h.inverse().unwrap();
        for (f, t) in TEMPLATE.iter().zip(photo) {
            assert!(close(inv.map(t.0, t.1), *f, 1e-6));
        }
        let p = h.to_projection();
        let (x, y) = p * (27.0f32, 11.0);
        assert!(close((f64::from(x), f64::from(y)), mark, 0.01));
    }

    #[test]
    fn identity_and_degenerate() {
        let h = Homography::from_points(TEMPLATE, TEMPLATE).unwrap();
        assert!(close(h.map(50.0, 60.0), (50.0, 60.0), 1e-9));
        let collinear = [(0.0, 0.0), (1.0, 1.0), (2.0, 2.0), (3.0, 3.0)];
        assert!(Homography::from_points(TEMPLATE, collinear).is_none());
    }
}
