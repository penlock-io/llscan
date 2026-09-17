//! Shared page-wide compact-ink proposals for label-grid fitting.
use image::{GrayImage, Luma};
use imageproc::{
    contrast::adaptive_threshold,
    filter::gaussian_blur_f32,
    region_labelling::{Connectivity, connected_components},
};
#[doc(hidden)]
pub mod isolation;
#[doc(hidden)]
pub mod tracks;
#[derive(Clone, Debug)]
pub struct Blob {
    pub bounds: [u32; 4],
    pub pixels: u32,
    pub origin: &'static str,
}

fn components(mask: &GrayImage, height: f32) -> Vec<Blob> {
    let labels = connected_components(mask, Connectivity::Eight, Luma([255]));
    let mut blobs: Vec<Blob> = Vec::new();
    for (x, y, p) in labels.enumerate_pixels() {
        let id = p[0] as usize;
        if id == 0 {
            continue;
        }
        if blobs.len() < id {
            blobs.resize(
                id,
                Blob {
                    bounds: [u32::MAX, u32::MAX, 0, 0],
                    pixels: 0,
                    origin: "raw_component",
                },
            );
        }
        let b = &mut blobs[id - 1];
        b.bounds[0] = b.bounds[0].min(x);
        b.bounds[1] = b.bounds[1].min(y);
        b.bounds[2] = b.bounds[2].max(x + 1);
        b.bounds[3] = b.bounds[3].max(y + 1);
        b.pixels += 1;
    }
    blobs.retain(|b| {
        if b.pixels == 0 {
            return false;
        }
        let w = (b.bounds[2] - b.bounds[0]) as f32;
        let h = (b.bounds[3] - b.bounds[1]) as f32;
        w >= 0.035 * height
            && h >= 0.035 * height
            && w <= 0.4 * height
            && h <= 0.4 * height
            && (0.35..=3.).contains(&(w / h))
            && b.pixels as f32 >= 3_f32.max(0.002 * height * height)
            && b.pixels as f32 / (w * h) >= 0.3
    });
    blobs
}

pub fn dots(gray: &GrayImage, height: f32) -> (GrayImage, GrayImage, Vec<Blob>) {
    let mask = adaptive_threshold(gray, (height * 0.6).round().max(1.) as u32, 12);
    let mut blobs = components(&mask, height);
    let (response, peaks) = dark_peaks(gray, height);
    for b in peaks {
        if blobs.iter().any(|a| {
            a.bounds[0] < b.bounds[2]
                && a.bounds[2] > b.bounds[0]
                && a.bounds[1] < b.bounds[3]
                && a.bounds[3] > b.bounds[1]
        }) {
            continue;
        }
        blobs.push(b);
    }
    (mask, response, blobs)
}

// A dark spot curves upward in both axes. A long ruling line curves in only
// one. The smaller Hessian eigenvalue therefore keeps compact darkness even
// when a dot is connected to a line; no connected-component separation needed.
fn dark_peaks(gray: &GrayImage, height: f32) -> (GrayImage, Vec<Blob>) {
    let sigma = (0.035 * height).max(1.);
    let blur = gaussian_blur_f32(gray, sigma);
    let step = sigma.round().max(1.) as u32;
    let radius = (2. * sigma).ceil() as u32;
    let mut heat = GrayImage::new(gray.width(), gray.height());
    let mut peaks = Vec::new();
    let sample = |x, y| f32::from(blur.get_pixel(x, y)[0]);
    for y in step..gray.height().saturating_sub(step) {
        for x in step..gray.width().saturating_sub(step) {
            let centre = sample(x, y);
            let xx = sample(x - step, y) + sample(x + step, y) - 2. * centre;
            let yy = sample(x, y - step) + sample(x, y + step) - 2. * centre;
            let xy = (sample(x + step, y + step) + sample(x - step, y - step)
                - sample(x - step, y + step)
                - sample(x + step, y - step))
                / 4.;
            let value = ((xx + yy) - ((xx - yy).powi(2) + 4. * xy * xy).sqrt()) / 2.;
            heat.put_pixel(x, y, Luma([(value.max(0.) * 16.).min(255.) as u8]));
            if value >= 3. {
                peaks.push((value, x, y));
            }
        }
    }
    peaks.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut found: Vec<Blob> = Vec::new();
    for (_, x, y) in peaks {
        if found.iter().any(|b| {
            let cx = (b.bounds[0] + b.bounds[2]) / 2;
            let cy = (b.bounds[1] + b.bounds[3]) / 2;
            x.abs_diff(cx) <= 2 * radius && y.abs_diff(cy) <= 2 * radius
        }) {
            continue;
        }
        found.push(Blob {
            bounds: [
                x.saturating_sub(radius),
                y.saturating_sub(radius),
                (x + radius + 1).min(gray.width()),
                (y + radius + 1).min(gray.height()),
            ],
            pixels: 0,
            origin: "compact_dark_peak",
        });
    }
    (heat, found)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn heatmap_finds_a_dot_attached_to_ruling_without_calling_the_line_a_dot() {
        let mut photo = GrayImage::from_pixel(180, 150, Luma([220]));
        for x in 10..170 {
            for y in 98..100 {
                photo.put_pixel(x, y, Luma([40]));
            }
        }
        for x in 77..85 {
            for y in 92..100 {
                photo.put_pixel(x, y, Luma([40]));
            }
        }
        for (image, cx, cy) in [
            (&photo, 81, 96),
            (&image::imageops::rotate90(&photo), 53, 81),
        ] {
            let (_, peaks) = dark_peaks(image, 60.);
            assert!(peaks.iter().any(|p| p.bounds[0] <= cx
                && p.bounds[2] > cx
                && p.bounds[1] <= cy
                && p.bounds[3] > cy));
        }
        let (_, peaks) = dark_peaks(&photo, 60.);
        assert!(!peaks.iter().any(|p| p.bounds[0] > 110 && p.bounds[2] < 150));
    }
    #[test]
    fn detached_dot_is_not_collapsed_with_stroke_above_it() {
        let mut photo = GrayImage::from_pixel(180, 150, Luma([220]));
        for y in 20..65 {
            for x in 70..78 {
                photo.put_pixel(x, y, Luma([40]));
            }
        }
        for y in 90..98 {
            for x in 70..78 {
                photo.put_pixel(x, y, Luma([40]));
            }
        }
        let (_, _, found) = dots(&photo, 60.);
        assert!(found.iter().any(|b| b.bounds == [70, 90, 78, 98]));
        assert!(!found.iter().any(|b| b.bounds == [70, 20, 78, 65]));
        assert!(
            dots(&GrayImage::from_pixel(180, 150, Luma([220])), 60.)
                .2
                .is_empty()
        );
    }
}
