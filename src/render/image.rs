use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;

use crate::{
    cell_frame::{CellFrame, ImageSlice, grapheme_cell_width},
    config::DisplayStyle,
    images::{ImageAlign, ImageFit, ImageShape},
};

use super::draw::{pixel_index, rounded_rect_contains};

pub struct ImageRun {
    pub image: ImageSlice,
    pub style: DisplayStyle,
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct CircleMask {
    pub center_x: f32,
    pub center_y: f32,
    pub radius: f32,
}

#[derive(Clone, Copy)]
pub struct ImageLayout {
    pub cell_width: u32,
    pub origin: u32,
    pub offset_y: i64,
    pub line_height: f32,
    pub cap_height: f32,
    pub baseline: f32,
}

pub struct ImageGeometry {
    pub x: i64,
    pub y: i64,
    pub width: u32,
    pub height: u32,
    pub clip_top: i64,
    pub clip_bottom: i64,
    pub circle: Option<CircleMask>,
}

pub struct ImagePaint {
    pub visible: Range<u32>,
    pub x: i64,
    pub y: i64,
    pub clip_top: i64,
    pub clip_bottom: i64,
    pub circle: Option<CircleMask>,
}

pub struct ReadyImage {
    pub pixels: std::sync::Arc<image::RgbaImage>,
    pub paint: ImagePaint,
}

pub fn collect_images(frame: &CellFrame) -> Vec<ImageRun> {
    let mut runs: Vec<ImageRun> = Vec::new();
    for (column, cell) in frame.cells.iter().enumerate() {
        let Some(slice) = &cell.image else { continue };
        if let Some(last) = runs.last_mut()
            && last.end == column
            && last.image.placement == slice.placement
            && std::sync::Arc::ptr_eq(&last.image.image, &slice.image)
            && last.image.index + (column - last.start) == slice.index
            && last.style == cell.style
        {
            last.end += 1;
            continue;
        }
        runs.push(ImageRun {
            image: slice.clone(),
            style: cell.style,
            start: column,
            end: column + 1,
        });
    }
    runs
}

pub fn place_fallback(frame: &mut CellFrame, run: &ImageRun) {
    for cell in &mut frame.cells[run.start..run.end] {
        cell.image = None;
        cell.continuation = false;
        cell.symbol.clear();
    }
    let mut column = run.start;
    for grapheme in run.image.image.fallback.graphemes(true) {
        let width = grapheme_cell_width(grapheme);
        if width == 0 || column + width > run.end {
            break;
        }
        frame.cells[column].symbol = grapheme.to_owned();
        for cell in &mut frame.cells[column + 1..column + width] {
            cell.continuation = true;
        }
        column += width;
    }
}

pub fn image_geometry(
    run: &ImageRun,
    source: (u32, u32),
    layout: ImageLayout,
) -> Option<ImageGeometry> {
    let (src_w, src_h) = source;
    if src_w == 0 || src_h == 0 || layout.cell_width == 0 {
        return None;
    }
    let spec = &run.image.image;
    let start = i64::try_from(run.start).ok()? - i64::try_from(run.image.index).ok()?;
    let box_x = i64::from(layout.origin) + start * i64::from(layout.cell_width);
    let box_w = (spec.width as f32) * layout.cell_width as f32;
    let (box_y, box_h) = if spec.fit == ImageFit::TextMatch {
        (
            layout.offset_y + (layout.baseline - layout.cap_height).round() as i64,
            layout.cap_height,
        )
    } else {
        (layout.offset_y, layout.line_height)
    };
    if box_h <= 0.0 {
        return None;
    }
    let (fit_x, fit_y, fit_w, fit_h, circle) = if spec.shape == ImageShape::Circle {
        let diameter = box_w.min(box_h);
        let left = match spec.align {
            ImageAlign::Left => box_x as f32,
            ImageAlign::Center => box_x as f32 + (box_w - diameter) / 2.0,
            ImageAlign::Right => box_x as f32 + box_w - diameter,
        };
        let top = box_y as f32 + (box_h - diameter) / 2.0;
        (
            left,
            top,
            diameter,
            diameter,
            Some(CircleMask {
                center_x: left + diameter / 2.0,
                center_y: top + diameter / 2.0,
                radius: diameter / 2.0,
            }),
        )
    } else {
        (box_x as f32, box_y as f32, box_w, box_h, None)
    };
    let sx = fit_w / src_w as f32;
    let sy = fit_h / src_h as f32;
    let (w, h) = match spec.fit {
        ImageFit::Stretch => (fit_w, fit_h),
        ImageFit::Contain | ImageFit::TextMatch => {
            (src_w as f32 * sx.min(sy), src_h as f32 * sx.min(sy))
        }
        ImageFit::ScaleDown => (
            src_w as f32 * sx.min(sy).min(1.0),
            src_h as f32 * sx.min(sy).min(1.0),
        ),
        ImageFit::Cover => (src_w as f32 * sx.max(sy), src_h as f32 * sx.max(sy)),
    };
    if !w.is_finite() || !h.is_finite() || w > 4096.0 || h > 4096.0 || w * h > 4_000_000.0 {
        return None;
    }
    let x = match spec.align {
        ImageAlign::Left => fit_x.round() as i64,
        ImageAlign::Center => (fit_x + (fit_w - w) / 2.0).round() as i64,
        ImageAlign::Right => (fit_x + fit_w - w).round() as i64,
    };
    let y = (fit_y + (fit_h - h) / 2.0).round() as i64;
    Some(ImageGeometry {
        x,
        y,
        width: w.round().max(1.0) as u32,
        height: h.round().max(1.0) as u32,
        clip_top: box_y,
        clip_bottom: box_y + box_h.round() as i64,
        circle,
    })
}

pub fn blit_image(
    canvas: &mut [u8],
    canvas_size: (u32, u32),
    radius: u32,
    paint: ImagePaint,
    pixels: &image::RgbaImage,
) {
    let (canvas_w, canvas_h) = canvas_size;
    let left = paint.x.max(i64::from(paint.visible.start)).max(0);
    let right = (paint.x + i64::from(pixels.width()))
        .min(i64::from(paint.visible.end))
        .min(i64::from(canvas_w));
    let top = paint.y.max(paint.clip_top).max(0);
    let bottom = (paint.y + i64::from(pixels.height()))
        .min(paint.clip_bottom)
        .min(i64::from(canvas_h));
    if left >= right || top >= bottom {
        return;
    }
    for row in top..bottom {
        for col in left..right {
            let px = col as u32;
            let py = row as u32;
            if !rounded_rect_contains(px, py, canvas_w, canvas_h, radius) {
                continue;
            }
            let source = pixels
                .get_pixel((col - paint.x) as u32, (row - paint.y) as u32)
                .0;
            let coverage = paint
                .circle
                .map_or(255, |mask| circle_coverage(mask, px, py));
            let alpha = (u32::from(source[3]) * coverage + 127) / 255;
            if alpha == 0 {
                continue;
            }
            let Some(index) = pixel_index(canvas_w, px, py) else {
                continue;
            };
            let Some(dest) = canvas.get_mut(index..index + 4) else {
                continue;
            };
            let old = u32::from_ne_bytes(dest.try_into().expect("four pixel bytes"));
            let inverse = 255 - alpha;
            let component =
                |value: u8, old: u32| (u32::from(value) * alpha + old * inverse + 127) / 255;
            let result = ((alpha + ((old >> 24) & 255) * inverse / 255) << 24)
                | (component(source[0], (old >> 16) & 255) << 16)
                | (component(source[1], (old >> 8) & 255) << 8)
                | component(source[2], old & 255);
            dest.copy_from_slice(&result.to_ne_bytes());
        }
    }
}

pub fn circle_coverage(circle: CircleMask, x: u32, y: u32) -> u32 {
    let dx = x as f32 + 0.5 - circle.center_x;
    let dy = y as f32 + 0.5 - circle.center_y;
    let distance_squared = dx * dx + dy * dy;
    let inner = (circle.radius - std::f32::consts::FRAC_1_SQRT_2).max(0.0);
    if circle.radius >= std::f32::consts::FRAC_1_SQRT_2 && distance_squared <= inner * inner {
        return 255;
    }
    let outer = circle.radius + std::f32::consts::FRAC_1_SQRT_2;
    if distance_squared >= outer * outer {
        return 0;
    }
    let mut covered = 0;
    for sy in 0..4 {
        for sx in 0..4 {
            let dx = x as f32 + (sx as f32 + 0.5) / 4.0 - circle.center_x;
            let dy = y as f32 + (sy as f32 + 0.5) / 4.0 - circle.center_y;
            if dx * dx + dy * dy <= circle.radius * circle.radius {
                covered += 1;
            }
        }
    }
    (covered * 255 + 8) / 16
}
