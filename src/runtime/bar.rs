use std::{
    ops::Range,
    time::{Duration, Instant},
};

use smithay_client_toolkit::{
    compositor::{CompositorState, FrameCallbackData, Region as WaylandRegion},
    shell::{WaylandSurface, wlr_layer::LayerSurface},
    shm::slot::SlotPool,
};
use wayland_client::{
    QueueHandle,
    protocol::{wl_output, wl_shm, wl_surface},
};

use crate::{
    cell_frame::{CapsuleShell, CellFrame, RichRegions, RichSpan, layout_rich_regions, text_cell_width},
    config::{BarLayer, BarMargins, BarPosition, DisplayAlign, Region, Theme},
    markup::{MarkupPart, MarkupSpan},
    render::{PaintTarget, TextRenderer, capsule_runs},
    runtime::model::truncate_to_width,
    runtime::{
        Runtime,
        model::{WidgetContent, WidgetDefinition, WidgetState},
    },
};

#[derive(Clone, Debug, PartialEq)]
pub struct RegionStrings {
    pub left: Vec<RichSpan>,
    pub center: Vec<RichSpan>,
    pub right: Vec<RichSpan>,
}

/// Geometry needed to convert logical cell ranges into buffer damage rectangles.
#[derive(Clone, Copy)]
struct DamageGeometry {
    columns: usize,
    origin: u32,
    cell_width: u32,
    logical_width: u32,
    scale: u32,
    width: i32,
    height: i32,
}

impl DamageGeometry {
    fn rect(self, range: Option<Range<usize>>) -> (i32, i32, i32, i32) {
        compute_damage_rect(
            range,
            self.columns,
            self.origin,
            self.cell_width,
            self.logical_width,
            self.scale,
            self.width,
            self.height,
        )
    }
}

pub struct Bar {
    pub id: u64,
    pub name: Option<String>,
    pub height: Option<u32>,
    pub position: BarPosition,
    pub layer_level: BarLayer,
    pub margin: BarMargins,
    pub exclusive: bool,
    pub theme: Option<Theme>,
    pub widget_definitions: Vec<WidgetDefinition>,
    pub output: wl_output::WlOutput,
    pub output_name: String,
    pub layer: LayerSurface,
    pub pool: SlotPool,
    pub logical_width: u32,
    pub logical_height: u32,
    pub scale: i32,
    pub configured: bool,
    pub hidden: bool,
    pub auto_hide: Option<Duration>,
    pub auto_hide_deadline: Option<Instant>,
    pub pointer_inside: bool,
    pub dirty: bool,
    pub frame_pending: bool,
    pub widgets: Vec<WidgetState>,
    pub frame_scratch: CellFrame,
    pub last_frame: Option<CellFrame>,
    pub last_geometry: Option<(u32, u32, i32)>,
    pub last_image_generation: u64,
    pub static_spans: Vec<Option<Vec<RichSpan>>>,
    pub widget_ranges: Vec<Option<std::ops::Range<usize>>>,
}

/// Returns the single outermost enclosing root scope style name if ALL spans in this widget
/// are completely enclosed within the exact same root scope and that scope has a style.
fn enclosing_root_scope_style(raw_spans: &[MarkupSpan]) -> Option<&str> {
    if raw_spans.is_empty() {
        return None;
    }
    let first_root_id = raw_spans[0].root_scope_id?;
    let first_style = raw_spans[0].root_scope_style.as_deref()?;
    for span in &raw_spans[1..] {
        if span.root_scope_id != Some(first_root_id) {
            return None;
        }
    }
    Some(first_style)
}

pub(crate) fn build_region_strings(
    widgets: &[WidgetState],
    static_spans: &mut Vec<Option<Vec<RichSpan>>>,
    theme: &Theme,
) -> RegionStrings {
    let mut left: Vec<RichSpan> = Vec::new();
    let mut center: Vec<RichSpan> = Vec::new();
    let mut right: Vec<RichSpan> = Vec::new();
    if static_spans.len() != widgets.len() {
        static_spans.resize_with(widgets.len(), || None);
    }
    for (index, widget) in widgets.iter().enumerate() {
        let target = match widget.region {
            Region::Left => &mut left,
            Region::Center => &mut center,
            Region::Right => &mut right,
        };
        let is_static = widget.pushed_markup.is_none()
            && matches!(
                widget.content,
                WidgetContent::Markup(_) | WidgetContent::Image(_)
            );
        if is_static && let Some(cached) = &static_spans[index] {
            target.extend(cached.iter().cloned());
            continue;
        }
        let raw_spans = widget.spans();

        let capsule_shell = enclosing_root_scope_style(&raw_spans)
            .and_then(|name| theme.style_for(Some(name)))
            .and_then(|s| {
                s.background.map(|bg| CapsuleShell {
                    background: bg,
                    inset_y: s.inset_y,
                    radius: s.radius,
                })
            });

        let mut remaining = widget.max_width.unwrap_or(usize::MAX);
        let mut spans = Vec::new();
        let mut width = 0;
        for m_span in raw_spans {
            if remaining == 0 {
                break;
            }
            let mut span_style = m_span
                .style
                .as_deref()
                .and_then(|s| theme.style_for(Some(s)))
                .unwrap_or(theme.default_style);

            if let Some(shell) = capsule_shell {
                if !m_span.is_sub_scope {
                    span_style.background = None;
                } else if span_style.background.is_some() {
                    if span_style.inset_y == 0 {
                        span_style.inset_y = shell.inset_y;
                    } else {
                        span_style.inset_y = shell.inset_y.saturating_add(span_style.inset_y);
                    }
                }
            }

            let target_action = m_span.target;
            let (p, visible_width, count) = match m_span.part {
                MarkupPart::Text(mut t) => {
                    if let Some(m) = m_span.max_width {
                        let text_w = text_cell_width(&t);
                        if text_w > m {
                            t = truncate_to_width(&t, m).into_owned();
                        }
                    }
                    if let Some(min) = m_span.min_width {
                        let text_w = text_cell_width(&t);
                        if min > text_w {
                            let padding = min - text_w;
                            let align = m_span.align.unwrap_or(DisplayAlign::Left);
                            let left_pad = match align {
                                DisplayAlign::Left => 0,
                                DisplayAlign::Center => padding / 2,
                                DisplayAlign::Right => padding,
                            };
                            let right_pad = padding - left_pad;
                            t = format!("{}{}{}", " ".repeat(left_pad), t, " ".repeat(right_pad));
                        }
                    }
                    let text_w = text_cell_width(&t);
                    if text_w > remaining {
                        let t_trunc = truncate_to_width(&t, remaining).into_owned();
                        let c = text_cell_width(&t_trunc);
                        (crate::images::Part::Text(t_trunc), None, c)
                    } else {
                        (crate::images::Part::Text(t), None, text_w)
                    }
                }
                MarkupPart::Image(img) => {
                    let visible = img.width.min(remaining);
                    (crate::images::Part::Image(img), Some(visible), visible)
                }
            };
            remaining = remaining.saturating_sub(count);
            width += count;
            spans.push(RichSpan {
                part: p,
                visible_width,
                style: span_style,
                capsule: capsule_shell,
                owner: Some(index),
                target: target_action,
            });
        }

        let widget_spans = if let Some(min) = widget.min_width
            && min > width
        {
            let padding = min - width;
            let left_pad = match widget.align {
                DisplayAlign::Left => 0,
                DisplayAlign::Center => padding / 2,
                DisplayAlign::Right => padding,
            };
            let make_space = |count| RichSpan {
                part: crate::images::Part::Text(" ".repeat(count)),
                visible_width: None,
                style: theme.default_style,
                capsule: capsule_shell,
                owner: Some(index),
                target: None,
            };
            let mut padded_spans = Vec::with_capacity(spans.len() + 2);
            if left_pad > 0 {
                padded_spans.push(make_space(left_pad));
            }
            padded_spans.extend(spans);
            if padding > left_pad {
                padded_spans.push(make_space(padding - left_pad));
            }
            padded_spans
        } else {
            spans
        };

        if is_static {
            static_spans[index] = Some(widget_spans.clone());
        }
        target.extend(widget_spans);
    }
    RegionStrings {
        left,
        center,
        right,
    }
}

impl Bar {
    pub fn region_strings(&mut self, theme: &Theme) -> RegionStrings {
        build_region_strings(&self.widgets, &mut self.static_spans, theme)
    }

    pub fn render(
        &mut self,
        theme: &Theme,
        renderer: &mut TextRenderer,
        queue_handle: &QueueHandle<Runtime>,
        compositor: &CompositorState,
    ) -> Result<(), String> {
        let background = theme.background;
        let radius = theme.radius;
        let default_style = theme.default_style;
        let regions = self.region_strings(theme);
        let (columns, origin) =
            renderer.cell_geometry(self.logical_width, theme.padding.horizontal);
        self.frame_scratch.reset(columns, default_style);
        layout_rich_regions(
            &mut self.frame_scratch,
            RichRegions {
                left: &regions.left,
                center: &regions.center,
                right: &regions.right,
            },
        );
        self.widget_ranges = self.frame_scratch.owner_ranges(self.widgets.len());
        let geometry = (self.logical_width, self.logical_height, self.scale);
        let image_generation = renderer
            .images
            .as_ref()
            .map_or(0, |store| store.generation());
        if self.last_frame.as_ref() == Some(&self.frame_scratch)
            && self.last_geometry == Some(geometry)
            && self.last_image_generation == image_generation
        {
            self.dirty = false;
            return Ok(());
        }

        let scale =
            u32::try_from(self.scale).map_err(|error| format!("invalid output scale: {error}"))?;
        let width = self
            .logical_width
            .checked_mul(scale)
            .ok_or_else(|| "bar buffer width overflow".to_owned())?;
        let height = self
            .logical_height
            .checked_mul(scale)
            .ok_or_else(|| "bar buffer height overflow".to_owned())?;
        let stride = width
            .checked_mul(4)
            .and_then(|stride| i32::try_from(stride).ok())
            .ok_or_else(|| "bar buffer stride overflow".to_owned())?;
        let width_i32 =
            i32::try_from(width).map_err(|error| format!("bar buffer width overflow: {error}"))?;
        let height_i32 = i32::try_from(height)
            .map_err(|error| format!("bar buffer height overflow: {error}"))?;

        let (buffer, canvas) = self
            .pool
            .create_buffer(width_i32, height_i32, stride, wl_shm::Format::Argb8888)
            .map_err(|error| {
                format!(
                    "cannot allocate bar buffer for {}: {error}",
                    self.output_name
                )
            })?;

        renderer.paint_frame(
            PaintTarget {
                canvas,
                width,
                height,
                scale,
                background,
                radius,
            },
            &self.frame_scratch,
            origin,
        );

        let damage_ranges = if self.last_geometry == Some(geometry)
            && self.last_image_generation == image_generation
        {
            self.last_frame
                .as_ref()
                .and_then(|last| last.diff_ranges(&self.frame_scratch))
        } else {
            None
        };
        submit_damage_ranges(
            self.layer.wl_surface(),
            damage_ranges.as_deref(),
            DamageGeometry {
                columns,
                origin,
                cell_width: renderer.cell_width(),
                logical_width: self.logical_width,
                scale,
                width: width_i32,
                height: height_i32,
            },
        );
        self.layer.wl_surface().frame(
            queue_handle,
            FrameCallbackData(self.layer.wl_surface().clone()),
        );

        if theme.background.alpha < 255 {
            if let Ok(region) = WaylandRegion::new(compositor) {
                let cell_w = renderer.cell_width();
                for cap in capsule_runs(&self.frame_scratch) {
                    let x = origin.saturating_add((cap.start as u32).saturating_mul(cell_w));
                    let w = ((cap.end - cap.start) as u32).saturating_mul(cell_w);
                    let inset_y = cap.shell.inset_y;
                    let y = inset_y.min(self.logical_height);
                    let h = self.logical_height.saturating_sub(2 * inset_y);
                    if let (Ok(x_i32), Ok(y_i32), Ok(w_i32), Ok(h_i32)) = (
                        i32::try_from(x),
                        i32::try_from(y),
                        i32::try_from(w),
                        i32::try_from(h),
                    ) {
                        region.add(x_i32, y_i32, w_i32, h_i32);
                    }
                }
                for range in self.widget_ranges.iter().flatten() {
                    let mut uncap_start = None;
                    for col in range.clone() {
                        let is_in_capsule = self
                            .frame_scratch
                            .cells
                            .get(col)
                            .is_some_and(|c| c.capsule.is_some());
                        if !is_in_capsule {
                            if uncap_start.is_none() {
                                uncap_start = Some(col);
                            }
                        } else if let Some(start) = uncap_start.take() {
                            let x = origin.saturating_add((start as u32).saturating_mul(cell_w));
                            let w = ((col - start) as u32).saturating_mul(cell_w);
                            if let (Ok(x_i32), Ok(w_i32), Ok(h_i32)) = (
                                i32::try_from(x),
                                i32::try_from(w),
                                i32::try_from(self.logical_height),
                            ) {
                                region.add(x_i32, 0, w_i32, h_i32);
                            }
                        }
                    }
                    if let Some(start) = uncap_start {
                        let x = origin.saturating_add((start as u32).saturating_mul(cell_w));
                        let w = ((range.end - start) as u32).saturating_mul(cell_w);
                        if let (Ok(x_i32), Ok(w_i32), Ok(h_i32)) = (
                            i32::try_from(x),
                            i32::try_from(w),
                            i32::try_from(self.logical_height),
                        ) {
                            region.add(x_i32, 0, w_i32, h_i32);
                        }
                    }
                }
                self.layer.wl_surface().set_input_region(Some(region.wl_region()));
            }
        } else {
            self.layer.wl_surface().set_input_region(None);
        }

        buffer.attach_to(self.layer.wl_surface()).map_err(|error| {
            format!("cannot attach bar buffer for {}: {error}", self.output_name)
        })?;
        self.layer.commit();
        if let Some(last) = &mut self.last_frame {
            std::mem::swap(last, &mut self.frame_scratch);
        } else {
            self.last_frame = Some(self.frame_scratch.clone());
        }
        self.last_geometry = Some(geometry);
        self.last_image_generation = image_generation;
        self.dirty = false;
        self.frame_pending = true;
        Ok(())
    }

    /// Finds the widget index at a given column.
    #[allow(dead_code)]
    pub fn widget_at_column(&self, col: usize) -> Option<usize> {
        self.frame_scratch.widget_at_column(col)
    }

    /// Gets the column range occupied by a given widget index.
    #[allow(dead_code)]
    pub fn widget_column_range(&self, widget_index: usize) -> Option<std::ops::Range<usize>> {
        self.widget_ranges.get(widget_index).cloned().flatten()
    }

    /// Hit-tests a surface-local coordinate (in logical pixels) to a widget index and sub-target.
    pub fn hit_test_widget(
        &self,
        surface_x: f64,
        origin: u32,
        cell_width: u32,
    ) -> Option<(usize, Option<String>)> {
        self.frame_scratch.hit_test(surface_x, origin, cell_width)
    }
}

/// Converts one logical column range into a physical buffer damage rectangle.
/// `None` intentionally means full damage.
#[allow(clippy::too_many_arguments)]
pub fn compute_damage_rect(
    col_range: Option<std::ops::Range<usize>>,
    columns: usize,
    origin: u32,
    cell_width: u32,
    logical_width: u32,
    scale: u32,
    width_i32: i32,
    height_i32: i32,
) -> (i32, i32, i32, i32) {
    if let Some(range) = col_range {
        let logical_start = if range.start == 0 {
            0
        } else {
            origin.saturating_add((range.start as u32).saturating_mul(cell_width))
        };
        let logical_end = if range.end >= columns {
            logical_width
        } else {
            origin.saturating_add((range.end as u32).saturating_mul(cell_width))
        };
        // Bleed margin of 2 cells on each side to accommodate glyph ink overhangs
        // (such as 1-cell Nerd Font icons like WiFi / volume that draw across up to ~2 cells,
        // italic slant, and negative bearings) both when newly drawn and when erased.
        let bleed_px = (cell_width.saturating_mul(2).saturating_mul(scale)) as i32;
        let start_px = ((logical_start.saturating_mul(scale)) as i32)
            .saturating_sub(if range.start == 0 { 0 } else { bleed_px })
            .max(0);
        let end_px = ((logical_end.saturating_mul(scale)) as i32)
            .saturating_add(if range.end >= columns { 0 } else { bleed_px })
            .min(width_i32);
        let damage_x = start_px.min(width_i32);
        let damage_width = (end_px - start_px)
            .max(0)
            .min(width_i32.saturating_sub(damage_x));
        if damage_width > 0 {
            return (damage_x, 0, damage_width, height_i32);
        }
    }
    (0, 0, width_i32, height_i32)
}

/// Coalesces column ranges whose 2-cell bleed margins would touch or overlap.
fn coalesce_damage_ranges(ranges: &[Range<usize>]) -> Vec<Range<usize>> {
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
    for range in ranges {
        if let Some(prev) = merged.last_mut()
            && range.start <= prev.end.saturating_add(4)
        {
            prev.end = prev.end.max(range.end);
        } else {
            merged.push(range.clone());
        }
    }
    merged
}

/// Submits each disjoint range separately; missing ranges mean full damage.
fn submit_damage_ranges(
    surface: &wl_surface::WlSurface,
    ranges: Option<&[Range<usize>]>,
    geometry: DamageGeometry,
) {
    if let Some(ranges) = ranges.filter(|ranges| !ranges.is_empty()) {
        for range in coalesce_damage_ranges(ranges) {
            submit_damage_range(surface, Some(range), geometry);
        }
    } else {
        submit_damage_range(surface, None, geometry);
    }
}

fn submit_damage_range(
    surface: &wl_surface::WlSurface,
    range: Option<Range<usize>>,
    geometry: DamageGeometry,
) {
    let (damage_x, damage_y, damage_width, damage_height) = geometry.rect(range);
    surface.damage_buffer(damage_x, damage_y, damage_width, damage_height);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_damage_rect_full_when_no_col_range() {
        let rect = compute_damage_rect(None, 100, 10, 8, 820, 1, 820, 30);
        assert_eq!(rect, (0, 0, 820, 30));
    }

    #[test]
    fn test_damage_rect_partial_middle() {
        // columns 10..20, origin 10, cell_width 8
        // logical_start = 10 + 10 * 8 = 90 (sub 2-cell bleed 16px = 74)
        // logical_end = 10 + 20 * 8 = 170 (add 2-cell bleed 16px = 186)
        // damage_width = 186 - 74 = 112
        let rect = compute_damage_rect(Some(10..20), 100, 10, 8, 820, 1, 820, 30);
        assert_eq!(rect, (74, 0, 112, 30));
    }

    #[test]
    fn test_damage_rect_partial_left_edge_covers_origin() {
        // columns 0..5, origin 10, cell_width 8
        // logical_start = 0 (covers padding / corner)
        // logical_end = 10 + 5 * 8 = 50 (add 2-cell bleed 16px = 66)
        let rect = compute_damage_rect(Some(0..5), 100, 10, 8, 820, 1, 820, 30);
        assert_eq!(rect, (0, 0, 66, 30));
    }

    #[test]
    fn test_damage_rect_partial_right_edge_covers_logical_width() {
        // columns 95..100, origin 10, cell_width 8, total logical 820
        // logical_start = 10 + 95 * 8 = 770 (sub 2-cell bleed 16px = 754)
        // logical_end = 820 (covers right padding / corner)
        let rect = compute_damage_rect(Some(95..100), 100, 10, 8, 820, 1, 820, 30);
        assert_eq!(rect, (754, 0, 66, 30));
    }

    #[test]
    fn test_damage_rect_scales_with_output_scale() {
        // scale = 2
        // logical_start = 90 -> 180 px (sub 2-cell bleed 32px = 148)
        // logical_end = 170 -> 340 px (add 2-cell bleed 32px = 372)
        // width = 224 px
        let rect = compute_damage_rect(Some(10..20), 100, 10, 8, 820, 2, 1640, 60);
        assert_eq!(rect, (148, 0, 224, 60));
    }

    #[test]
    fn test_coalesce_damage_ranges_merges_overlapping_bleed() {
        let merged = coalesce_damage_ranges(&[2..4, 6..8, 20..22]);
        assert_eq!(merged, vec![2..8, 20..22]);
    }

    #[test]
    fn test_scope_container_resolution() {
        use std::path::Path;
        use crate::config::{DisplayStyle, Rgba};
        use crate::markup::parse_markup;

        let pill_bg: Rgba = "#1e1e2e".parse().unwrap();
        let active_bg: Rgba = "#89b4fa".parse().unwrap();
        let fg: Rgba = "#cdd6f4".parse().unwrap();

        let mut theme = Theme::default();
        theme.styles.insert(
            "pill".into(),
            DisplayStyle {
                foreground: fg,
                background: Some(pill_bg),
                bold: false,
                italic: false,
                underline: false,
                strikethrough: false,
                inset_y: 0,
                radius: Some(16),
            },
        );
        theme.styles.insert(
            "active".into(),
            DisplayStyle {
                foreground: fg,
                background: Some(active_bg),
                bold: true,
                italic: false,
                underline: false,
                strikethrough: false,
                inset_y: 0,
                radius: None,
            },
        );

        let spans = parse_markup("#player(@pill){ [⏮] [ ⏸ ](@active) [⏭] }", Path::new(".")).unwrap();
        let widgets = vec![WidgetState {
            id: "player".into(),
            region: Region::Center,
            min_width: None,
            align: DisplayAlign::Left,
            max_width: None,
            content: WidgetContent::Markup(spans),
            pushed_markup: None,
        }];

        let mut static_spans = Vec::new();
        let regions = build_region_strings(&widgets, &mut static_spans, &theme);
        // ["⏮", " ⏸ ", "⏭"]
        assert_eq!(regions.center.len(), 3);
        let expected_shell = CapsuleShell {
            background: pill_bg,
            inset_y: 0,
            radius: Some(16),
        };
        for span in &regions.center {
            assert_eq!(span.capsule, Some(expected_shell));
        }
        // Outer scope nodes have their cell background cleared (absorbed by container shell)
        assert_eq!(regions.center[0].style.background, None);
        assert_eq!(regions.center[2].style.background, None);
        // Inner overridden node retains its cell highlight background
        assert_eq!(regions.center[1].style.background, Some(active_bg));
    }

    #[test]
    fn test_workspaces_active_highlight_in_scope_container() {
        use std::path::Path;
        use crate::config::{DisplayStyle, Rgba};
        use crate::markup::parse_markup;

        let surface_bg: Rgba = "#1e1e2e".parse().unwrap();
        let accent_blue: Rgba = "#89b4fa".parse().unwrap();
        let muted_color: Rgba = "#6c7086".parse().unwrap();

        let mut theme = Theme::default();
        theme.styles.insert(
            "accent".into(),
            DisplayStyle {
                foreground: "#11111b".parse().unwrap(),
                background: Some(accent_blue),
                bold: true,
                italic: false,
                underline: false,
                strikethrough: false,
                inset_y: 0,
                radius: None,
            },
        );
        theme.styles.insert(
            "muted".into(),
            DisplayStyle {
                foreground: muted_color,
                background: None,
                bold: false,
                italic: false,
                underline: false,
                strikethrough: false,
                inset_y: 0,
                radius: None,
            },
        );
        theme.styles.insert(
            "ws_pill".into(),
            DisplayStyle {
                foreground: "#cdd6f4".parse().unwrap(),
                background: Some(surface_bg),
                bold: false,
                italic: false,
                underline: false,
                strikethrough: false,
                inset_y: 0,
                radius: Some(8),
            },
        );
        theme.styles.insert(
            "ws_active".into(),
            DisplayStyle {
                foreground: "#11111b".parse().unwrap(),
                background: Some(accent_blue),
                bold: true,
                italic: false,
                underline: false,
                strikethrough: false,
                inset_y: 0,
                radius: None,
            },
        );

        // Case 1: Workspaces output without an enclosing root scope: NOT a container!
        let markup_raw = "#ws:1{ [ 1 ](@accent) }[ ](@muted)#ws:2{ [2](@muted) }";
        let spans_raw = parse_markup(markup_raw, Path::new(".")).unwrap();
        let widgets_raw = vec![WidgetState {
            id: "workspaces".into(),
            region: Region::Left,
            min_width: None,
            align: DisplayAlign::Left,
            max_width: None,
            content: WidgetContent::Markup(spans_raw),
            pushed_markup: None,
        }];
        let mut static_spans = Vec::new();
        let regions_raw = build_region_strings(&widgets_raw, &mut static_spans, &theme);
        assert_eq!(regions_raw.left.len(), 3);
        assert_eq!(regions_raw.left[0].capsule, None);
        assert_eq!(regions_raw.left[1].capsule, None);
        assert_eq!(regions_raw.left[2].capsule, None);

        // Case 2: Workspaces output fully enclosed by a root scope `#workspaces(@ws_pill){ ... }`
        let markup_pill = "#workspaces(@ws_pill){ #ws:1{ [ 1 ](@ws_active) }[ ](@muted)#ws:2{ [2](@muted) } }";
        let spans_pill = parse_markup(markup_pill, Path::new(".")).unwrap();
        let widgets_pill = vec![WidgetState {
            id: "workspaces".into(),
            region: Region::Left,
            min_width: None,
            align: DisplayAlign::Left,
            max_width: None,
            content: WidgetContent::Markup(spans_pill),
            pushed_markup: None,
        }];
        let mut static_spans2 = Vec::new();
        let regions_pill = build_region_strings(&widgets_pill, &mut static_spans2, &theme);

        assert_eq!(regions_pill.left.len(), 3);
        let surface_shell = CapsuleShell {
            background: surface_bg,
            inset_y: 0,
            radius: Some(8),
        };
        for span in &regions_pill.left {
            assert_eq!(span.capsule, Some(surface_shell));
        }

        // Active workspace (index 0: " 1 ") keeps its @ws_active cell highlight!
        assert_eq!(regions_pill.left[0].part, crate::images::Part::Text(" 1 ".into()));
        assert_eq!(regions_pill.left[0].style.background, Some(accent_blue));

        // Inactive workspace (index 2: "2") has cell background None (transparents to container shell)
        assert_eq!(regions_pill.left[2].part, crate::images::Part::Text("2".into()));
        assert_eq!(regions_pill.left[2].style.background, None);
    }

    #[test]
    fn test_nested_scope_inset_y_accumulation() {
        use std::path::Path;
        use crate::config::{DisplayStyle, Rgba};
        use crate::markup::parse_markup;

        let outer_bg: Rgba = "#313244".parse().unwrap();
        let badge_bg: Rgba = "#cba6f7".parse().unwrap();
        let fg: Rgba = "#cdd6f4".parse().unwrap();

        let mut theme = Theme::default();
        theme.styles.insert(
            "pill".into(),
            DisplayStyle {
                foreground: fg,
                background: Some(outer_bg),
                bold: false,
                italic: false,
                underline: false,
                strikethrough: false,
                inset_y: 3,
                radius: Some(12),
            },
        );
        theme.styles.insert(
            "badge".into(),
            DisplayStyle {
                foreground: fg,
                background: Some(badge_bg),
                bold: true,
                italic: false,
                underline: false,
                strikethrough: false,
                inset_y: 2,
                radius: Some(4),
            },
        );
        theme.styles.insert(
            "default_h".into(),
            DisplayStyle {
                foreground: fg,
                background: Some(badge_bg),
                bold: false,
                italic: false,
                underline: false,
                strikethrough: false,
                inset_y: 0,
                radius: None,
            },
        );

        let markup = "#box(@pill){ [A] [B](@badge) [C](@default_h) }";
        let spans = parse_markup(markup, Path::new(".")).unwrap();
        let widgets = vec![WidgetState {
            id: "box".into(),
            region: Region::Center,
            min_width: None,
            align: DisplayAlign::Left,
            max_width: None,
            content: WidgetContent::Markup(spans),
            pushed_markup: None,
        }];
        let mut static_spans = Vec::new();
        let regions = build_region_strings(&widgets, &mut static_spans, &theme);

        assert_eq!(regions.center.len(), 3);
        // "A" (outer content): background cleared to container shell
        assert_eq!(regions.center[0].part, crate::images::Part::Text("A".into()));
        assert_eq!(regions.center[0].style.background, None);
        // "B" (nested @badge with explicit inset_y=2): accumulated inset_y = 3 + 2 = 5!
        assert_eq!(regions.center[1].part, crate::images::Part::Text("B".into()));
        assert_eq!(regions.center[1].style.background, Some(badge_bg));
        assert_eq!(regions.center[1].style.inset_y, 5);
        assert_eq!(regions.center[1].style.radius, Some(4));
        // "C" (nested @default_h with inset_y=0): inherits outer inset_y = 3 (equal height)!
        assert_eq!(regions.center[2].part, crate::images::Part::Text("C".into()));
        assert_eq!(regions.center[2].style.background, Some(badge_bg));
        assert_eq!(regions.center[2].style.inset_y, 3);
        assert_eq!(regions.center[2].style.radius, None);
    }

    #[test]
    fn test_text_attribute_max_width_preserves_scope_padding() {
        use std::path::Path;
        use crate::config::{DisplayStyle, Rgba};
        use crate::markup::parse_markup;

        let pill_bg: Rgba = "#1e1e2e".parse().unwrap();
        let fg: Rgba = "#cdd6f4".parse().unwrap();

        let mut theme = Theme::default();
        theme.styles.insert(
            "pill".into(),
            DisplayStyle {
                foreground: fg,
                background: Some(pill_bg),
                bold: false,
                italic: false,
                underline: false,
                strikethrough: false,
                inset_y: 0,
                radius: Some(16),
            },
        );
        theme.styles.insert(
            "subtext".into(),
            DisplayStyle {
                foreground: "#a6adc8".parse().unwrap(),
                background: None,
                bold: false,
                italic: false,
                underline: false,
                strikethrough: false,
                inset_y: 0,
                radius: None,
            },
        );

        // Scope capsule with 2 spaces on left, a long title with text-level max_width=10, and 2 spaces on right
        let markup = "#window(@pill){ [  ][Very Long Window Title That Exceeds Width](@subtext max_width=10)[  ] }";
        let spans = parse_markup(markup, Path::new(".")).unwrap();
        let widgets = vec![WidgetState {
            id: "window".into(),
            region: Region::Left,
            min_width: None,
            align: DisplayAlign::Left,
            max_width: None,
            content: WidgetContent::Markup(spans),
            pushed_markup: None,
        }];
        let mut static_spans = Vec::new();
        let regions = build_region_strings(&widgets, &mut static_spans, &theme);

        // Must retain all 3 spans: left padding, truncated title, and right padding!
        assert_eq!(regions.left.len(), 3);
        assert_eq!(regions.left[0].part, crate::images::Part::Text("  ".into()));
        assert_eq!(regions.left[1].part, crate::images::Part::Text("Very Long…".into()));
        assert_eq!(regions.left[2].part, crate::images::Part::Text("  ".into()));

        // All 3 spans belong to the same capsule shell
        let expected_shell = CapsuleShell {
            background: pill_bg,
            inset_y: 0,
            radius: Some(16),
        };
        for s in &regions.left {
            assert_eq!(s.capsule, Some(expected_shell));
        }
    }
}
