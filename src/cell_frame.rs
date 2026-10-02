use std::{ops::Range, sync::Arc};

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::{
    config::DisplayStyle,
    images::{ImageSpec, Part},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CapsuleShell {
    pub background: crate::config::Rgba,
    pub inset_y: u32,
    pub radius: Option<u32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RichSpan {
    pub part: Part,
    pub visible_width: Option<usize>,
    pub style: DisplayStyle,
    pub capsule: Option<CapsuleShell>,
    pub owner: Option<usize>,
    pub target: Option<String>,
}

pub struct RichRegions<'a> {
    pub left: &'a [RichSpan],
    pub center: &'a [RichSpan],
    pub right: &'a [RichSpan],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageSlice {
    pub image: Arc<ImageSpec>,
    pub index: usize,
    pub placement: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StyledText {
    pub text: String,
    pub style: DisplayStyle,
    pub owner: Option<usize>,
}

#[allow(dead_code)]
pub struct RegionText<'a> {
    pub left: &'a [StyledText],
    pub center: &'a [StyledText],
    pub right: &'a [StyledText],
}

#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub symbol: String,
    pub image: Option<ImageSlice>,
    pub style: DisplayStyle,
    pub capsule: Option<CapsuleShell>,
    pub owner: Option<usize>,
    pub target: Option<String>,
    pub continuation: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CellFrame {
    pub cells: Vec<Cell>,
}

impl CellFrame {
    #[allow(dead_code)]
    pub fn new(columns: usize, default_style: DisplayStyle) -> Self {
        Self {
            cells: vec![
                Cell {
                    symbol: String::new(),
                    image: None,
                    style: default_style,
                    capsule: None,
                    owner: None,
                    target: None,
                    continuation: false,
                };
                columns
            ],
        }
    }

    pub fn reset(&mut self, columns: usize, default_style: DisplayStyle) {
        if self.cells.len() < columns {
            self.cells.resize_with(columns, || Cell {
                symbol: String::new(),
                image: None,
                style: default_style,
                capsule: None,
                owner: None,
                target: None,
                continuation: false,
            });
        } else if self.cells.len() > columns {
            self.cells.truncate(columns);
        }
        for cell in &mut self.cells {
            cell.symbol.clear();
            cell.image = None;
            cell.style = default_style;
            cell.capsule = None;
            cell.owner = None;
            cell.target = None;
            cell.continuation = false;
        }
    }

    /// Returns the disjoint logical column ranges that changed between frames.
    ///
    /// Each range is expanded to include every continuation cell belonging to
    /// a changed wide grapheme. This is required for partial Wayland damage:
    /// the visible symbol is stored only in the leading cell, while the
    /// continuation cells still need to be copied from the new buffer.
    pub fn diff_ranges(&self, other: &Self) -> Option<Vec<Range<usize>>> {
        if self.cells.len() != other.cells.len() {
            let end = self.cells.len().max(other.cells.len());
            return Some(std::iter::once(0..end).collect());
        }

        let mut ranges: Vec<Range<usize>> = Vec::new();
        let mut index = 0;
        while index < self.cells.len() {
            if self.cells[index] == other.cells[index] {
                index += 1;
                continue;
            }

            let mut end = index + 1;
            while end < self.cells.len() && self.cells[end] != other.cells[end] {
                end += 1;
            }

            let changed_range = self.expand_wide_range(other, index..end);
            index = changed_range.end;
            Self::merge_damage_range(&mut ranges, changed_range);
        }

        (!ranges.is_empty()).then_some(ranges)
    }

    /// Expands a changed range to include the continuation cells of a wide
    /// grapheme in either frame.
    fn expand_wide_range(&self, other: &Self, mut range: Range<usize>) -> Range<usize> {
        while range.start > 0
            && (self.cells[range.start].continuation || other.cells[range.start].continuation)
        {
            range.start -= 1;
        }
        while range.end < self.cells.len()
            && (self.cells[range.end].continuation || other.cells[range.end].continuation)
        {
            range.end += 1;
        }
        range
    }

    /// Appends a damage range, merging it with an overlapping or adjacent
    /// range so Wayland receives the smallest practical request set.
    fn merge_damage_range(ranges: &mut Vec<Range<usize>>, range: Range<usize>) {
        if let Some(previous) = ranges.last_mut()
            && range.start <= previous.end
        {
            previous.end = previous.end.max(range.end);
        } else {
            ranges.push(range);
        }
    }

    /// Returns the bounding range of all changed columns.
    #[allow(dead_code)]
    pub fn diff_columns(&self, other: &Self) -> Option<Range<usize>> {
        let ranges = self.diff_ranges(other)?;
        Some(ranges.first()?.start..ranges.last()?.end)
    }

    /// Computes the column range occupied by each widget owner.
    pub fn owner_ranges(&self, count: usize) -> Vec<Option<Range<usize>>> {
        let mut bounds: Vec<Option<(usize, usize)>> = vec![None; count];
        for (col, cell) in self.cells.iter().enumerate() {
            if let Some(owner) = cell.owner
                && owner < count
            {
                match &mut bounds[owner] {
                    Some((_, max_col)) => {
                        *max_col = col;
                    }
                    None => {
                        bounds[owner] = Some((col, col));
                    }
                }
            }
        }
        bounds
            .into_iter()
            .map(|opt| opt.map(|(start, end)| start..end + 1))
            .collect()
    }

    /// Finds the widget index at a given column.
    pub fn widget_at_column(&self, col: usize) -> Option<usize> {
        self.cells.get(col).and_then(|cell| cell.owner)
    }

    /// Finds the widget index and sub-target at a given column.
    pub fn target_at_column(&self, col: usize) -> Option<(usize, Option<String>)> {
        self.cells
            .get(col)
            .and_then(|cell| cell.owner.map(|owner| (owner, cell.target.clone())))
    }

    /// Hit-tests a surface-local coordinate (in logical pixels) to a widget index and sub-target.
    pub fn hit_test(
        &self,
        surface_x: f64,
        origin: u32,
        cell_width: u32,
    ) -> Option<(usize, Option<String>)> {
        if cell_width == 0 || surface_x < origin as f64 {
            return None;
        }
        let col = ((surface_x - origin as f64) / cell_width as f64) as usize;
        self.target_at_column(col)
    }

    fn write_region(&mut self, spans: &[RichSpan], start: usize, clip: Range<usize>) {
        let mut column = start;
        for (placement, span) in spans.iter().enumerate() {
            let Part::Text(text) = &span.part else {
                let Part::Image(image) = &span.part else {
                    unreachable!()
                };
                let end = column.saturating_add(span.visible_width.unwrap_or(image.width));
                let image = Arc::new(image.clone());
                for i in column.max(clip.start)..end.min(clip.end).min(self.cells.len()) {
                    let cell = &mut self.cells[i];
                    cell.symbol.clear();
                    cell.image = Some(ImageSlice {
                        image: image.clone(),
                        index: i - column,
                        placement,
                    });
                    cell.style = span.style;
                    cell.capsule = span.capsule;
                    cell.owner = span.owner;
                    cell.target = span.target.clone();
                    cell.continuation = i != column;
                }
                column = end;
                if column >= self.cells.len() {
                    return;
                }
                continue;
            };
            let mut previous: Option<usize> = None;
            for grapheme in text.graphemes(true) {
                let grapheme = printable_grapheme(grapheme);
                let width = grapheme_cell_width(grapheme);
                if width == 0 {
                    if let Some(index) = previous {
                        self.cells[index].symbol.push_str(grapheme);
                    }
                    continue;
                }
                let end = column.saturating_add(width);
                if column >= clip.start && end <= clip.end && end <= self.cells.len() {
                    let cell = &mut self.cells[column];
                    cell.symbol.clear();
                    cell.image = None;
                    cell.symbol.push_str(grapheme);
                    cell.style = span.style;
                    cell.capsule = span.capsule;
                    cell.owner = span.owner;
                    cell.target = span.target.clone();
                    cell.continuation = false;
                    for cell in &mut self.cells[column + 1..end] {
                        cell.symbol.clear();
                        cell.image = None;
                        cell.style = span.style;
                        cell.capsule = span.capsule;
                        cell.owner = span.owner;
                        cell.target = span.target.clone();
                        cell.continuation = true;
                    }
                    previous = Some(column);
                } else {
                    previous = None;
                }
                column = end;
                if column >= self.cells.len() {
                    return;
                }
            }
        }
    }
}

#[allow(dead_code)]
pub fn layout_regions(frame: &mut CellFrame, regions: RegionText<'_>) {
    let convert = |spans: &[StyledText]| -> Vec<RichSpan> {
        spans
            .iter()
            .map(|span| RichSpan {
                part: Part::Text(span.text.clone()),
                visible_width: None,
                style: span.style,
                capsule: None,
                owner: span.owner,
                target: None,
            })
            .collect()
    };
    let left = convert(regions.left);
    let center = convert(regions.center);
    let right = convert(regions.right);
    layout_rich_regions(
        frame,
        RichRegions {
            left: &left,
            center: &center,
            right: &right,
        },
    );
}

pub fn layout_rich_regions(frame: &mut CellFrame, regions: RichRegions<'_>) {
    let columns = frame.cells.len();
    let gap = 1;
    let left_width = region_width(regions.left, columns);
    let center_width = region_width(regions.center, columns);
    let right_width = region_width(regions.right, columns);
    let right_start = columns.saturating_sub(right_width);
    let left_limit = if right_width == 0 {
        columns
    } else {
        right_start.saturating_sub(gap)
    };
    let left_end = left_width.min(left_limit);
    let center_start = columns.saturating_sub(center_width) / 2;
    let center_min = if left_width == 0 {
        0
    } else {
        left_end.saturating_add(gap)
    };
    let center_max = if right_width == 0 {
        columns
    } else {
        right_start.saturating_sub(gap)
    };

    frame.write_region(regions.left, 0, 0..left_end);
    frame.write_region(regions.center, center_start, center_min..center_max);
    frame.write_region(regions.right, right_start, right_start..columns);
}

pub fn grapheme_cell_width(grapheme: &str) -> usize {
    grapheme.width()
}

pub fn text_cell_width(text: &str) -> usize {
    text.graphemes(true)
        .map(|grapheme| grapheme_cell_width(printable_grapheme(grapheme)))
        .sum()
}

fn printable_grapheme(grapheme: &str) -> &str {
    if grapheme.chars().any(char::is_control) {
        " "
    } else {
        grapheme
    }
}

fn region_width(spans: &[RichSpan], limit: usize) -> usize {
    spans.iter().fold(0usize, |width, span| {
        width
            .saturating_add(match &span.part {
                Part::Text(text) => text_cell_width(text),
                Part::Image(image) => span.visible_width.unwrap_or(image.width),
            })
            .min(limit)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DisplayStyle, Rgba};

    fn style() -> DisplayStyle {
        DisplayStyle {
            foreground: Rgba {
                red: 255,
                green: 255,
                blue: 255,
                alpha: 255,
            },
            background: None,
            bold: false,
            italic: false,
            underline: false,
            strikethrough: false,
            inset_y: 0,
            radius: None,
        }
    }

    fn span(text: &str, owner: Option<usize>) -> StyledText {
        StyledText {
            text: text.to_owned(),
            style: style(),
            owner,
        }
    }

    fn frame(columns: usize, regions: RegionText<'_>) -> CellFrame {
        let mut frame = CellFrame::new(columns, style());
        layout_regions(&mut frame, regions);
        frame
    }

    #[test]
    fn image_spans_clip_by_column_without_changing_their_source_geometry() {
        let spec = ImageSpec {
            src: "icon.png".into(),
            width: 4,
            fit: Default::default(),
            shape: Default::default(),
            align: Default::default(),
            fallback: "*".into(),
        };
        let left = [
            RichSpan {
                part: Part::Text("A".into()),
                visible_width: None,
                style: style(),
                capsule: None,
                owner: Some(0),
                target: None,
            },
            RichSpan {
                part: Part::Image(spec.clone()),
                visible_width: None,
                style: style(),
                capsule: None,
                owner: Some(1),
                target: None,
            },
        ];
        let right = [RichSpan {
            part: Part::Text("R".into()),
            visible_width: None,
            style: style(),
            capsule: None,
            owner: Some(2),
            target: None,
        }];
        let mut frame = CellFrame::new(5, style());
        layout_rich_regions(
            &mut frame,
            RichRegions {
                left: &left,
                center: &[],
                right: &right,
            },
        );
        assert_eq!(frame.cells[0].symbol, "A");
        assert_eq!(frame.cells[4].symbol, "R");
        assert_eq!(frame.cells[1].image.as_ref().unwrap().index, 0);
        assert_eq!(frame.cells[2].image.as_ref().unwrap().index, 1);
        assert!(frame.cells[3].image.is_none());
        assert_eq!(frame.cells[2].owner, Some(1));
        let first = frame.clone();
        frame.reset(5, style());
        assert!(frame.cells.iter().all(|cell| cell.image.is_none()));
        layout_rich_regions(
            &mut frame,
            RichRegions {
                left: &left,
                center: &[],
                right: &right,
            },
        );
        assert_eq!(first.diff_columns(&frame), None);
    }

    #[test]
    fn separates_regions_and_preserves_owners() {
        let left = [
            span("A", Some(0)),
            span("  ", None),
            span("\u{754c}", Some(1)),
        ];
        let center = [span("C", Some(2))];
        let right = [span("R", Some(3))];
        let frame = frame(
            15,
            RegionText {
                left: &left,
                center: &center,
                right: &right,
            },
        );
        assert_eq!(frame.cells[0].owner, Some(0));
        assert_eq!(frame.cells[1].owner, None);
        assert_eq!(frame.cells[3].symbol, "\u{754c}");
        assert!(frame.cells[4].continuation);
        assert_eq!(frame.cells[4].owner, Some(1));
        assert_eq!(frame.cells[7].symbol, "C");
        assert_eq!(frame.cells[14].symbol, "R");
    }

    #[test]
    fn never_splits_wide_graphemes_at_clip_boundaries() {
        let left = [span("a\u{754c}", Some(0))];
        let right = [span("R", Some(1))];
        let frame = frame(
            4,
            RegionText {
                left: &left,
                center: &[],
                right: &right,
            },
        );
        assert_eq!(frame.cells[0].symbol, "a");
        assert!(frame.cells[1].symbol.is_empty());
        assert_eq!(frame.cells[3].symbol, "R");
    }

    #[test]
    fn combining_marks_stay_in_one_cell() {
        let left = [span("e\u{301}x", Some(0))];
        let frame = frame(
            3,
            RegionText {
                left: &left,
                center: &[],
                right: &[],
            },
        );
        assert_eq!(frame.cells[0].symbol, "e\u{301}");
        assert_eq!(frame.cells[1].symbol, "x");
    }

    #[test]
    fn competing_regions_clip_the_center_without_overwriting_right() {
        let left = [span("abcd", Some(0))];
        let center = [span("CENTER", Some(1))];
        let right = [span("xyz", Some(2))];
        let frame = frame(
            10,
            RegionText {
                left: &left,
                center: &center,
                right: &right,
            },
        );
        let symbols: String = frame
            .cells
            .iter()
            .map(|cell| cell.symbol.chars().next().unwrap_or(' '))
            .collect();
        assert_eq!(symbols, "abcd T xyz");
        assert_eq!(frame.cells[5].owner, Some(1));
        assert_eq!(frame.cells[7].owner, Some(2));
    }

    #[test]
    fn grapheme_width_follows_standard_unicode_width() {
        assert_eq!(grapheme_cell_width("\u{F000}"), 1);
        assert_eq!(grapheme_cell_width("\u{E0B4}"), 1);
        assert_eq!(grapheme_cell_width("\u{E0B6}"), 1);
        assert_eq!(grapheme_cell_width("A"), 1);
        assert_eq!(grapheme_cell_width("界"), 2);
        let left = [span("界好", Some(0))];
        let frame = frame(
            5,
            RegionText {
                left: &left,
                center: &[],
                right: &[],
            },
        );
        assert!(frame.cells[1].continuation);
        assert!(frame.cells[3].continuation);
        assert_eq!(frame.cells[4].owner, None);
    }

    #[test]
    fn damage_range_covers_changed_wide_glyph() {
        let old = frame(
            4,
            RegionText {
                left: &[span("界", Some(0))],
                center: &[],
                right: &[],
            },
        );
        let new = frame(
            4,
            RegionText {
                left: &[span("好", Some(0))],
                center: &[],
                right: &[],
            },
        );
        assert_eq!(old.diff_columns(&new), Some(0..2));
    }

    #[test]
    fn adjacent_items_do_not_receive_implicit_spacing() {
        let left = [span("A", Some(0)), span("B", Some(1))];
        let frame = frame(
            4,
            RegionText {
                left: &left,
                center: &[],
                right: &[],
            },
        );
        assert_eq!(frame.cells[0].symbol, "A");
        assert_eq!(frame.cells[1].symbol, "B");
        assert_eq!(frame.cells[2].owner, None);
    }

    #[test]
    fn left_and_right_keep_one_cell_between_them() {
        let left = [span("abc", Some(0))];
        let right = [span("xy", Some(1))];
        let frame = frame(
            5,
            RegionText {
                left: &left,
                center: &[],
                right: &right,
            },
        );
        let symbols: String = frame
            .cells
            .iter()
            .map(|cell| cell.symbol.chars().next().unwrap_or(' '))
            .collect();
        assert_eq!(symbols, "ab xy");
    }

    #[test]
    fn keeps_frame_bounded_for_long_output_and_zero_width() {
        let left = [span(&"a".repeat(100_000), Some(0))];
        let frame = frame(
            3,
            RegionText {
                left: &left,
                center: &[],
                right: &[],
            },
        );
        assert_eq!(frame.cells.len(), 3);
        assert_eq!(
            frame
                .cells
                .iter()
                .map(|cell| cell.symbol.as_str())
                .collect::<String>(),
            "aaa"
        );
        let empty = self::tests::frame(
            0,
            RegionText {
                left: &left,
                center: &[],
                right: &[],
            },
        );
        assert!(empty.cells.is_empty());
    }

    #[test]
    fn control_characters_do_not_create_extra_rows() {
        let left = [span("A\nB\tC", Some(0))];
        let frame = frame(
            5,
            RegionText {
                left: &left,
                center: &[],
                right: &[],
            },
        );
        let symbols: String = frame
            .cells
            .iter()
            .map(|cell| cell.symbol.as_str())
            .collect();
        assert_eq!(symbols, "A B C");
    }

    #[test]
    fn cell_frame_reset_and_diff() {
        let mut frame_a = CellFrame::new(5, style());
        let mut frame_b = CellFrame::new(5, style());
        assert_eq!(frame_a.diff_columns(&frame_b), None);

        let left = [span("Hi", Some(0))];
        layout_regions(
            &mut frame_b,
            RegionText {
                left: &left,
                center: &[],
                right: &[],
            },
        );
        assert_eq!(frame_a.diff_columns(&frame_b), Some(0..2));

        frame_a.reset(5, style());
        assert_eq!(frame_a.cells.len(), 5);
        assert!(frame_a.cells.iter().all(|c| c.symbol.is_empty()));
    }

    #[test]
    fn diff_ranges_keep_far_changes_separate() {
        let old = CellFrame::new(8, style());
        let mut new = old.clone();
        new.cells[1].symbol = "A".into();
        new.cells[6].symbol = "B".into();
        assert_eq!(old.diff_ranges(&new), Some(vec![1..2, 6..7]));
        assert_eq!(old.diff_columns(&new), Some(1..7));
    }

    #[test]
    fn owner_ranges_tracks_column_bounds() {
        let left = [span("AB", Some(0)), span(" ", None), span("XYZ", Some(1))];
        let right = [span("99", Some(2))];
        let frame = frame(
            15,
            RegionText {
                left: &left,
                center: &[],
                right: &right,
            },
        );
        let ranges = frame.owner_ranges(4);
        assert_eq!(ranges[0], Some(0..2));
        assert_eq!(ranges[1], Some(3..6));
        assert_eq!(ranges[2], Some(13..15));
        assert_eq!(ranges[3], None);

        // Hit-test on this frame:
        // origin = 10, cell_width = 8
        // col 0..2 is owner 0 (pixels [10, 26))
        assert_eq!(frame.hit_test(5.0, 10, 8), None); // padding
        assert_eq!(frame.hit_test(10.0, 10, 8), Some((0, None))); // col 0
        assert_eq!(frame.hit_test(25.9, 10, 8), Some((0, None))); // col 1
        assert_eq!(frame.hit_test(26.0, 10, 8), None); // col 2 is space (None)
        assert_eq!(frame.hit_test(34.0, 10, 8), Some((1, None))); // col 3
    }
}
