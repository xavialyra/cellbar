pub mod draw;
pub mod font;
pub mod image;

#[cfg(test)]
mod tests;

pub use draw::{
    Color, Placement, draw_cached_glyph, draw_glyph_rect, fill_rect, fill_rounded,
    fill_rounded_rect,
};
pub use font::{
    FontCandidate, FontStyleRequirements, GlyphKey, LazyFontStyles, ResolvedFontConfig,
    ShapedGlyph, SwashCache, font_cache_id, load_fonts,
};
pub use image::{
    ImageLayout, ImagePaint, ReadyImage, blit_image, collect_images, image_geometry,
    place_fallback,
};

use unicode_segmentation::UnicodeSegmentation;

use crate::{
    cell_frame::{CapsuleShell, CellFrame, StyledText},
    config::{DisplayStyle, FontFamilyEntry, Rgba},
    images::ImageStore,
};

pub struct TextRenderer {
    font_db: fontdb::Database,
    lazy_fonts: LazyFontStyles,
    primary_face: fontdb::ID,
    shape_context: swash::shape::ShapeContext,
    scale_context: swash::scale::ScaleContext,
    cache: SwashCache,
    family: String,
    candidates: Vec<FontCandidate>,
    fallback_indices: Vec<usize>,
    has_mappings: bool,
    font_size: f32,
    cell_width: u32,
    cell_height: u32,
    // Logical row metrics measured once from the primary face; scaled at paint
    // time. `baseline` is the y offset from the top of the cell and is shared
    // by every glyph and image on the row.
    cap_height: f32,
    baseline: f32,
    prepared: Option<PreparedRegion>,
    pub(crate) images: Option<ImageStore>,
}

pub struct PaintTarget<'a> {
    pub canvas: &'a mut [u8],
    pub width: u32,
    pub height: u32,
    pub scale: u32,
    pub background: Rgba,
    pub radius: u32,
}

pub(crate) struct PreparedRegion {
    pub height: u32,
    pub scale: u32,
    pub segments: Vec<PreparedSegment>,
}

/// Vertical metrics of one text row, in logical pixels.
#[derive(Clone, Copy)]
pub(crate) struct RowMetrics {
    pub cap_height: f32,
    pub baseline: f32,
}

/// Measures the row metrics from the primary face once, so every glyph and
/// image on a row shares the same baseline regardless of which fallback font
/// shaped it.
pub(crate) fn measure_row_metrics(
    db: &fontdb::Database,
    family: &str,
    font_size: f32,
    cell_height: u32,
) -> RowMetrics {
    let line_height = cell_height as f32;
    let mut metrics = RowMetrics {
        cap_height: font_size * 0.7,
        baseline: line_height * 0.8,
    };
    let family = fontdb::Family::Name(family);
    let query = fontdb::Query {
        families: &[family],
        ..Default::default()
    };
    let Some(id) = db.query(&query) else {
        return metrics;
    };
    db.with_face_data(id, |data, index| {
        let Ok(face) = ttf_parser::Face::parse(data, index) else {
            return metrics;
        };
        let units = f32::from(face.units_per_em());
        if units <= 0.0 {
            return metrics;
        }
        let ascent = f32::from(face.ascender()) * font_size / units;
        let descent = -f32::from(face.descender()) * font_size / units;
        metrics.baseline =
            ((line_height - (ascent + descent)) / 2.0 + ascent).clamp(0.0, line_height);
        if let Some(cap) = face.capital_height() {
            metrics.cap_height = (f32::from(cap) * font_size / units)
                .max(1.0)
                .min(line_height);
        }
        metrics
    })
    .unwrap_or(metrics)
}

pub(crate) struct PreparedSegment {
    pub text: String,
    pub style: DisplayStyle,
    pub glyphs: Vec<ShapedGlyph>,
}

pub(crate) struct FrameRun {
    pub text: StyledText,
    pub start: usize,
    pub end: usize,
    pub graphemes: Vec<(usize, usize)>,
}

pub(crate) fn glyph_columns(run: &FrameRun, glyph: &ShapedGlyph) -> Option<(usize, usize)> {
    let index = run
        .graphemes
        .partition_point(|(byte, _)| *byte <= glyph.start);
    let &(_, start) = run.graphemes.get(index.checked_sub(1)?)?;
    let next = run.graphemes.partition_point(|(byte, _)| *byte < glyph.end);
    let end = run
        .graphemes
        .get(next)
        .map_or(run.end, |&(_, column)| column);
    (end > start).then_some((start, end))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CapsuleRun {
    pub start: usize,
    pub end: usize,
    pub shell: CapsuleShell,
    pub owner: Option<usize>,
}

pub(crate) fn capsule_runs(frame: &CellFrame) -> Vec<CapsuleRun> {
    let mut runs: Vec<CapsuleRun> = Vec::new();
    for (column, cell) in frame.cells.iter().enumerate() {
        let Some(shell) = cell.capsule else {
            continue;
        };
        if runs.last().is_none_or(|run| {
            run.end != column || run.shell != shell || run.owner != cell.owner
        }) {
            runs.push(CapsuleRun {
                start: column,
                end: column,
                shell,
                owner: cell.owner,
            });
        }
        let run = runs.last_mut().expect("just appended a capsule run");
        run.end = column + 1;
    }
    runs
}

pub(crate) fn frame_runs(frame: &CellFrame) -> Vec<FrameRun> {
    let mut runs: Vec<FrameRun> = Vec::new();
    for (column, cell) in frame.cells.iter().enumerate() {
        if cell.image.is_some() {
            continue;
        }
        if cell.symbol.is_empty() && !cell.continuation {
            continue;
        }
        if runs.last().is_none_or(|run| {
            run.end != column || run.text.style != cell.style || run.text.owner != cell.owner
        }) {
            runs.push(FrameRun {
                text: StyledText {
                    text: String::new(),
                    style: cell.style,
                    owner: cell.owner,
                },
                start: column,
                end: column,
                graphemes: Vec::new(),
            });
        }
        let run = runs.last_mut().expect("just appended a run");
        run.end = column + 1;
        if !cell.continuation {
            run.graphemes.push((run.text.text.len(), column));
            run.text.text.push_str(&cell.symbol);
        }
    }
    runs
}

impl TextRenderer {
    pub fn new(
        families: &[FontFamilyEntry],
        font_size: f32,
        cell_width_adjust: i32,
        line_height: f32,
        requirements: FontStyleRequirements,
    ) -> Self {
        let resolved = ResolvedFontConfig::from_entries(families);
        let family = resolved.primary_family.clone();
        let has_mappings = resolved
            .candidates
            .iter()
            .any(|candidate| matches!(candidate, FontCandidate::Mapping { .. }));
        let candidates = resolved.candidates.clone();
        let fallback_indices = resolved.regular_families.iter().skip(1).copied().collect();
        let (font_db, lazy_fonts) = load_fonts(&resolved, requirements);
        let mut shape_context = swash::shape::ShapeContext::with_max_entries(4);
        let scale_context = swash::scale::ScaleContext::with_max_entries(4);

        let mut measured_width = None;
        let query = fontdb::Query {
            families: &[fontdb::Family::Name(&family)],
            ..Default::default()
        };
        if let Some(id) = font_db.query(&query) {
            font_db.with_face_data(id, |data, index| {
                if let Some(font_ref) = swash::FontRef::from_index(data, index as usize) {
                    let mut shaper = shape_context
                        .builder_with_id(font_ref, font_cache_id(id, index))
                        .size(font_size)
                        .build();
                    shaper.add_str("0");
                    shaper.shape_with(|c| {
                        for g in c.glyphs {
                            measured_width = Some(g.advance);
                        }
                    });
                }
            });
        }
        let cell_width = measured_width
            .map(|w| (w + cell_width_adjust as f32).ceil().max(1.0) as u32)
            .unwrap_or_else(|| (font_size * 0.6 + cell_width_adjust as f32).ceil().max(1.0) as u32);
        let cell_height = crate::config::cell_height(font_size, line_height);
        let metrics = measure_row_metrics(&font_db, &family, font_size, cell_height);
        let primary_face = font_db.faces().next().map(|f| f.id).expect("at least one font loaded");
        Self {
            font_db,
            lazy_fonts,
            primary_face,
            shape_context,
            scale_context,
            cache: SwashCache::new(),
            family,
            candidates,
            fallback_indices,
            has_mappings,
            font_size,
            cell_width,
            cell_height,
            cap_height: metrics.cap_height,
            baseline: metrics.baseline,
            prepared: None,
            images: None,
        }
    }

    #[cfg(test)]
    pub fn from_family_names(
        names: &[&str],
        font_size: f32,
        requirements: FontStyleRequirements,
    ) -> Self {
        let entries: Vec<FontFamilyEntry> = names
            .iter()
            .map(|&name| FontFamilyEntry::Family(name.to_owned()))
            .collect();
        Self::new(
            &entries,
            font_size,
            0,
            crate::config::DEFAULT_LINE_HEIGHT,
            requirements,
        )
    }

    pub fn cell_width(&self) -> u32 {
        self.cell_width
    }

    pub fn cell_geometry(&self, logical_width: u32, padding: u32) -> (usize, u32) {
        let available = logical_width.saturating_sub(padding.saturating_mul(2));
        let columns = available / self.cell_width;
        let origin = padding + (available - columns * self.cell_width) / 2;
        (columns as usize, origin)
    }

    pub fn paint_frame(&mut self, target: PaintTarget<'_>, frame: &CellFrame, origin: u32) {
        let radius = target.radius.saturating_mul(target.scale);
        fill_rounded(
            target.canvas,
            target.width,
            target.height,
            target.background,
            radius,
        );
        let mut text_frame = frame.clone();
        let image_runs = collect_images(frame);
        let mut ready = Vec::new();
        let cell_width = self.cell_width.saturating_mul(target.scale);
        let x_origin = origin.saturating_mul(target.scale);
        let cell_height = self
            .cell_height
            .saturating_mul(target.scale)
            .min(target.height);
        let line_height = cell_height as f32;
        let y_offset = target.height.saturating_sub(cell_height) / 2;
        let y_start = y_offset.min(target.height);
        let y_end = (y_offset + cell_height).min(target.height);
        let y_range = y_start..y_end;
        let cap_runs = capsule_runs(frame);
        for cap in &cap_runs {
            let start = x_origin.saturating_add((cap.start as u32).saturating_mul(cell_width));
            let end = x_origin.saturating_add((cap.end as u32).saturating_mul(cell_width));
            let inset_y = cap.shell.inset_y.saturating_mul(target.scale);
            let cap_y_start = inset_y.min(target.height);
            let cap_y_end = target.height.saturating_sub(inset_y).max(cap_y_start);
            let cap_height = cap_y_end.saturating_sub(cap_y_start);
            let cap_radius = cap
                .shell
                .radius
                .map(|r| r.saturating_mul(target.scale))
                .unwrap_or(cap_height / 2);
            fill_rounded_rect(
                target.canvas,
                target.width,
                target.height,
                Placement {
                    clip_start: start,
                    clip_end: end,
                },
                start..end,
                cap_y_start..cap_y_end,
                radius,
                cap_radius,
                cap.shell.background,
            );
        }
        let scale = target.scale as f32;
        let cap_height = (self.cap_height * scale).min(line_height);
        let baseline = (self.baseline * scale).min(line_height);
        let surface = Placement {
            clip_start: 0,
            clip_end: target.width,
        };
        let cell_top = i64::from(y_start);
        let cell_bottom = i64::from(y_end);
        for run in &image_runs {
            let visible = (x_origin.saturating_add((run.start as u32).saturating_mul(cell_width)))
                ..(x_origin.saturating_add((run.end as u32).saturating_mul(cell_width)));
            if let Some(background) = run.style.background {
                fill_rect(
                    target.canvas,
                    target.width,
                    target.height,
                    Placement {
                        clip_start: visible.start,
                        clip_end: visible.end,
                    },
                    visible.clone(),
                    y_range.clone(),
                    radius,
                    background,
                );
            }
            let mut loaded = false;
            if let Some(store) = self.images.as_mut()
                && let Some((src_w, src_h)) = store.dimensions(&run.image.image.src)
            {
                let layout = ImageLayout {
                    cell_width,
                    origin: x_origin,
                    offset_y: i64::from(y_offset),
                    line_height,
                    cap_height,
                    baseline,
                };
                let geometry = image_geometry(run, (src_w, src_h), layout);
                if let Some(geometry) = geometry
                    && let Some(pixels) =
                        store.scaled(&run.image.image.src, geometry.width, geometry.height)
                {
                    let box_left = x_origin as i64
                        + (run.start as i64 - run.image.index as i64) * cell_width as i64;
                    let box_right = box_left + (run.image.image.width as i64) * cell_width as i64;
                    let cap_clip = cap_runs.iter().find(|c| run.start >= c.start && run.start < c.end).map(|cap| {
                        let c_start = x_origin.saturating_add((cap.start as u32).saturating_mul(cell_width));
                        let c_end = x_origin.saturating_add((cap.end as u32).saturating_mul(cell_width));
                        (c_start, c_end)
                    });
                    let clip = if let Some((c_start, c_end)) = cap_clip {
                        (visible.start.max(box_left.max(0) as u32).max(c_start))
                            ..(visible.end.min(box_right.max(0) as u32).min(c_end))
                    } else {
                        (visible.start.max(box_left.max(0) as u32))
                            ..(visible.end.min(box_right.max(0) as u32))
                    };
                    ready.push(ReadyImage {
                        pixels,
                        paint: ImagePaint {
                            visible: clip,
                            x: geometry.x,
                            y: geometry.y,
                            clip_top: geometry.clip_top,
                            clip_bottom: geometry.clip_bottom,
                            circle: geometry.circle,
                        },
                    });
                    loaded = true;
                }
            }
            if !loaded {
                place_fallback(&mut text_frame, run);
            }
        }
        let runs = frame_runs(&text_frame);
        loop {
            let generation = self.lazy_fonts.generation;
            self.prepare_runs(&runs, target.height, target.scale);
            if generation == self.lazy_fonts.generation {
                break;
            }
            self.prepared = None;
        }

        for image in ready {
            blit_image(
                target.canvas,
                (target.width, target.height),
                radius,
                image.paint,
                &image.pixels,
            );
        }
        let y_offset = y_offset as i32;
        let Some(prepared) = self.prepared.as_mut() else {
            return;
        };
        // Pass 1: Paint all run backgrounds first so glyph ink overhangs
        // (such as wide Nerd Font icons) are never clipped by adjacent run backgrounds.
        for run in &runs {
            if let Some(background) = run.text.style.background {
                let start = x_origin.saturating_add((run.start as u32).saturating_mul(cell_width));
                let end = x_origin.saturating_add((run.end as u32).saturating_mul(cell_width));
                let placement = Placement {
                    clip_start: start,
                    clip_end: end,
                };
                let inset_y = run.text.style.inset_y.saturating_mul(target.scale);
                let is_inside_capsule = frame
                    .cells
                    .get(run.start)
                    .is_some_and(|c| c.capsule.is_some());
                let (base_top, base_bottom) = if is_inside_capsule {
                    (0, target.height)
                } else {
                    (y_start, y_end)
                };
                let inner_y_start = (base_top + inset_y).min(target.height);
                let inner_y_end = base_bottom.saturating_sub(inset_y).max(inner_y_start);
                let inner_y_range = inner_y_start..inner_y_end;

                if let Some(inner_radius) = run.text.style.radius {
                    fill_rounded_rect(
                        target.canvas,
                        target.width,
                        target.height,
                        placement,
                        start..end,
                        inner_y_range,
                        radius,
                        inner_radius.saturating_mul(target.scale),
                        background,
                    );
                } else {
                    fill_rect(
                        target.canvas,
                        target.width,
                        target.height,
                        placement,
                        start..end,
                        inner_y_range,
                        radius,
                        background,
                    );
                }
            }
        }

        // Pass 2: Paint all glyphs and decorations on top.
        for (run, segment) in runs.iter().zip(&mut prepared.segments) {
            let (run_placement, run_cell_top, run_cell_bottom) = if let Some(cap) =
                cap_runs.iter().find(|c| run.start >= c.start && run.start < c.end)
            {
                let start = x_origin.saturating_add((cap.start as u32).saturating_mul(cell_width));
                let end = x_origin.saturating_add((cap.end as u32).saturating_mul(cell_width));
                let inset_y = cap.shell.inset_y.saturating_mul(target.scale);
                let cap_y_start = inset_y.min(target.height);
                let cap_y_end = target.height.saturating_sub(inset_y).max(cap_y_start);
                (
                    Placement {
                        clip_start: start,
                        clip_end: end,
                    },
                    i64::from(cap_y_start),
                    i64::from(cap_y_end),
                )
            } else {
                (surface, cell_top, cell_bottom)
            };

            let color = Color::rgba(
                run.text.style.foreground.red,
                run.text.style.foreground.green,
                run.text.style.foreground.blue,
                run.text.style.foreground.alpha,
            );
            let mut previous = None;
            for glyph in &segment.glyphs {
                let Some((column, last)) = glyph_columns(run, glyph) else {
                    continue;
                };
                let cell_start =
                    x_origin.saturating_add((column as u32).saturating_mul(cell_width));
                let cell_end =
                    x_origin.saturating_add((last as u32).saturating_mul(cell_width));
                let first_x = match previous {
                    Some((start, x)) if start == glyph.start => x,
                    _ => glyph.x,
                };
                previous = Some((glyph.start, first_x));
                let group_width = segment
                    .glyphs
                    .iter()
                    .filter(|other| other.start == glyph.start && other.end == glyph.end)
                    .map(|other| other.x + other.w - first_x)
                    .fold(0.0_f32, f32::max);
                let offset = ((cell_end - cell_start) as f32 - group_width) / 2.0;

                let physical_x = (cell_start as f32 + offset - first_x + glyph.x).round() as i32;
                let physical_y = baseline.round() as i32;
                let subpixel_x = ((((cell_start as f32 + offset - first_x + glyph.x).fract() * 4.0).round() as i32).rem_euclid(4)) as u8;

                let key = GlyphKey {
                    font_id: glyph.font_id,
                    glyph_id: glyph.glyph_id,
                    font_size_bits: (self.font_size * scale).to_bits(),
                    subpixel_x,
                    bold: run.text.style.bold,
                    italic: run.text.style.italic,
                };

                if let Some(cached) =
                    self.cache
                        .get_glyph(&self.font_db, &mut self.scale_context, key)
                {
                    draw_cached_glyph(
                        target.canvas,
                        target.width,
                        target.height,
                        run_placement,
                        radius,
                        physical_x,
                        physical_y + y_offset,
                        run_cell_top,
                        run_cell_bottom,
                        color,
                        cached,
                    );
                }
            }

            for (enabled, is_strike) in [
                (run.text.style.underline, false),
                (run.text.style.strikethrough, true),
            ] {
                if !enabled {
                    continue;
                }
                let rect_start =
                    x_origin.saturating_add((run.start as u32).saturating_mul(cell_width));
                let rect_end =
                    x_origin.saturating_add((run.end as u32).saturating_mul(cell_width));
                let thickness = ((self.font_size * scale * 0.08).ceil() as u32).max(1);
                let y = if is_strike {
                    (baseline - (self.cap_height * scale * 0.5)) as i32 + y_offset
                } else {
                    (baseline + (self.font_size * scale * 0.15)) as i32 + y_offset
                };
                draw_glyph_rect(
                    target.canvas,
                    target.width,
                    target.height,
                    run_placement,
                    radius,
                    rect_start as i32,
                    y,
                    rect_end.saturating_sub(rect_start),
                    thickness,
                    run_cell_top,
                    run_cell_bottom,
                    color,
                );
            }
        }
    }

    pub(crate) fn shape_span(&mut self, text: &str, style: DisplayStyle, _height: u32, scale: u32) -> Vec<ShapedGlyph> {
        let font_size = self.font_size * scale as f32;
        let mapped = self.has_mappings && self.text_matches_mapping(text);

        if !mapped {
            let shaped = self.font_db.with_face_data(self.primary_face, |data, index| {
                let font_ref = swash::FontRef::from_index(data, index as usize)?;
                let mut shaper = self
                    .shape_context
                    .builder_with_id(font_ref, font_cache_id(self.primary_face, index))
                    .size(font_size)
                    .build();
                shaper.add_str(text);
                let mut text_glyphs = Vec::new();
                shaper.shape_with(|c| {
                    for g in c.glyphs {
                        text_glyphs.push((g.id, g.advance, g.x, g.y, c.source.start as usize, c.source.end as usize));
                    }
                });
                Some(text_glyphs)
            }).flatten();

            if let Some(shaped_glyphs) = shaped {
                let missing = shaped_glyphs.iter().any(|(id, ..)| *id == 0);
                if !missing {
                    let mut glyphs = Vec::with_capacity(shaped_glyphs.len());
                    let mut current_x = 0.0_f32;
                    for (g_id, advance, offset_x, offset_y, start, end) in shaped_glyphs {
                        glyphs.push(ShapedGlyph {
                            font_id: self.primary_face,
                            glyph_id: g_id,
                            x: current_x + offset_x,
                            y: offset_y,
                            w: advance,
                            start,
                            end,
                        });
                        current_x += advance;
                    }
                    self.lazy_fonts.load_used_styles(&mut self.font_db, &glyphs, style);
                    return glyphs;
                }
            }
        }

        let mut glyphs = Vec::new();
        let mut current_x = 0.0_f32;

        for (cluster_start, cluster) in text.grapheme_indices(true) {
            let cluster_end = cluster_start + cluster.len();
            let mut chosen_face = None;

            if self.has_mappings
                && let Some(family_idx) = self.family_for_cluster(cluster)
            {
                let family_name = &self.lazy_fonts.families[family_idx];
                let query = fontdb::Query {
                    families: &[fontdb::Family::Name(family_name)],
                    weight: if style.bold { fontdb::Weight::BOLD } else { fontdb::Weight::NORMAL },
                    style: if style.italic { fontdb::Style::Italic } else { fontdb::Style::Normal },
                    ..Default::default()
                };
                if let Some(id) = self.font_db.query(&query) {
                    chosen_face = Some(id);
                }
            }

            if chosen_face.is_none() {
                let primary_query = fontdb::Query {
                    families: &[fontdb::Family::Name(&self.family)],
                    weight: if style.bold { fontdb::Weight::BOLD } else { fontdb::Weight::NORMAL },
                    style: if style.italic { fontdb::Style::Italic } else { fontdb::Style::Normal },
                    ..Default::default()
                };
                if let Some(id) = self.font_db.query(&primary_query) {
                    let all_covered = cluster.chars().all(|c| {
                        self.font_db.with_face_data(id, |data, index| {
                            ttf_parser::Face::parse(data, index).is_ok_and(|f| f.glyph_index(c).is_some())
                        }).unwrap_or(false)
                    });
                    if all_covered {
                        chosen_face = Some(id);
                    }
                }
            }

            if chosen_face.is_none() {
                for &family_idx in &self.fallback_indices {
                    self.lazy_fonts.load_regular(&mut self.font_db, family_idx);
                    let family_name = &self.lazy_fonts.families[family_idx];
                    let query = fontdb::Query {
                        families: &[fontdb::Family::Name(family_name)],
                        weight: if style.bold { fontdb::Weight::BOLD } else { fontdb::Weight::NORMAL },
                        style: if style.italic { fontdb::Style::Italic } else { fontdb::Style::Normal },
                        ..Default::default()
                    };
                    if let Some(id) = self.font_db.query(&query) {
                        let all_covered = cluster.chars().all(|c| {
                            self.font_db.with_face_data(id, |data, index| {
                                ttf_parser::Face::parse(data, index).is_ok_and(|f| f.glyph_index(c).is_some())
                            }).unwrap_or(false)
                        });
                        if all_covered {
                            chosen_face = Some(id);
                            break;
                        }
                    }
                }
            }

            let face_id = chosen_face.unwrap_or_else(|| {
                let query = fontdb::Query {
                    families: &[fontdb::Family::Name(&self.family)],
                    weight: if style.bold { fontdb::Weight::BOLD } else { fontdb::Weight::NORMAL },
                    style: if style.italic { fontdb::Style::Italic } else { fontdb::Style::Normal },
                    ..Default::default()
                };
                self.font_db.query(&query).or_else(|| self.font_db.faces().next().map(|f| f.id)).expect("at least one font loaded")
            });

            let shaped = self.font_db.with_face_data(face_id, |data, index| {
                let font_ref = swash::FontRef::from_index(data, index as usize)?;
                let mut shaper = self
                    .shape_context
                    .builder_with_id(font_ref, font_cache_id(face_id, index))
                    .size(font_size)
                    .build();
                shaper.add_str(cluster);
                let mut cluster_glyphs = Vec::new();
                shaper.shape_with(|c| {
                    for g in c.glyphs {
                        cluster_glyphs.push((g.id, g.advance, g.x, g.y));
                    }
                });
                Some(cluster_glyphs)
            }).flatten().unwrap_or_default();

            for (g_id, advance, offset_x, offset_y) in shaped {
                glyphs.push(ShapedGlyph {
                    font_id: face_id,
                    glyph_id: g_id,
                    x: current_x + offset_x,
                    y: offset_y,
                    w: advance,
                    start: cluster_start,
                    end: cluster_end,
                });
                current_x += advance;
            }
        }

        self.lazy_fonts.load_used_styles(&mut self.font_db, &glyphs, style);
        glyphs
    }

    fn prepare_runs(&mut self, runs: &[FrameRun], height: u32, scale: u32) {
        if let Some(prepared) = &self.prepared
            && prepared.height == height
            && prepared.scale == scale
            && prepared.segments.len() == runs.len()
            && prepared
                .segments
                .iter()
                .zip(runs)
                .all(|(seg, run)| seg.text == run.text.text && seg.style == run.text.style)
        {
            return;
        }

        let mut old_segments = if let Some(prepared) = self.prepared.take() {
            if prepared.height == height && prepared.scale == scale {
                prepared.segments
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        let mut segments = Vec::with_capacity(runs.len());
        for run in runs {
            if let Some(pos) = old_segments
                .iter()
                .position(|seg| seg.text == run.text.text && seg.style == run.text.style)
            {
                segments.push(old_segments.swap_remove(pos));
            } else {
                let glyphs = self.shape_span(&run.text.text, run.text.style, height, scale);
                segments.push(PreparedSegment {
                    text: run.text.text.clone(),
                    style: run.text.style,
                    glyphs,
                });
            }
        }

        self.prepared = Some(PreparedRegion {
            height,
            scale,
            segments,
        });
    }

    #[cfg(test)]
    pub fn prepare(&mut self, spans: &[StyledText], height: u32, scale: u32) {
        if let Some(prepared) = &self.prepared
            && prepared.height == height
            && prepared.scale == scale
            && prepared.segments.len() == spans.len()
            && prepared
                .segments
                .iter()
                .zip(spans)
                .all(|(seg, span)| seg.text == span.text && seg.style == span.style)
        {
            return;
        }

        let mut old_segments = if let Some(prepared) = self.prepared.take() {
            if prepared.height == height && prepared.scale == scale {
                prepared.segments
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        let mut segments = Vec::with_capacity(spans.len());
        for span in spans {
            if let Some(pos) = old_segments
                .iter()
                .position(|seg| seg.text == span.text && seg.style == span.style)
            {
                segments.push(old_segments.swap_remove(pos));
            } else {
                let glyphs = self.shape_span(&span.text, span.style, height, scale);
                segments.push(PreparedSegment {
                    text: span.text.clone(),
                    style: span.style,
                    glyphs,
                });
            }
        }

        self.prepared = Some(PreparedRegion {
            height,
            scale,
            segments,
        });
    }

    pub(crate) fn text_matches_mapping(&self, text: &str) -> bool {
        text.chars().any(|c| {
            self.candidates.iter().any(|candidate| {
                matches!(candidate, FontCandidate::Mapping { matchers, .. }
                    if matchers.iter().any(|matcher| matcher.matches(c)))
            })
        })
    }

    pub(crate) fn family_for_cluster(&mut self, cluster: &str) -> Option<usize> {
        for candidate in &self.candidates {
            let family = match candidate {
                FontCandidate::Family(family) => *family,
                FontCandidate::Mapping { matchers, family } => {
                    if !cluster
                        .chars()
                        .any(|c| matchers.iter().any(|matcher| matcher.matches(c)))
                    {
                        continue;
                    }
                    *family
                }
            };
            self.lazy_fonts.load_regular(&mut self.font_db, family);
            if cluster
                .chars()
                .filter(|c| !matches!(*c, '\u{200d}' | '\u{fe0e}' | '\u{fe0f}'))
                .all(|c| self.lazy_fonts.has_glyph(&self.font_db, family, c))
            {
                return Some(family);
            }
        }
        None
    }
}
