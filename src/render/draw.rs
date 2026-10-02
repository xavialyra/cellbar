use std::{ops::Range, sync::LazyLock};

use crate::config::Rgba;

static GAMMA_TABLES: LazyLock<GammaTables> = LazyLock::new(GammaTables::init);

struct GammaTables {
    to_linear: [u16; 256],
    to_srgb: [u8; 4096],
}

impl GammaTables {
    fn init() -> Self {
        let mut to_linear = [0u16; 256];
        for (i, item) in to_linear.iter_mut().enumerate() {
            let v = (i as f32) / 255.0;
            *item = (v.powf(2.2) * 4095.0 + 0.5) as u16;
        }
        let mut to_srgb = [0u8; 4096];
        for (j, item) in to_srgb.iter_mut().enumerate() {
            let v = (j as f32) / 4095.0;
            *item = (v.powf(1.0 / 2.2) * 255.0 + 0.5).min(255.0) as u8;
        }
        Self { to_linear, to_srgb }
    }
}

#[inline(always)]
pub fn blend_channel_gamma(src: u8, dst: u8, alpha: u8) -> u8 {
    if alpha == 0 {
        return dst;
    }
    if alpha == 255 {
        return src;
    }
    let tables = &*GAMMA_TABLES;
    let src_lin = tables.to_linear[src as usize] as u32;
    let dst_lin = tables.to_linear[dst as usize] as u32;
    let a = alpha as u32;
    let inv = 255 - a;
    let mixed_lin = (src_lin * a + dst_lin * inv + 127) / 255;
    tables.to_srgb[mixed_lin.min(4095) as usize]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color(pub u8, pub u8, pub u8, pub u8);

impl Color {
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Color(r, g, b, a)
    }

    #[allow(dead_code)]
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Color(r, g, b, 255)
    }

    pub fn as_rgba_tuple(&self) -> (u8, u8, u8, u8) {
        (self.0, self.1, self.2, self.3)
    }
}

#[derive(Clone, Copy)]
pub struct Placement {
    pub clip_start: u32,
    pub clip_end: u32,
}

#[allow(clippy::too_many_arguments)]
pub fn draw_glyph_rect(
    canvas: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
    placement: Placement,
    radius: u32,
    glyph_x: i32,
    glyph_y: i32,
    glyph_width: u32,
    glyph_height: u32,
    clip_top: i64,
    clip_bottom: i64,
    color: Color,
) {
    let origin_x = i64::from(glyph_x);
    let start_x = origin_x.max(i64::from(placement.clip_start)).max(0) as u32;
    let end_x = (origin_x + i64::from(glyph_width))
        .min(i64::from(placement.clip_end))
        .min(i64::from(canvas_width))
        .max(0) as u32;
    let start_y = i64::from(glyph_y).max(clip_top).max(0) as u32;
    let end_y = (i64::from(glyph_y) + i64::from(glyph_height))
        .min(clip_bottom)
        .min(i64::from(canvas_height))
        .max(0) as u32;

    if start_x >= end_x || start_y >= end_y {
        return;
    }

    for y in start_y..end_y {
        for x in start_x..end_x {
            if !rounded_rect_contains(x, y, canvas_width, canvas_height, radius) {
                continue;
            }
            let Some(index) = pixel_index(canvas_width, x, y) else {
                continue;
            };
            let Some(pixel) = canvas.get_mut(index..index + 4) else {
                continue;
            };
            blend_pixel(pixel, color);
        }
    }
}

pub fn fill(canvas: &mut [u8], color: Rgba) {
    let pixel = premultiplied_argb(color).to_ne_bytes();
    for chunk in canvas.as_chunks_mut::<4>().0 {
        *chunk = pixel;
    }
}

pub fn fill_rounded(canvas: &mut [u8], width: u32, height: u32, color: Rgba, radius: u32) {
    if radius == 0 {
        fill(canvas, color);
        return;
    }

    let inside = premultiplied_argb(color).to_ne_bytes();
    let outside = [0; 4];
    for y in 0..height {
        for x in 0..width {
            let Some(index) = pixel_index(width, x, y) else {
                continue;
            };
            let Some(pixel) = canvas.get_mut(index..index + 4) else {
                continue;
            };
            if rounded_rect_contains(x, y, width, height, radius) {
                pixel.copy_from_slice(&inside);
            } else {
                pixel.copy_from_slice(&outside);
            }
        }
    }
}

// Compute coverage (0..=255) for smooth anti-aliased rounded rectangle edges.
pub fn rounded_rect_coverage(x: u32, y: u32, width: u32, height: u32, radius: u32) -> u8 {
    if radius == 0 || x >= width || y >= height {
        return 255;
    }
    let radius = radius.min(width.min(height) / 2);
    if radius == 0 {
        return 255;
    }

    let corner = if x < radius && y < radius {
        Some((radius, radius))
    } else if x >= width.saturating_sub(radius) && y < radius {
        Some((width.saturating_sub(radius), radius))
    } else if x < radius && y >= height.saturating_sub(radius) {
        Some((radius, height.saturating_sub(radius)))
    } else if x >= width.saturating_sub(radius) && y >= height.saturating_sub(radius) {
        Some((width.saturating_sub(radius), height.saturating_sub(radius)))
    } else {
        None
    };
    let Some((corner_x, corner_y)) = corner else {
        return 255;
    };

    let px = x as f32 + 0.5;
    let py = y as f32 + 0.5;
    let cx = corner_x as f32;
    let cy = corner_y as f32;
    let dx = px - cx;
    let dy = py - cy;
    let dist_sq = dx * dx + dy * dy;
    let r = radius as f32;

    let r_inner = (r - std::f32::consts::FRAC_1_SQRT_2).max(0.0);
    if dist_sq <= r_inner * r_inner {
        return 255;
    }
    let r_outer = r + std::f32::consts::FRAC_1_SQRT_2;
    if dist_sq >= r_outer * r_outer {
        return 0;
    }

    // 4x4 subpixel grid (16 samples) for smooth anti-aliased edge
    let mut covered = 0u32;
    let r_sq = r * r;
    for sy in 0..4 {
        for sx in 0..4 {
            let spx = x as f32 + (sx as f32 + 0.5) / 4.0;
            let spy = y as f32 + (sy as f32 + 0.5) / 4.0;
            let sdx = spx - cx;
            let sdy = spy - cy;
            if sdx * sdx + sdy * sdy <= r_sq {
                covered += 1;
            }
        }
    }
    ((covered * 255 + 8) / 16) as u8
}

// Test pixel centers so the transparent corners follow the configured radius.
pub fn rounded_rect_contains(x: u32, y: u32, width: u32, height: u32, radius: u32) -> bool {
    if radius == 0 || x >= width || y >= height {
        return true;
    }
    let radius = radius.min(width.min(height) / 2);
    if radius == 0 {
        return true;
    }

    let corner = if x < radius && y < radius {
        Some((radius, radius))
    } else if x >= width.saturating_sub(radius) && y < radius {
        Some((width.saturating_sub(radius), radius))
    } else if x < radius && y >= height.saturating_sub(radius) {
        Some((radius, height.saturating_sub(radius)))
    } else if x >= width.saturating_sub(radius) && y >= height.saturating_sub(radius) {
        Some((width.saturating_sub(radius), height.saturating_sub(radius)))
    } else {
        None
    };
    let Some((corner_x, corner_y)) = corner else {
        return true;
    };

    let pixel_x = u128::from(x) * 2 + 1;
    let pixel_y = u128::from(y) * 2 + 1;
    let center_x = u128::from(corner_x) * 2;
    let center_y = u128::from(corner_y) * 2;
    let dx = pixel_x.abs_diff(center_x);
    let dy = pixel_y.abs_diff(center_y);
    dx * dx + dy * dy <= u128::from(radius) * 2 * u128::from(radius) * 2
}

#[allow(clippy::too_many_arguments)]
pub fn fill_rect(
    canvas: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
    placement: Placement,
    x_range: Range<u32>,
    y_range: Range<u32>,
    radius: u32,
    color: Rgba,
) {
    fill_rounded_rect(
        canvas,
        canvas_width,
        canvas_height,
        placement,
        x_range,
        y_range,
        radius,
        0,
        color,
    );
}

#[allow(clippy::too_many_arguments)]
pub fn fill_rounded_rect(
    canvas: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
    placement: Placement,
    x_range: Range<u32>,
    y_range: Range<u32>,
    bar_radius: u32,
    capsule_radius: u32,
    color: Rgba,
) {
    let start_x = x_range.start.max(placement.clip_start).min(canvas_width);
    let end_x = x_range.end.min(placement.clip_end).min(canvas_width);
    if start_x >= end_x {
        return;
    }
    let start_y = y_range.start.min(canvas_height);
    let end_y = y_range.end.min(canvas_height);
    if start_y >= end_y {
        return;
    }
    let rect_w = x_range.end.saturating_sub(x_range.start);
    let rect_h = y_range.end.saturating_sub(y_range.start);
    if rect_w == 0 || rect_h == 0 {
        return;
    }
    let source = Color::rgba(color.red, color.green, color.blue, color.alpha);
    for y in start_y..end_y {
        let local_y = y.saturating_sub(y_range.start);
        for x in start_x..end_x {
            let mut cov = 255u32;
            if bar_radius > 0 {
                let bar_cov = u32::from(rounded_rect_coverage(
                    x,
                    y,
                    canvas_width,
                    canvas_height,
                    bar_radius,
                ));
                if bar_cov == 0 {
                    continue;
                }
                cov = (cov * bar_cov + 127) / 255;
            }
            if capsule_radius > 0 {
                let local_x = x.saturating_sub(x_range.start);
                let cap_cov = u32::from(rounded_rect_coverage(
                    local_x,
                    local_y,
                    rect_w,
                    rect_h,
                    capsule_radius,
                ));
                if cap_cov == 0 {
                    continue;
                }
                cov = (cov * cap_cov + 127) / 255;
            }
            if cov == 0 {
                continue;
            }
            let Some(index) = pixel_index(canvas_width, x, y) else {
                continue;
            };
            let Some(pixel) = canvas.get_mut(index..index + 4) else {
                continue;
            };
            if cov == 255 {
                blend_pixel(pixel, source);
            } else {
                let effective_alpha = ((u32::from(color.alpha) * cov + 127) / 255) as u8;
                if effective_alpha > 0 {
                    blend_pixel(
                        pixel,
                        Color::rgba(color.red, color.green, color.blue, effective_alpha),
                    );
                }
            }
        }
    }
}

pub fn pixel_index(width: u32, x: u32, y: u32) -> Option<usize> {
    y.checked_mul(width)?
        .checked_add(x)?
        .checked_mul(4)?
        .try_into()
        .ok()
}

pub fn blend_pixel(pixel: &mut [u8], source: Color) {
    let destination = u32::from_ne_bytes([pixel[0], pixel[1], pixel[2], pixel[3]]);
    let destination_alpha = (destination >> 24) & 0xff;
    let destination_red = (destination >> 16) & 0xff;
    let destination_green = (destination >> 8) & 0xff;
    let destination_blue = destination & 0xff;
    let (red, green, blue, alpha) = source.as_rgba_tuple();
    let alpha = u32::from(alpha);
    let inverse = 255 - alpha;

    let output_alpha = alpha + (destination_alpha * inverse + 127) / 255;
    let blend = |channel: u8, destination_channel: u32| {
        (u32::from(channel) * alpha + destination_channel * inverse + 127) / 255
    };
    let output = (output_alpha << 24)
        | (blend(red, destination_red) << 16)
        | (blend(green, destination_green) << 8)
        | blend(blue, destination_blue);
    pixel.copy_from_slice(&output.to_ne_bytes());
}

pub fn blend_pixel_subpixel_gamma(pixel: &mut [u8], fg: Color, cov_r: u8, cov_g: u8, cov_b: u8) {
    let (fg_r, fg_g, fg_b, fg_a) = fg.as_rgba_tuple();
    if fg_a == 0 || (cov_r == 0 && cov_g == 0 && cov_b == 0) {
        return;
    }
    // FreeType-style 3-tap FIR balance across subpixels to eliminate color fringing while preserving 3x resolution
    let avg = ((u32::from(cov_r) + u32::from(cov_g) + u32::from(cov_b) + 1) / 3) as u8;
    let cov_r = ((u16::from(cov_r) * 2 + u16::from(avg) + 1) / 3) as u8;
    let cov_g = ((u16::from(cov_g) * 2 + u16::from(avg) + 1) / 3) as u8;
    let cov_b = ((u16::from(cov_b) * 2 + u16::from(avg) + 1) / 3) as u8;

    let alpha_r = ((u16::from(cov_r) * u16::from(fg_a) + 127) / 255) as u8;
    let alpha_g = ((u16::from(cov_g) * u16::from(fg_a) + 127) / 255) as u8;
    let alpha_b = ((u16::from(cov_b) * u16::from(fg_a) + 127) / 255) as u8;

    let destination = u32::from_ne_bytes([pixel[0], pixel[1], pixel[2], pixel[3]]);
    let dst_a = ((destination >> 24) & 0xff) as u8;
    let dst_r = ((destination >> 16) & 0xff) as u8;
    let dst_g = ((destination >> 8) & 0xff) as u8;
    let dst_b = (destination & 0xff) as u8;

    if dst_a == 255 {
        let out_r = blend_channel_gamma(fg_r, dst_r, alpha_r);
        let out_g = blend_channel_gamma(fg_g, dst_g, alpha_g);
        let out_b = blend_channel_gamma(fg_b, dst_b, alpha_b);
        let output =
            (255u32 << 24) | (u32::from(out_r) << 16) | (u32::from(out_g) << 8) | u32::from(out_b);
        pixel.copy_from_slice(&output.to_ne_bytes());
        return;
    }

    let avg_alpha = ((u32::from(alpha_r) + u32::from(alpha_g) + u32::from(alpha_b) + 1) / 3) as u8;
    let out_a = avg_alpha + ((u32::from(dst_a) * (255 - u32::from(avg_alpha)) + 127) / 255) as u8;
    if out_a == 0 {
        return;
    }

    let blend_one = |fg_c: u8, dst_pm: u8, a: u8| -> u8 {
        if dst_a == 0 {
            return ((u32::from(fg_c) * u32::from(a) + 127) / 255) as u8;
        }
        let unpm_dst =
            ((u32::from(dst_pm) * 255 + u32::from(dst_a) / 2) / u32::from(dst_a)).min(255) as u8;
        let weight =
            ((u32::from(a) * 255 + u32::from(out_a) / 2) / u32::from(out_a)).min(255) as u8;
        let unpm_out = blend_channel_gamma(fg_c, unpm_dst, weight);
        ((u32::from(unpm_out) * u32::from(out_a) + 127) / 255) as u8
    };

    let out_r = blend_one(fg_r, dst_r, alpha_r);
    let out_g = blend_one(fg_g, dst_g, alpha_g);
    let out_b = blend_one(fg_b, dst_b, alpha_b);
    let output = (u32::from(out_a) << 24)
        | (u32::from(out_r) << 16)
        | (u32::from(out_g) << 8)
        | u32::from(out_b);
    pixel.copy_from_slice(&output.to_ne_bytes());
}

pub fn blend_pixel_grayscale_gamma(pixel: &mut [u8], fg: Color, coverage: u8) {
    blend_pixel_subpixel_gamma(pixel, fg, coverage, coverage, coverage);
}

#[allow(clippy::too_many_arguments)]
pub fn draw_cached_glyph(
    canvas: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
    placement: Placement,
    radius: u32,
    physical_x: i32,
    physical_y: i32,
    clip_top: i64,
    clip_bottom: i64,
    color: Color,
    cached: &super::font::CachedGlyph,
) {
    let origin_x = i64::from(physical_x) + i64::from(cached.left);
    let origin_y = i64::from(physical_y) - i64::from(cached.top);
    let glyph_width = cached.width;
    let glyph_height = cached.height;

    let start_x = origin_x.max(i64::from(placement.clip_start)).max(0) as u32;
    let end_x = (origin_x + i64::from(glyph_width))
        .min(i64::from(placement.clip_end))
        .min(i64::from(canvas_width))
        .max(0) as u32;
    let start_y = origin_y.max(clip_top).max(0) as u32;
    let end_y = (origin_y + i64::from(glyph_height))
        .min(clip_bottom)
        .min(i64::from(canvas_height))
        .max(0) as u32;

    if start_x >= end_x || start_y >= end_y {
        return;
    }

    for y in start_y..end_y {
        let gy = (i64::from(y) - origin_y) as u32;
        for x in start_x..end_x {
            if !rounded_rect_contains(x, y, canvas_width, canvas_height, radius) {
                continue;
            }
            let gx = (i64::from(x) - origin_x) as u32;
            let Some(index) = pixel_index(canvas_width, x, y) else {
                continue;
            };
            let Some(pixel) = canvas.get_mut(index..index + 4) else {
                continue;
            };
            match cached.content {
                swash::scale::image::Content::Mask => {
                    let data_idx = (gy * glyph_width + gx) as usize;
                    if let Some(&alpha) = cached.data.get(data_idx)
                        && alpha > 0
                    {
                        blend_pixel_grayscale_gamma(pixel, color, alpha);
                    }
                }
                swash::scale::image::Content::SubpixelMask => {
                    let data_idx = ((gy * glyph_width + gx) * 4) as usize;
                    if let Some(sub) = cached.data.get(data_idx..data_idx + 3) {
                        blend_pixel_subpixel_gamma(pixel, color, sub[0], sub[1], sub[2]);
                    }
                }
                swash::scale::image::Content::Color => {
                    let data_idx = ((gy * glyph_width + gx) * 4) as usize;
                    if let Some(rgba) = cached.data.get(data_idx..data_idx + 4)
                        && rgba[3] > 0
                    {
                        blend_pixel(pixel, Color::rgba(rgba[0], rgba[1], rgba[2], rgba[3]));
                    }
                }
            }
        }
    }
}

pub fn premultiplied_argb(color: Rgba) -> u32 {
    let alpha = u32::from(color.alpha);
    let premultiply = |channel: u8| (u32::from(channel) * alpha + 127) / 255;

    (alpha << 24)
        | (premultiply(color.red) << 16)
        | (premultiply(color.green) << 8)
        | premultiply(color.blue)
}
