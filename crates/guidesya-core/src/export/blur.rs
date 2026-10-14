//! Burn-in redaction: blur and pixelate rewrite the pixels of a region.
//!
//! This is privacy-critical. It never draws an overlay; it replaces every
//! pixel in the region with a value computed from many pixels, so exported
//! images do not carry the original pixels underneath:
//!
//! - Regions are grown to at least [`MIN_AMOUNT`] × [`MIN_AMOUNT`]
//!   ([`effective_region`], clamped to the image), so a tiny or sub-pixel
//!   rect still mixes 16 pixels.
//! - Amounts below [`MIN_AMOUNT`] are raised to it, so a tiny radius or block
//!   cannot leave text readable.
//! - Pixelate folds a remainder cell narrower than [`MIN_AMOUNT`] into the
//!   previous cell, so no 1×1 or thin cell keeps its own pixels.
//! - Blur of a region narrower than its kernel (`2 × radius + 1`) in either
//!   dimension fills it with its mean instead: a clamped box blur would
//!   barely mix such a region.
//! - A region of a single pixel (only possible in a 1-pixel image) is
//!   filled with neutral grey.
//!
//! Samples come only from inside the (grown) region, so nothing outside it
//! changes. Works on 8-bit RGBA, straight or premultiplied (averaging
//! premultiplied values is the correct blur for transparent pixels).

use crate::model::BlurMode;

/// Smallest blur radius, pixel block size and region side, in image pixels.
pub const MIN_AMOUNT: u32 = 4;
/// Blur radius used when the annotation sets none.
pub const DEFAULT_BLUR_RADIUS: u32 = 12;
/// Pixel block size used when the annotation sets none.
pub const DEFAULT_BLOCK_SIZE: u32 = 10;
/// Box blur passes; three approximate a Gaussian.
const PASSES: usize = 3;

/// An integer pixel region inside the image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Rewrites `region` (grown by [`effective_region`]) of the
/// `width` × `height` RGBA image in `data`.
pub fn burn_in(
    data: &mut [u8],
    (width, height): (u32, u32),
    region: Region,
    mode: BlurMode,
    amount: Option<f64>,
) {
    let Some(r) = effective_region(region, width, height) else {
        return;
    };
    if r.width * r.height == 1 {
        let i = index(width, r.x, r.y);
        let a = u16::from(data[i + 3]);
        let grey = ((128 * a + 127) / 255) as u8;
        data[i..i + 3].fill(grey);
        return;
    }
    match mode {
        BlurMode::Blur => {
            let radius = effective(amount, DEFAULT_BLUR_RADIUS);
            let kernel = 2 * radius + 1;
            if r.width < kernel || r.height < kernel {
                mean_fill(data, width, r);
            } else {
                blur(data, width, r, radius);
            }
        }
        BlurMode::Pixelate => pixelate(data, width, r, effective(amount, DEFAULT_BLOCK_SIZE)),
    }
}

/// `region` clipped to the image and grown, around its centre, to at least
/// [`MIN_AMOUNT`] in each dimension (or the image size, if smaller). `None`
/// if it lies outside the image or is empty.
pub fn effective_region(region: Region, width: u32, height: u32) -> Option<Region> {
    let (x, w) = grow_axis(region.x, region.width, width)?;
    let (y, h) = grow_axis(region.y, region.height, height)?;
    Some(Region {
        x,
        y,
        width: w,
        height: h,
    })
}

fn grow_axis(start: u32, len: u32, size: u32) -> Option<(u32, u32)> {
    let end = start.saturating_add(len).min(size);
    if start >= end {
        return None;
    }
    let len = end - start;
    let need = MIN_AMOUNT.min(size);
    if len >= need {
        return Some((start, len));
    }
    let start = start.saturating_sub((need - len) / 2).min(size - need);
    Some((start, need))
}

fn effective(amount: Option<f64>, default: u32) -> u32 {
    match amount {
        Some(a) if a.is_finite() && a > 0.0 => (a.round() as u32).clamp(MIN_AMOUNT, 1024),
        _ => default,
    }
}

fn index(width: u32, x: u32, y: u32) -> usize {
    (y as usize * width as usize + x as usize) * 4
}

/// Fills `r` with the mean of its pixels.
fn mean_fill(data: &mut [u8], width: u32, r: Region) {
    fill_cell(data, width, r.x, r.y, r.width, r.height);
}

/// Separable box blur, repeated [`PASSES`] times, in place with one line
/// buffer. Edges clamp to the region, so only region pixels are sampled.
/// Rounding per pass to 8 bits matches storing every pass.
fn blur(data: &mut [u8], width: u32, r: Region, radius: u32) {
    let radius = radius as usize;
    let mut line: Vec<[u8; 4]> = Vec::with_capacity(r.width.max(r.height) as usize);
    let mut out: Vec<[u8; 4]> = Vec::with_capacity(line.capacity());
    for _ in 0..PASSES {
        for y in r.y..r.y + r.height {
            let start = index(width, r.x, y);
            let row = &mut data[start..start + r.width as usize * 4];
            line.clear();
            line.extend(row.chunks_exact(4).map(|px| [px[0], px[1], px[2], px[3]]));
            box_line(&line, radius, &mut out);
            for (px, v) in row.chunks_exact_mut(4).zip(&out) {
                px.copy_from_slice(v);
            }
        }
        for x in r.x..r.x + r.width {
            line.clear();
            line.extend((r.y..r.y + r.height).map(|y| {
                let i = index(width, x, y);
                [data[i], data[i + 1], data[i + 2], data[i + 3]]
            }));
            box_line(&line, radius, &mut out);
            for (y, v) in (r.y..r.y + r.height).zip(&out) {
                let i = index(width, x, y);
                data[i..i + 4].copy_from_slice(v);
            }
        }
    }
}

/// Mean over a `2 * radius + 1` window with clamped edges, rounded, into
/// `out`.
fn box_line(line: &[[u8; 4]], radius: usize, out: &mut Vec<[u8; 4]>) {
    let n = line.len();
    let window = (2 * radius + 1) as u32;
    let at = |i: isize| line[i.clamp(0, n as isize - 1) as usize];
    let mut sum = [0u32; 4];
    for i in -(radius as isize)..=radius as isize {
        let v = at(i);
        (0..4).for_each(|c| sum[c] += u32::from(v[c]));
    }
    out.clear();
    for x in 0..n {
        out.push(sum.map(|s| ((s + window / 2) / window) as u8));
        let (add, sub) = (
            at((x + radius + 1) as isize),
            at(x as isize - radius as isize),
        );
        (0..4).for_each(|c| sum[c] = sum[c] + u32::from(add[c]) - u32::from(sub[c]));
    }
}

/// Cells of `block` along an axis of `len`, with a remainder narrower than
/// [`MIN_AMOUNT`] folded into the previous cell.
fn cells(len: u32, block: u32) -> impl Iterator<Item = (u32, u32)> {
    let full = len / block;
    let rest = len % block;
    let fold = rest > 0 && rest < MIN_AMOUNT && full > 0;
    let count = full + u32::from(rest > 0 && !fold);
    (0..count).map(move |i| {
        let start = i * block;
        let size = if i + 1 == count { len - start } else { block };
        (start, size)
    })
}

/// Fills each `block`-sized cell (aligned to the region) with its mean.
fn pixelate(data: &mut [u8], width: u32, r: Region, block: u32) {
    for (by, bh) in cells(r.height, block) {
        for (bx, bw) in cells(r.width, block) {
            fill_cell(data, width, r.x + bx, r.y + by, bw, bh);
        }
    }
}

fn fill_cell(data: &mut [u8], width: u32, x0: u32, y0: u32, w: u32, h: u32) {
    let rows = || (y0..y0 + h).map(|y| index(width, x0, y)..index(width, x0 + w, y));
    let mut sum = [0u64; 4];
    for row in rows() {
        for px in data[row].chunks_exact(4) {
            (0..4).for_each(|c| sum[c] += u64::from(px[c]));
        }
    }
    let n = u64::from(w) * u64::from(h);
    let mean = sum.map(|s| ((s + n / 2) / n) as u8);
    for row in rows() {
        for px in data[row].chunks_exact_mut(4) {
            px.copy_from_slice(&mean);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(width: u32, height: u32) -> Vec<u8> {
        let mut state = 0x2545_f491_u32;
        (0..width * height * 4)
            .map(|i| {
                if i % 4 == 3 {
                    return 255;
                }
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as u8
            })
            .collect()
    }

    /// Redacts `region` of a noise image and checks that every pixel of the
    /// grown region changed and nothing outside it did.
    fn check(mode: BlurMode, amount: Option<f64>, region: Region, (w, h): (u32, u32)) {
        let source = noise(w, h);
        let mut out = source.clone();
        burn_in(&mut out, (w, h), region, mode, amount);
        let g = effective_region(region, w, h).unwrap();
        assert!(g.width >= MIN_AMOUNT.min(w) && g.height >= MIN_AMOUNT.min(h));
        for y in 0..h {
            for x in 0..w {
                let i = index(w, x, y);
                let inside =
                    (g.x..g.x + g.width).contains(&x) && (g.y..g.y + g.height).contains(&y);
                if inside {
                    assert_ne!(
                        out[i..i + 3],
                        source[i..i + 3],
                        "{mode:?} {region:?} kept ({x}, {y})"
                    );
                } else {
                    assert_eq!(
                        out[i..i + 4],
                        source[i..i + 4],
                        "{mode:?} {region:?} changed ({x}, {y})"
                    );
                }
            }
        }
    }

    const SIZES: [(u32, u32); 5] = [(1, 1), (3, 3), (21, 21), (11, 31), (31, 11)];

    #[test]
    fn no_original_pixel_survives_any_size() {
        for (rw, rh) in SIZES {
            for (x, y) in [(5, 4), (0, 0), (45 - rw, 40 - rh)] {
                let region = Region {
                    x,
                    y,
                    width: rw,
                    height: rh,
                };
                for amount in [None, Some(1.0), Some(4.0), Some(6.0), Some(10.0)] {
                    check(BlurMode::Blur, amount, region, (45, 40));
                    check(BlurMode::Pixelate, amount, region, (45, 40));
                }
            }
        }
    }

    #[test]
    fn regions_grow_to_the_minimum_within_the_image() {
        let r = |x, y, width, height| Region {
            x,
            y,
            width,
            height,
        };
        assert_eq!(effective_region(r(5, 5, 1, 1), 20, 20), Some(r(4, 4, 4, 4)));
        assert_eq!(
            effective_region(r(0, 19, 1, 1), 20, 20),
            Some(r(0, 16, 4, 4))
        );
        assert_eq!(
            effective_region(r(18, 0, 30, 2), 20, 20),
            Some(r(16, 0, 4, 4))
        );
        assert_eq!(
            effective_region(r(2, 3, 10, 6), 20, 20),
            Some(r(2, 3, 10, 6))
        );
        assert_eq!(effective_region(r(0, 0, 1, 1), 2, 3), Some(r(0, 0, 2, 3)));
        assert_eq!(effective_region(r(20, 0, 1, 1), 20, 20), None);
        assert_eq!(effective_region(r(3, 3, 0, 5), 20, 20), None);
    }

    #[test]
    fn single_pixel_images_become_grey() {
        let mut px = vec![1, 2, 3, 255];
        let all = Region {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        };
        burn_in(&mut px, (1, 1), all, BlurMode::Pixelate, None);
        assert_eq!(px, [128, 128, 128, 255]);
    }

    #[test]
    fn remainder_cells_fold_into_the_previous_cell() {
        let collect = |len, block| cells(len, block).collect::<Vec<_>>();
        assert_eq!(collect(21, 10), [(0, 10), (10, 11)]);
        assert_eq!(collect(23, 10), [(0, 10), (10, 13)]);
        assert_eq!(collect(24, 10), [(0, 10), (10, 10), (20, 4)]);
        assert_eq!(collect(20, 10), [(0, 10), (10, 10)]);
        assert_eq!(collect(4, 10), [(0, 4)]);
    }

    #[test]
    fn amounts_are_clamped() {
        assert_eq!(effective(Some(1.0), 9), MIN_AMOUNT);
        assert_eq!(effective(Some(f64::NAN), 9), 9);
        assert_eq!(effective(Some(-3.0), 9), 9);
        assert_eq!(effective(Some(7.6), 9), 8);
        assert_eq!(effective(None, 9), 9);
    }

    #[test]
    fn pixelate_fills_blocks_with_their_mean() {
        let mut data = vec![0u8; 4 * 4 * 4];
        for (i, px) in data.chunks_exact_mut(4).enumerate() {
            px.copy_from_slice(&[(i * 10) as u8, 0, 0, 255]);
        }
        let all = Region {
            x: 0,
            y: 0,
            width: 4,
            height: 4,
        };
        pixelate(&mut data, 4, all, 4);
        assert!(data.chunks_exact(4).all(|px| px == [75, 0, 0, 255]));
    }
}
