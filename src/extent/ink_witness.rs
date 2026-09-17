//! E2 numeric evidence from the one existing component-label raster.
//! Neither a size class nor seeding is a semantic claim of safe ownership.

use super::{Component, Quad, Raster, Rect, Value, contains, json};

pub(super) fn area_floor(scale: f32) -> f64 {
    let side = 0.05 * f64::from(scale);
    side * side
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Hit {
    whole_photo: Option<Rect>,
    nominal: u32,
    rounded: u32,
    union: u32,
    roi: Option<[u32; 4]>,
    photo: Option<Rect>,
    sample: Option<(f32, f32)>,
}

fn include_photo(bounds: &mut Option<Rect>, p: (f32, f32)) {
    if let Some(bounds) = bounds {
        bounds.include(p);
    } else {
        *bounds = Some(Rect([p.0, p.1, p.0, p.1]));
    }
}

#[derive(Debug, PartialEq)]
struct Observed {
    id: usize,
    seeded: bool,
    pixels: u32,
    bounds: [u32; 4],
    border: bool,
    hit: Hit,
}

/// Only counts, coordinates and flags survive past geometry, never masks.
#[derive(Debug, PartialEq)]
pub(super) struct Witness {
    scale: f32,
    floor: f64,
    nominal: Quad,
    rounded: Quad,
    complete: bool,
    components: Vec<Observed>,
}

impl Witness {
    pub(super) fn len(&self) -> usize {
        self.components.len()
    }

    pub(super) fn buffer_bytes(&self) -> usize {
        self.components.capacity() * std::mem::size_of::<Observed>()
    }

    pub(super) fn collect(
        raster: &Raster,
        scale: f32,
        labels: &[u32],
        components: &[Component],
        nominal: &Quad,
        rounded: &Quad,
        complete: bool,
    ) -> Self {
        assert_eq!(labels.len(), raster.w as usize * raster.h as usize);
        let mut hits = vec![Hit::default(); components.len()];
        // Finish the same label-raster pass that formerly stopped at the first
        // unseeded hit. Include whole-component photo bounds as well as hits.
        for (index, &id) in labels.iter().enumerate() {
            if id == 0 {
                continue;
            }
            let (x, y) = (index as u32 % raster.w, index as u32 / raster.w);
            let p = raster.point(x as f32 + 0.5, y as f32 + 0.5);
            let hit = &mut hits[id as usize - 1];
            include_photo(&mut hit.whole_photo, p);
            let in_nominal = contains(nominal, p);
            let in_rounded = contains(rounded, p);
            if !in_nominal && !in_rounded {
                continue;
            }
            hit.nominal += u32::from(in_nominal);
            hit.rounded += u32::from(in_rounded);
            hit.union += 1;
            if let Some([l, t, r, b]) = &mut hit.roi {
                *l = (*l).min(x);
                *t = (*t).min(y);
                *r = (*r).max(x + 1);
                *b = (*b).max(y + 1);
            } else {
                hit.roi = Some([x, y, x + 1, y + 1]);
            }
            include_photo(&mut hit.photo, p);
            hit.sample.get_or_insert(p);
        }
        Self {
            scale,
            floor: area_floor(scale),
            nominal: nominal.clone(),
            rounded: rounded.clone(),
            complete,
            components: components
                .iter()
                .zip(hits)
                .enumerate()
                .filter_map(|(index, (c, hit))| {
                    (hit.union > 0).then_some(Observed {
                        id: index + 1,
                        seeded: c.seeded,
                        pixels: c.pixels,
                        bounds: c.bounds,
                        border: c.border,
                        hit,
                    })
                })
                .collect(),
        }
    }

    pub(super) fn has_unowned_ink(&self) -> bool {
        self.components
            .iter()
            .any(|c| !c.seeded && f64::from(c.pixels) >= self.floor)
    }

    pub(super) fn json(&self) -> Value {
        json!({
            "status": "collected",
            "coverage": if self.complete { "complete" } else { "partial" },
            "coverage_scope": "inspected ROI only; not an ink-ownership verdict",
            "support_height": self.scale, "area_floor_pixels": self.floor,
            "nominal_footprint": self.nominal.0, "rounded_footprint": self.rounded.0,
            "roi_bounds_convention": "half-open pixel cells",
            "photo_bounds_convention": "axis-aligned extrema of foreground pixel centres",
            "components": self.components.iter().map(|c| {
                let large = f64::from(c.pixels) >= self.floor;
                json!({
                    "id": c.id, "seeded": c.seeded, "total_pixels": c.pixels,
                    "at_or_above_floor": large,
                    "policy_role": match (c.seeded, large) {
                        (true, true) => "seeded_and_large",
                        (true, false) => "seeded_small",
                        (false, true) => "unseeded_veto_capable",
                        (false, false) => "unseeded_ignored",
                    },
                    "touches_roi_border": c.border,
                    "roi_bounds": c.bounds, "photo_bounds": c.hit.whole_photo.unwrap().0,
                    "nominal_pixels": c.hit.nominal, "rounded_pixels": c.hit.rounded,
                    "union_pixels": c.hit.union,
                    "intersection_roi_bounds": c.hit.roi.unwrap(),
                    "intersection_photo_bounds": c.hit.photo.unwrap().0,
                    "representative_photo_pixel": c.hit.sample.unwrap(),
                })
            }).collect::<Vec<_>>(),
        })
    }
}

/// The same veto as Witness::has_unowned_ink, without Hit/Observed allocations.
/// Count the entire connected component, but require at least one foreground
/// pixel in either read footprint. Bounding-box overlap is not sufficient.
pub(super) fn has_unowned_ink(
    raster: &Raster,
    scale: f32,
    labels: &[u32],
    components: &[Component],
    nominal: &Quad,
    rounded: &Quad,
) -> bool {
    assert_eq!(labels.len(), raster.w as usize * raster.h as usize);
    let floor = area_floor(scale);
    labels.iter().enumerate().any(|(index, &id)| {
        if id == 0 {
            return false;
        }
        let c = &components[id as usize - 1];
        let large = f64::from(c.pixels) >= floor;
        if c.seeded || !large {
            return false;
        }
        let p = raster.point(
            (index as u32 % raster.w) as f32 + 0.5,
            (index as u32 / raster.w) as f32 + 0.5,
        );
        contains(nominal, p) || contains(rounded, p)
    })
}
