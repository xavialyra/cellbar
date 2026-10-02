use super::{PaintTarget, TextRenderer, draw::*, font::*, frame_runs, image::*};
use crate::{
    cell_frame::{CapsuleShell, CellFrame, ImageSlice, RegionText, StyledText, layout_regions},
    config::{DisplayStyle, FontFamilyEntry, Rgba},
    images::{ImageAlign, ImageFit, ImageShape, ImageStore},
};

fn test_style() -> DisplayStyle {
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

fn paint_regions(renderer: &mut TextRenderer, target: PaintTarget<'_>, regions: RegionText<'_>) {
    let (columns, origin) = renderer.cell_geometry(target.width / target.scale, 8);
    let mut frame = CellFrame::new(columns, crate::config::Theme::default().default_style);
    layout_regions(&mut frame, regions);
    renderer.paint_frame(target, &frame, origin);
}

#[test]
fn premultiplies_translucent_colors() {
    assert_eq!(
        premultiplied_argb(Rgba {
            red: 0xff,
            green: 0x80,
            blue: 0,
            alpha: 0x80,
        }),
        0x80804000
    );
}

#[test]
fn image_resource_renders_inside_reserved_cells_after_worker_completes() {
    use crate::{
        cell_frame::{RichRegions, RichSpan, layout_rich_regions},
        images::{ImageSpec, Part},
    };
    use smithay_client_toolkit::reexports::calloop::{
        EventLoop,
        channel::{self, Event},
    };
    use std::time::Duration;
    let path = std::env::temp_dir().join(format!("cellbar-render-{}.png", std::process::id()));
    image::RgbaImage::from_pixel(4, 4, image::Rgba([255, 0, 0, 255]))
        .save(&path)
        .unwrap();
    let (sender, receiver) = channel::sync_channel(8);
    let mut loop_: EventLoop<Vec<crate::images::store::ResultMessage>> =
        EventLoop::try_new().unwrap();
    loop_
        .handle()
        .insert_source(receiver, |event, _, events| {
            if let Event::Msg(event) = event {
                events.push(event);
            }
        })
        .unwrap();
    let mut events = Vec::new();
    let mut renderer =
        TextRenderer::from_family_names(&["monospace"], 12.0, FontStyleRequirements::default());
    renderer.images = Some(ImageStore::new(sender));
    let mut frame = CellFrame::new(8, test_style());
    let left = [
        RichSpan {
            part: Part::Text("A".into()),
            visible_width: None,
            style: test_style(),
            capsule: None,
            owner: Some(0),
            target: None,
        },
        RichSpan {
            part: Part::Image(ImageSpec {
                src: path.clone(),
                width: 2,
                fit: ImageFit::Stretch,
                shape: ImageShape::Rect,
                align: ImageAlign::Center,
                fallback: "?".into(),
            }),
            visible_width: None,
            style: test_style(),
            capsule: None,
            owner: Some(1),
            target: None,
        },
    ];
    layout_rich_regions(
        &mut frame,
        RichRegions {
            left: &left,
            center: &[],
            right: &[],
        },
    );
    let width = 8 * renderer.cell_width();
    let height = 24;
    let mut canvas = vec![0; (width * height * 4) as usize];
    for _ in 0..3 {
        renderer.paint_frame(
            PaintTarget {
                canvas: &mut canvas,
                width,
                height,
                scale: 1,
                background: Rgba {
                    red: 0,
                    green: 0,
                    blue: 0,
                    alpha: 255,
                },
                radius: 0,
            },
            &frame,
            0,
        );
        loop_.dispatch(Duration::from_secs(2), &mut events).unwrap();
        for result in events.drain(..) {
            renderer.images.as_mut().unwrap().complete(result);
        }
    }
    let px = |x: u32, y: u32| {
        u32::from_ne_bytes(
            canvas[pixel_index(width, x, y).unwrap()..][..4]
                .try_into()
                .unwrap(),
        )
    };
    assert_eq!(px(renderer.cell_width() + 1, height / 2), 0xffff0000);
    assert_eq!(px(3 * renderer.cell_width() + 1, height / 2), 0xff000000);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn circle_fit_uses_square_viewport_and_preserves_full_mask_when_clipped() {
    use crate::images::ImageSpec;
    use std::sync::Arc;

    let spec = ImageSpec {
        src: "cover.png".into(),
        width: 4,
        fit: ImageFit::Cover,
        shape: ImageShape::Circle,
        align: ImageAlign::Center,
        fallback: "*".into(),
    };
    let run = ImageRun {
        image: ImageSlice {
            image: Arc::new(spec),
            index: 2,
            placement: 0,
        },
        style: test_style(),
        start: 2,
        end: 4,
    };
    let layout = ImageLayout {
        cell_width: 8,
        origin: 0,
        offset_y: 0,
        line_height: 16.0,
        cap_height: 10.0,
        baseline: 12.0,
    };
    let geometry = image_geometry(&run, (16, 16), layout).unwrap();
    assert_eq!(
        (
            geometry.x,
            geometry.y,
            geometry.width,
            geometry.height,
            geometry.clip_top,
            geometry.clip_bottom
        ),
        (8, 0, 16, 16, 0, 16)
    );
    let mask = geometry.circle.unwrap();
    assert_eq!(
        (mask.center_x, mask.center_y, mask.radius),
        (16.0, 8.0, 8.0)
    );

    let pixels = image::RgbaImage::from_pixel(16, 16, image::Rgba([255, 0, 0, 255]));
    let mut canvas = vec![0; 32 * 16 * 4];
    fill(
        &mut canvas,
        Rgba {
            red: 0,
            green: 0,
            blue: 0,
            alpha: 255,
        },
    );
    blit_image(
        &mut canvas,
        (32, 16),
        0,
        ImagePaint {
            visible: 16..32,
            x: geometry.x,
            y: geometry.y,
            clip_top: geometry.clip_top,
            clip_bottom: geometry.clip_bottom,
            circle: Some(mask),
        },
        &pixels,
    );
    let color = |x, y| {
        u32::from_ne_bytes(
            canvas[pixel_index(32, x, y).unwrap()..][..4]
                .try_into()
                .unwrap(),
        )
    };
    assert_eq!(color(16, 8), 0xffff0000);
    assert_eq!(color(15, 8), 0xff000000);
    assert_eq!(color(21, 0), 0xff000000);
    assert_eq!(color(24, 8), 0xff000000);

    let mut textmatch = (*run.image.image).clone();
    textmatch.fit = ImageFit::TextMatch;
    textmatch.align = ImageAlign::Right;
    let textmatch_run = ImageRun {
        image: ImageSlice {
            image: Arc::new(textmatch),
            index: 2,
            placement: 0,
        },
        ..run
    };
    let geometry = image_geometry(
        &textmatch_run,
        (16, 8),
        ImageLayout {
            cell_width: 8,
            origin: 0,
            offset_y: 0,
            line_height: 16.0,
            cap_height: 10.0,
            baseline: 12.0,
        },
    )
    .unwrap();
    assert_eq!(
        (
            geometry.x,
            geometry.y,
            geometry.width,
            geometry.height,
            geometry.clip_top
        ),
        (22, 5, 10, 5, 2)
    );
    let mask = geometry.circle.unwrap();
    assert_eq!(
        (mask.center_x, mask.center_y, mask.radius),
        (27.0, 7.0, 5.0)
    );
    let mut fallback = CellFrame::new(4, test_style());
    place_fallback(&mut fallback, &textmatch_run);
    assert_eq!(fallback.cells[2].symbol, "*");
}

#[test]
fn circle_mask_antialiases_edge_and_preserves_center() {
    let circle = CircleMask {
        center_x: 8.0,
        center_y: 8.0,
        radius: 8.0,
    };
    assert_eq!(circle_coverage(circle, 8, 8), 255);
    assert_eq!(circle_coverage(circle, 0, 0), 0);
    let edge = circle_coverage(circle, 0, 6);
    assert!(edge > 0 && edge < 255);
}

#[test]
fn image_blit_respects_columns_alpha_and_rounded_corners() {
    let mut canvas = vec![0; 8 * 8 * 4];
    fill_rounded(
        &mut canvas,
        8,
        8,
        Rgba {
            red: 0,
            green: 0,
            blue: 0,
            alpha: 255,
        },
        2,
    );
    let pixels = image::RgbaImage::from_pixel(8, 8, image::Rgba([255, 0, 0, 128]));
    blit_image(
        &mut canvas,
        (8, 8),
        2,
        ImagePaint {
            visible: 2..6,
            x: 0,
            y: 0,
            clip_top: 0,
            clip_bottom: 8,
            circle: None,
        },
        &pixels,
    );
    let color = |x, y| {
        u32::from_ne_bytes(
            canvas[pixel_index(8, x, y).unwrap()..][..4]
                .try_into()
                .unwrap(),
        )
    };
    assert_eq!(color(0, 4), 0xff000000);
    assert_eq!(color(3, 4), 0xff800000);
    assert_eq!(color(7, 4), 0xff000000);
    assert_eq!(color(0, 0), 0);
}

#[test]
fn rounded_background_clears_corners() {
    let color = Rgba {
        red: 0x20,
        green: 0x40,
        blue: 0x60,
        alpha: 0xff,
    };
    let mut canvas = vec![0; 20 * 20 * 4];
    fill_rounded(&mut canvas, 20, 20, color, 6);

    let corner = pixel_index(20, 0, 0).expect("corner pixel");
    let center = pixel_index(20, 10, 10).expect("center pixel");
    assert_eq!(&canvas[corner..corner + 4], &[0, 0, 0, 0]);
    assert_eq!(
        &canvas[center..center + 4],
        &premultiplied_argb(color).to_ne_bytes()
    );
}

#[test]
fn cell_grid_is_centered_within_padding() {
    let renderer =
        TextRenderer::from_family_names(&["monospace"], 12.0, FontStyleRequirements::default());
    let (columns, origin) = renderer.cell_geometry(1000, 8);
    assert!(origin >= 8);
    assert!((1000 - origin - (columns as u32) * renderer.cell_width()).abs_diff(origin) <= 1);
}

#[test]
fn text_rasterization_changes_background_pixels() {
    let background = Rgba {
        red: 0x18,
        green: 0x1a,
        blue: 0x1f,
        alpha: 0xff,
    };
    let mut canvas = vec![0; 400 * 30 * 4];
    let style = DisplayStyle {
        foreground: Rgba {
            red: 0xd8,
            green: 0xde,
            blue: 0xe9,
            alpha: 0xff,
        },
        background: None,
        bold: false,
        italic: false,
        underline: false,
        strikethrough: false,
        inset_y: 0,
        radius: None,
    };
    let left = [StyledText {
        text: "termway".to_owned(),
        style,
        owner: None,
    }];
    let center = [StyledText {
        text: "12:34".to_owned(),
        style,
        owner: None,
    }];
    let mut renderer =
        TextRenderer::from_family_names(&["monospace"], 12.0, FontStyleRequirements::default());
    paint_regions(
        &mut renderer,
        PaintTarget {
            canvas: &mut canvas,
            width: 400,
            height: 30,
            scale: 1,
            background,
            radius: 0,
        },
        RegionText {
            left: &left,
            center: &center,
            right: &[],
        },
    );

    let background_pixel = premultiplied_argb(background).to_ne_bytes();
    assert!(
        canvas
            .as_chunks::<4>()
            .0
            .iter()
            .any(|pixel| pixel != &background_pixel)
    );
}

#[test]
fn adjusts_cell_width_and_grid_capacity_without_changing_line_height() {
    let entries = [FontFamilyEntry::Family("monospace".into())];
    let base = TextRenderer::new(
        &entries,
        12.0,
        0,
        crate::config::DEFAULT_LINE_HEIGHT,
        FontStyleRequirements::default(),
    );
    let wide = TextRenderer::new(
        &entries,
        12.0,
        2,
        crate::config::DEFAULT_LINE_HEIGHT,
        FontStyleRequirements::default(),
    );
    let narrow = TextRenderer::new(
        &entries,
        12.0,
        -2,
        crate::config::DEFAULT_LINE_HEIGHT,
        FontStyleRequirements::default(),
    );
    assert_eq!(wide.cell_width(), base.cell_width() + 2);
    assert_eq!(
        narrow.cell_width(),
        base.cell_width().saturating_sub(2).max(1)
    );
    assert!(wide.cell_geometry(100, 8).0 <= base.cell_geometry(100, 8).0);
    assert!(base.cell_geometry(100, 8).0 * base.cell_width() as usize + 16 <= 100);
    let clamped = TextRenderer::new(
        &entries,
        12.0,
        -256,
        crate::config::DEFAULT_LINE_HEIGHT,
        FontStyleRequirements::default(),
    );
    assert_eq!(clamped.cell_width(), 1);
}

#[test]
fn grid_rasterization_stays_within_its_cell_at_output_scale() {
    let mut renderer =
        TextRenderer::from_family_names(&["monospace"], 12.0, FontStyleRequirements::default());
    let style = crate::config::Theme::default().default_style;
    let left = [StyledText {
        text: "W".into(),
        style,
        owner: Some(0),
    }];
    let (columns, origin) = renderer.cell_geometry(100, 8);
    let mut frame = CellFrame::new(columns, style);
    layout_regions(
        &mut frame,
        RegionText {
            left: &left,
            center: &[],
            right: &[],
        },
    );
    let background = Rgba {
        red: 0,
        green: 0,
        blue: 0,
        alpha: 255,
    };
    let mut canvas = vec![0; 200 * 60 * 4];
    renderer.paint_frame(
        PaintTarget {
            canvas: &mut canvas,
            width: 200,
            height: 60,
            scale: 2,
            background,
            radius: 0,
        },
        &frame,
        origin,
    );
    let background_pixel = premultiplied_argb(background).to_ne_bytes();
    let active = origin * 2..(origin + renderer.cell_width()) * 2;
    let changed: Vec<_> = canvas
        .as_chunks::<4>()
        .0
        .iter()
        .enumerate()
        .filter(|(_, pixel)| *pixel != &background_pixel)
        .map(|(pixel, _)| pixel % 200_usize)
        .collect();
    assert!(!changed.is_empty());
    assert!(changed.into_iter().all(|x| active.contains(&(x as u32))));
}

#[test]
fn fullwidth_glyph_uses_both_cells_without_overlapping_next_item() {
    if find_fonts("Noto Sans CJK JP", "regular", "roman").is_empty() {
        eprintln!("skipping CJK raster test: font unavailable");
        return;
    }
    let mut renderer = TextRenderer::new(
        &[
            FontFamilyEntry::Family("Noto Sans Mono".into()),
            FontFamilyEntry::Family("Noto Sans CJK JP".into()),
        ],
        14.0,
        2,
        crate::config::DEFAULT_LINE_HEIGHT,
        FontStyleRequirements::default(),
    );
    let foreground = Rgba {
        red: 255,
        green: 255,
        blue: 255,
        alpha: 255,
    };
    let background = Rgba {
        red: 0,
        green: 0,
        blue: 0,
        alpha: 255,
    };
    let style = DisplayStyle {
        foreground,
        background: None,
        bold: false,
        italic: false,
        underline: false,
        strikethrough: false,
        inset_y: 0,
        radius: None,
    };
    let left = [
        StyledText {
            text: "界".into(),
            style,
            owner: Some(0),
        },
        StyledText {
            text: "A".into(),
            style,
            owner: Some(1),
        },
    ];
    let mut frame = CellFrame::new(8, style);
    layout_regions(
        &mut frame,
        RegionText {
            left: &left,
            center: &[],
            right: &[],
        },
    );
    assert_eq!(frame.cells[0].symbol, "界");
    assert!(frame.cells[1].continuation);
    assert_eq!(frame.cells[2].symbol, "A");
    let cell_width = renderer.cell_width();
    let mut canvas = vec![0; (cell_width * 8 * 30 * 4) as usize];
    renderer.paint_frame(
        PaintTarget {
            canvas: &mut canvas,
            width: cell_width * 8,
            height: 30,
            scale: 1,
            background,
            radius: 0,
        },
        &frame,
        0,
    );
    let black = premultiplied_argb(background).to_ne_bytes();
    let mut counts = [0; 8];
    for (index, pixel) in canvas.as_chunks::<4>().0.iter().enumerate() {
        if pixel != &black {
            counts[(index % (cell_width as usize * 8)) / cell_width as usize] += 1;
        }
    }
    assert!(
        counts[0] > 0 && counts[1] > 0,
        "fullwidth glyph: {counts:?}"
    );
    assert!(counts[2] > 0, "following ASCII glyph: {counts:?}");
    assert_eq!(&counts[3..], &[0; 5]);
}

#[test]
fn mixed_width_text_keeps_cell_positions_with_fallback_fonts() {
    if find_fonts("Noto Sans CJK JP", "regular", "roman").is_empty() {
        eprintln!("skipping CJK raster test: font unavailable");
        return;
    }
    let mut renderer = TextRenderer::from_family_names(
        &["Noto Sans Mono", "Noto Sans CJK JP"],
        14.0,
        FontStyleRequirements::default(),
    );
    let style = crate::config::Theme::default().default_style;
    let right = [StyledText {
        text: "A界B".into(),
        style,
        owner: Some(0),
    }];
    let mut frame = CellFrame::new(8, style);
    layout_regions(
        &mut frame,
        RegionText {
            left: &[],
            center: &[],
            right: &right,
        },
    );
    assert_eq!(frame.cells[4].symbol, "A");
    assert_eq!(frame.cells[5].symbol, "界");
    assert!(frame.cells[6].continuation);
    assert_eq!(frame.cells[7].symbol, "B");
    let cell_width = renderer.cell_width() as usize;
    let width = cell_width * 8;
    let background = Rgba {
        red: 0,
        green: 0,
        blue: 0,
        alpha: 255,
    };
    let mut canvas = vec![0; width * 30 * 4];
    renderer.paint_frame(
        PaintTarget {
            canvas: &mut canvas,
            width: width as u32,
            height: 30,
            scale: 1,
            background,
            radius: 0,
        },
        &frame,
        0,
    );
    let black = premultiplied_argb(background).to_ne_bytes();
    let mut counts = [0; 8];
    for (index, pixel) in canvas.as_chunks::<4>().0.iter().enumerate() {
        if pixel != &black {
            counts[(index % width) / cell_width] += 1;
        }
    }
    assert_eq!(&counts[..4], &[0; 4]);
    assert!(counts[4..].iter().all(|&count| count > 0), "{counts:?}");
}

#[test]
fn expanded_cells_center_glyphs_at_double_scale() {
    let entries = [FontFamilyEntry::Family("monospace".into())];
    let style = crate::config::Theme::default().default_style;
    let left = [StyledText {
        text: "0".into(),
        style,
        owner: Some(0),
    }];
    let first_pixel = |adjust| {
        let mut renderer = TextRenderer::new(
            &entries,
            12.0,
            adjust,
            crate::config::DEFAULT_LINE_HEIGHT,
            FontStyleRequirements::default(),
        );
        let (columns, origin) = renderer.cell_geometry(100, 8);
        let mut frame = CellFrame::new(columns, style);
        layout_regions(
            &mut frame,
            RegionText {
                left: &left,
                center: &[],
                right: &[],
            },
        );
        let mut canvas = vec![0; 200 * 60 * 4];
        let background = Rgba {
            red: 0,
            green: 0,
            blue: 0,
            alpha: 255,
        };
        renderer.paint_frame(
            PaintTarget {
                canvas: &mut canvas,
                width: 200,
                height: 60,
                scale: 2,
                background,
                radius: 0,
            },
            &frame,
            origin,
        );
        let background_pixel = premultiplied_argb(background).to_ne_bytes();
        canvas
            .as_chunks::<4>()
            .0
            .iter()
            .enumerate()
            .filter(|(_, pixel)| *pixel != &background_pixel)
            .map(|(index, _)| index % 200 - (origin * 2) as usize)
            .min()
            .expect("glyph pixels")
    };
    assert_eq!(first_pixel(4), first_pixel(0) + 4);
}

#[test]
fn adjacent_displays_are_shaped_separately() {
    let style = crate::config::Theme::default().default_style;
    let left = [
        StyledText {
            text: "f".into(),
            style,
            owner: Some(0),
        },
        StyledText {
            text: "i".into(),
            style,
            owner: Some(1),
        },
    ];
    let mut frame = CellFrame::new(2, style);
    layout_regions(
        &mut frame,
        RegionText {
            left: &left,
            center: &[],
            right: &[],
        },
    );
    let runs = frame_runs(&frame);
    assert_eq!(runs.len(), 2);
    assert_eq!((runs[0].start, runs[0].end), (0, 1));
    assert_eq!((runs[1].start, runs[1].end), (1, 2));
}

#[test]
fn box_drawing_separator_fills_the_cell_exactly() {
    if find_fonts("JetBrainsMono Nerd Font", "regular", "roman").is_empty() {
        eprintln!("skipping separator test: font unavailable");
        return;
    }
    // U+2502 is drawn 1.52em tall by the font so box frames still join
    // across rows. Terminals such as foot and kitty generate the character
    // themselves and fill exactly one cell, which is why a separator there
    // matches a cell background. Clipping ink to the cell reproduces that.
    let mut renderer = TextRenderer::from_family_names(
        &["JetBrainsMono Nerd Font"],
        13.0,
        FontStyleRequirements::default(),
    );
    let style = test_style();
    let width = renderer.cell_width * 8;
    let cell_height = crate::config::cell_height(13.0, crate::config::DEFAULT_LINE_HEIGHT);
    let height = cell_height + 12;
    let background = Rgba {
        red: 0,
        green: 0,
        blue: 0,
        alpha: 255,
    };
    let left = [StyledText {
        text: "\u{2502}".to_owned(),
        style,
        owner: Some(0),
    }];
    let mut canvas = vec![0; (width * height * 4) as usize];
    paint_regions(
        &mut renderer,
        PaintTarget {
            canvas: &mut canvas,
            width,
            height,
            scale: 1,
            background,
            radius: 0,
        },
        RegionText {
            left: &left,
            center: &[],
            right: &[],
        },
    );
    let background_pixel = premultiplied_argb(background).to_ne_bytes();
    let ink_rows: Vec<u32> = (0..height)
        .filter(|&y| {
            (0..width).any(|x| {
                let index = ((y * width + x) * 4) as usize;
                canvas[index..index + 4] != background_pixel
            })
        })
        .collect();
    let top = (height - cell_height) / 2;
    assert_eq!(
        ink_rows,
        (top..top + cell_height).collect::<Vec<_>>(),
        "separator must span exactly one cell"
    );
}

#[test]
fn custom_line_height_resizes_the_cell_box() {
    // The configured line height must reach the renderer, not just
    // `Theme::bar_height`: the styled background box, the glyph clip and
    // the image viewport all derive from the same cell.
    let background_color = Rgba {
        red: 0x89,
        green: 0xb4,
        blue: 0xfa,
        alpha: 255,
    };
    let style = DisplayStyle {
        foreground: Rgba {
            red: 0,
            green: 0,
            blue: 0,
            alpha: 255,
        },
        background: Some(background_color),
        bold: false,
        italic: false,
        underline: false,
        strikethrough: false,
        inset_y: 0,
        radius: None,
    };
    let bar = Rgba {
        red: 0,
        green: 0,
        blue: 0,
        alpha: 255,
    };
    for (line_height, expected) in [(1.25_f32, 17_u32), (1.5, 20)] {
        let mut renderer = TextRenderer::new(
            &[FontFamilyEntry::Family("monospace".into())],
            13.0,
            0,
            line_height,
            FontStyleRequirements::default(),
        );
        let cell_height = crate::config::cell_height(13.0, line_height);
        assert_eq!(cell_height, expected, "line_height {line_height}");
        let width = renderer.cell_width * 4;
        let height = cell_height + 12;
        let left = [StyledText {
            text: "1".to_owned(),
            style,
            owner: Some(0),
        }];
        let mut canvas = vec![0; (width * height * 4) as usize];
        paint_regions(
            &mut renderer,
            PaintTarget {
                canvas: &mut canvas,
                width,
                height,
                scale: 1,
                background: bar,
                radius: 0,
            },
            RegionText {
                left: &left,
                center: &[],
                right: &[],
            },
        );
        let fill = premultiplied_argb(background_color).to_ne_bytes();
        let rows = (0..height)
            .filter(|&y| {
                (0..width).any(|x| {
                    let index = ((y * width + x) * 4) as usize;
                    canvas[index..index + 4] == fill
                })
            })
            .count() as u32;
        assert_eq!(rows, expected, "line_height {line_height} background rows");
    }
}

#[test]
fn icon_ink_overhangs_its_cells_like_a_terminal() {
    if find_fonts("JetBrainsMono Nerd Font", "regular", "roman").is_empty() {
        eprintln!("skipping icon overhang test: font unavailable");
        return;
    }
    // U+F028 (volume) has a one-cell advance but draws its sound waves
    // across roughly two cells. A terminal paints that overhang; clipping
    // ink to the reserved cells slices the waves off.
    let mut renderer = TextRenderer::from_family_names(
        &["JetBrainsMono Nerd Font"],
        13.0,
        FontStyleRequirements::default(),
    );
    let style = test_style();
    let cell_width = renderer.cell_width;
    let width = cell_width * 8;
    let height = crate::config::cell_height(13.0, crate::config::DEFAULT_LINE_HEIGHT) + 12;
    let background = Rgba {
        red: 0,
        green: 0,
        blue: 0,
        alpha: 255,
    };
    let left = [StyledText {
        text: "\u{f028}".to_owned(),
        style,
        owner: Some(0),
    }];
    let mut canvas = vec![0; (width * height * 4) as usize];
    paint_regions(
        &mut renderer,
        PaintTarget {
            canvas: &mut canvas,
            width,
            height,
            scale: 1,
            background,
            radius: 0,
        },
        RegionText {
            left: &left,
            center: &[],
            right: &[],
        },
    );
    let background_pixel = premultiplied_argb(background).to_ne_bytes();
    let lit = |x: u32| {
        (0..height).any(|y| {
            let index = ((y * width + x) * 4) as usize;
            canvas[index..index + 4] != background_pixel
        })
    };
    let ink_end = (0..width).filter(|&x| lit(x)).max().unwrap();
    let (_, origin) = renderer.cell_geometry(width, 8);
    let reserved_end = origin + cell_width;
    assert!(
        ink_end > reserved_end,
        "icon ink stops at its cell edge ({ink_end} <= {reserved_end}); overhang was clipped"
    );
    assert!(ink_end < width, "icon ink ran off the surface");
}

#[test]
fn cjk_fallback_shares_the_row_baseline() {
    if find_fonts("JetBrainsMono Nerd Font", "regular", "roman").is_empty()
        || find_fonts("Noto Sans Mono CJK JP", "regular", "roman").is_empty()
    {
        eprintln!("skipping CJK baseline test: fonts unavailable");
        return;
    }
    let entries = [
        FontFamilyEntry::Family("JetBrainsMono Nerd Font".to_owned()),
        FontFamilyEntry::Family("Noto Sans Mono CJK JP".to_owned()),
    ];
    let cell_height = crate::config::cell_height(13.0, crate::config::DEFAULT_LINE_HEIGHT);
    let width = 200;
    let height = cell_height + 12;
    let bar = Rgba {
        red: 0,
        green: 0,
        blue: 0,
        alpha: 0xff,
    };
    // Ink rows of a glyph rendered alone. Two separate frames are needed
    // because a mixed Latin+CJK line shares one shaped baseline, which
    // hides a fallback font that would otherwise push CJK down.
    let ink_row_span = |text: &str| -> (u32, u32) {
        let mut renderer = TextRenderer::new(
            &entries,
            13.0,
            0,
            crate::config::DEFAULT_LINE_HEIGHT,
            FontStyleRequirements::default(),
        );
        let left = [StyledText {
            text: text.to_owned(),
            style: test_style(),
            owner: Some(0),
        }];
        let mut canvas = vec![0; (width * height * 4) as usize];
        paint_regions(
            &mut renderer,
            PaintTarget {
                canvas: &mut canvas,
                width,
                height,
                scale: 1,
                background: bar,
                radius: 0,
            },
            RegionText {
                left: &left,
                center: &[],
                right: &[],
            },
        );
        let bar_pixel = premultiplied_argb(bar).to_ne_bytes();
        let mut bounds: Option<(u32, u32)> = None;
        for y in 0..height {
            for x in 0..width {
                let index = pixel_index(width, x, y).unwrap();
                if canvas[index..index + 4] == bar_pixel[..] {
                    continue;
                }
                let rows = bounds.get_or_insert((y, y));
                rows.0 = rows.0.min(y);
                rows.1 = rows.1.max(y);
            }
        }
        bounds.expect("painted glyph ink")
    };
    let latin = ink_row_span("H");
    let cjk = ink_row_span("\u{4e2d}");
    // Doubled row centers avoid the half-pixel ambiguity of inclusive bounds.
    let latin_center = latin.0 + latin.1;
    let cjk_center = cjk.0 + cjk.1;
    assert!(
        cjk_center <= latin_center + 2,
        "CJK fallback is pushed below the Latin baseline: latin {latin:?} cjk {cjk:?}"
    );
    let cell_center = 2 * (height - cell_height) / 2 + cell_height;
    assert!(
        (cjk_center as i32 - cell_center as i32).abs() <= 2,
        "CJK glyph is not vertically centered in its cell: cell center {cell_center} cjk {cjk:?}"
    );
}

#[test]
fn styled_background_centers_its_glyph() {
    if find_fonts("JetBrainsMono Nerd Font", "bold", "roman").is_empty() {
        eprintln!("skipping background centering test: font unavailable");
        return;
    }
    let bar = Rgba {
        red: 0x18,
        green: 0x18,
        blue: 0x25,
        alpha: 0xff,
    };
    let accent = Rgba {
        red: 0x89,
        green: 0xb4,
        blue: 0xfa,
        alpha: 0xff,
    };
    let style = DisplayStyle {
        foreground: Rgba {
            red: 0x11,
            green: 0x11,
            blue: 0x1b,
            alpha: 0xff,
        },
        background: Some(accent),
        bold: true,
        ..test_style()
    };
    let mut renderer = TextRenderer::from_family_names(
        &["JetBrainsMono Nerd Font"],
        13.0,
        FontStyleRequirements {
            bold: true,
            italic: false,
        },
    );
    let cell_height = crate::config::cell_height(13.0, crate::config::DEFAULT_LINE_HEIGHT);
    let width = 200;
    // Match `Theme::bar_height`: one cell plus the surface vertical padding.
    let height = cell_height + 2 * 6;
    let mut canvas = vec![0; (width * height * 4) as usize];
    let left = [StyledText {
        text: " 2 ".to_owned(),
        style,
        owner: Some(0),
    }];
    paint_regions(
        &mut renderer,
        PaintTarget {
            canvas: &mut canvas,
            width,
            height,
            scale: 1,
            background: bar,
            radius: 0,
        },
        RegionText {
            left: &left,
            center: &[],
            right: &[],
        },
    );

    let bar_pixel = premultiplied_argb(bar).to_ne_bytes();
    let accent_pixel = premultiplied_argb(accent).to_ne_bytes();
    let mut block: Option<(u32, u32, u32, u32)> = None;
    let mut ink: Option<(u32, u32, u32, u32)> = None;
    for y in 0..height {
        for x in 0..width {
            let index = pixel_index(width, x, y).unwrap();
            let pixel = &canvas[index..index + 4];
            let slot = if pixel == accent_pixel {
                &mut block
            } else if pixel != bar_pixel {
                &mut ink
            } else {
                continue;
            };
            let bounds = slot.get_or_insert((x, y, x, y));
            bounds.0 = bounds.0.min(x);
            bounds.1 = bounds.1.min(y);
            bounds.2 = bounds.2.max(x);
            bounds.3 = bounds.3.max(y);
        }
    }
    let block = block.expect("painted background");
    let ink = ink.expect("painted glyph");
    // The painted background is exactly one cell, centered in the bar.
    let cell_top = (height - cell_height) / 2;
    assert_eq!(
        (block.1, block.3 + 1),
        (cell_top, cell_top + cell_height),
        "background must cover exactly one centered cell: {block:?}"
    );
    // Doubled centers avoid the half-pixel ambiguity of inclusive bounds.
    let block_center_x = (block.0 + block.2) as i32;
    let block_center_y = (block.1 + block.3) as i32;
    let ink_center_x = (ink.0 + ink.2) as i32;
    let ink_center_y = (ink.1 + ink.3) as i32;
    assert!(
        (block_center_x - ink_center_x).abs() <= 1,
        "glyph is off center horizontally: background {block:?} glyph {ink:?}"
    );
    assert!(
        (block_center_y - ink_center_y).abs() <= 1,
        "glyph is off center vertically: background {block:?} glyph {ink:?}"
    );
}

#[test]
fn styled_segment_paints_its_background() {
    let bar_background = Rgba {
        red: 0x10,
        green: 0x10,
        blue: 0x10,
        alpha: 0xff,
    };
    let style = DisplayStyle {
        foreground: Rgba {
            red: 0xff,
            green: 0xff,
            blue: 0xff,
            alpha: 0xff,
        },
        background: Some(Rgba {
            red: 0x20,
            green: 0x40,
            blue: 0x60,
            alpha: 0xff,
        }),
        bold: true,
        italic: false,
        underline: false,
        strikethrough: false,
        inset_y: 0,
        radius: None,
    };
    let left = [StyledText {
        text: "x".to_owned(),
        style,
        owner: None,
    }];
    let mut canvas = vec![0; 200 * 30 * 4];
    let mut renderer = TextRenderer::from_family_names(
        &["monospace"],
        12.0,
        FontStyleRequirements {
            bold: true,
            italic: false,
        },
    );
    paint_regions(
        &mut renderer,
        PaintTarget {
            canvas: &mut canvas,
            width: 200,
            height: 30,
            scale: 1,
            background: bar_background,
            radius: 0,
        },
        RegionText {
            left: &left,
            center: &[],
            right: &[],
        },
    );

    // Check y = 0 within vertical padding: should maintain bar_background without fill bleed
    let pad_index = pixel_index(200, 8, 0).expect("padding pixel");
    assert_eq!(
        &canvas[pad_index..pad_index + 4],
        &premultiplied_argb(bar_background).to_ne_bytes()
    );

    // Check y = 10 within line-height region: cell background should be painted
    let content_index = pixel_index(200, 8, 10).expect("content pixel");
    assert_eq!(
        &canvas[content_index..content_index + 4],
        &premultiplied_argb(style.background.expect("background")).to_ne_bytes()
    );
}

#[test]
fn fallback_bold_loads_only_when_shaping_uses_it() {
    let primary = "JetBrainsMono Nerd Font Propo";
    let fallback = "Noto Sans CJK SC";
    if find_fonts(primary, "regular", "roman").is_empty()
        || find_fonts(primary, "bold", "roman").is_empty()
        || find_fonts(fallback, "regular", "roman").is_empty()
        || find_fonts(fallback, "bold", "roman").is_empty()
    {
        eprintln!("skipping font integration test: JetBrainsMono/Noto CJK fixtures unavailable");
        return;
    }
    let bold_paths = find_fonts(fallback, "bold", "roman");
    let mut renderer = TextRenderer::from_family_names(
        &[primary, fallback],
        12.0,
        FontStyleRequirements {
            bold: true,
            italic: false,
        },
    );
    let style = DisplayStyle {
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
    };
    let left = [StyledText {
        text: "12:34 CPU".to_owned(),
        style: DisplayStyle {
            bold: true,
            ..style
        },
        owner: None,
    }];
    let mut center = [StyledText {
        text: "中文标题".to_owned(),
        style,
        owner: None,
    }];
    let right = [StyledText {
        text: "状态".to_owned(),
        style,
        owner: None,
    }];
    let mut canvas = vec![0; 600 * 30 * 4];
    renderer.prepare(&left, 30, 1);
    assert!(!renderer.lazy_fonts.attempted.contains(&(1, false, false)));
    let paint = |renderer: &mut TextRenderer, canvas: &mut [u8], center: &[StyledText]| {
        paint_regions(
            renderer,
            PaintTarget {
                canvas,
                width: 600,
                height: 30,
                scale: 1,
                background: Rgba {
                    red: 0,
                    green: 0,
                    blue: 0,
                    alpha: 255,
                },
                radius: 0,
            },
            RegionText {
                left: &left,
                center,
                right: &right,
            },
        );
    };
    assert!(!renderer.lazy_fonts.attempted.contains(&(1, false, false)));
    assert!(
        bold_paths
            .iter()
            .all(|path| !renderer.lazy_fonts.file_families.contains_key(path))
    );
    paint(&mut renderer, &mut canvas, &center);
    assert!(renderer.lazy_fonts.attempted.contains(&(1, false, false)));
    assert!(
        bold_paths
            .iter()
            .all(|path| !renderer.lazy_fonts.file_families.contains_key(path))
    );
    assert!(renderer.lazy_fonts.generation > 0);

    center[0].style.bold = true;
    paint(&mut renderer, &mut canvas, &center);
    assert!(
        bold_paths
            .iter()
            .any(|path| renderer.lazy_fonts.file_families.contains_key(path))
    );
    let prepared = renderer.prepared.as_ref().unwrap();
    let glyphs = &prepared.segments[1].glyphs;
    assert!(!glyphs.is_empty());
    assert!(glyphs.iter().all(|glyph| {
        glyph.glyph_id != 0
            && renderer
                .font_db
                .face(glyph.font_id)
                .is_some_and(|face| face.weight == fontdb::Weight::BOLD)
    }));
    let generation = renderer.lazy_fonts.generation;
    let face_count = renderer.font_db.faces().count();
    paint(&mut renderer, &mut canvas, &center);
    assert_eq!(renderer.lazy_fonts.generation, generation);
    assert_eq!(renderer.font_db.faces().count(), face_count);
}

#[test]
fn opaque_blend_replaces_destination() {
    let mut pixel = 0xff102030_u32.to_ne_bytes();
    blend_pixel(&mut pixel, Color::rgb(0xaa, 0xbb, 0xcc));
    assert_eq!(u32::from_ne_bytes(pixel), 0xffaabbcc);
}

#[test]
fn parses_char_matcher_patterns() {
    let matchers = parse_char_matchers("U+E000-U+F8FF");
    assert_eq!(matchers, vec![CharMatcher::Range(0xE000..=0xF8FF)]);
    assert!(matchers[0].matches('\u{E000}'));
    assert!(matchers[0].matches('\u{F000}'));
    assert!(!matchers[0].matches('A'));

    let multi = parse_char_matchers("U+0041, U+0042-U+0044, 0x0045..=0x0046, X");
    assert!(multi.iter().any(|m| m.matches('A')));
    assert!(multi.iter().any(|m| m.matches('C')));
    assert!(multi.iter().any(|m| m.matches('F')));
    assert!(multi.iter().any(|m| m.matches('X')));
    assert!(!multi.iter().any(|m| m.matches('Z')));

    let literals = parse_char_matchers("󰀝󰂄");
    assert!(literals.iter().any(|m| m.matches('󰀝')));
    assert!(literals.iter().any(|m| m.matches('󰂄')));
    assert!(!literals.iter().any(|m| m.matches('a')));
}

#[test]
fn resolves_font_config_with_mappings_and_fallbacks() {
    use std::collections::BTreeMap;
    let mut map = BTreeMap::new();
    map.insert("U+E000-U+F8FF".to_owned(), "Symbols Nerd Font".to_owned());
    let entries = vec![
        FontFamilyEntry::Mapping(map),
        FontFamilyEntry::Family("JetBrains Mono".to_owned()),
        FontFamilyEntry::Family("Noto Sans CJK SC".to_owned()),
    ];
    let resolved = ResolvedFontConfig::from_entries(&entries);
    assert_eq!(resolved.primary_family, "JetBrains Mono");
    assert_eq!(resolved.fallback_families, vec!["Noto Sans CJK SC"]);
    assert_eq!(resolved.candidates.len(), 3);
    assert!(matches!(
        &resolved.candidates[0],
        FontCandidate::Mapping { matchers, .. }
            if matchers.iter().any(|matcher| matcher.matches('\u{E005}'))
    ));
    assert!(
        resolved
            .all_load_families
            .contains(&"Symbols Nerd Font".to_owned())
    );
    assert!(
        resolved
            .all_load_families
            .contains(&"JetBrains Mono".to_owned())
    );
    assert!(
        resolved
            .all_load_families
            .contains(&"Noto Sans CJK SC".to_owned())
    );
    assert_eq!(resolved.regular_families, vec![1, 2]);
}

#[test]
fn no_mapping_keeps_plain_text_shaping() {
    let renderer =
        TextRenderer::from_family_names(&["monospace"], 12.0, FontStyleRequirements::default());
    assert!(!renderer.has_mappings);
}

#[test]
fn font_match_resolves_one_file_and_accepts_paths() {
    let matched = find_fonts("monospace", "regular", "roman");
    if matched.is_empty() {
        eprintln!("skipping font integration test: Fontconfig unavailable");
        return;
    }
    assert_eq!(matched.len(), 1);
    assert_eq!(find_fonts(&matched[0], "bold", "italic"), matched);
}

#[test]
fn media_private_use_glyphs_keep_full_cluster_ranges() {
    if find_fonts("JetBrainsMono Nerd Font", "regular", "roman").is_empty() {
        eprintln!("skipping media glyph test: font unavailable");
        return;
    }
    let mut renderer = TextRenderer::from_family_names(
        &["JetBrainsMono Nerd Font"],
        14.0,
        FontStyleRequirements::default(),
    );
    let glyphs = renderer.shape_span("󰏤󰐊", crate::config::Theme::default().default_style, 30, 1);
    assert_eq!(glyphs.len(), 2);
    assert_eq!((glyphs[0].start, glyphs[0].end), (0, 4));
    assert_eq!((glyphs[1].start, glyphs[1].end), (4, 8));
}

#[test]
fn media_private_use_glyphs_paint_inside_reserved_cells() {
    if find_fonts("JetBrainsMono Nerd Font", "regular", "roman").is_empty() {
        eprintln!("skipping media glyph paint test: font unavailable");
        return;
    }
    let mut renderer = TextRenderer::from_family_names(
        &["JetBrainsMono Nerd Font"],
        14.0,
        FontStyleRequirements::default(),
    );
    let style = test_style();
    let mut frame = CellFrame::new(2, style);
    layout_regions(
        &mut frame,
        RegionText {
            left: &[StyledText {
                text: "󰏤󰐊".into(),
                style,
                owner: Some(0),
            }],
            center: &[],
            right: &[],
        },
    );
    let cell_width = renderer.cell_width();
    let width = cell_width * 2;
    let height = 30;
    let background = Rgba {
        red: 0,
        green: 0,
        blue: 0,
        alpha: 255,
    };
    let mut canvas = vec![0; (width * height * 4) as usize];
    renderer.paint_frame(
        PaintTarget {
            canvas: &mut canvas,
            width,
            height,
            scale: 1,
            background,
            radius: 0,
        },
        &frame,
        0,
    );
    let black = premultiplied_argb(background).to_ne_bytes();
    let mut counts = [0usize; 2];
    for (index, pixel) in canvas.as_chunks::<4>().0.iter().enumerate() {
        if pixel != &black {
            counts[(index % width as usize) / cell_width as usize] += 1;
        }
    }
    assert!(counts.iter().all(|&count| count > 0), "{counts:?}");
}

#[test]
fn missing_glyph_is_visible_to_shaping() {
    let mut renderer =
        TextRenderer::from_family_names(&["monospace"], 12.0, FontStyleRequirements::default());
    let glyphs = renderer.shape_span("\u{10ffff}", test_style(), 24, 1);
    assert!(glyphs.iter().any(|glyph| glyph.glyph_id == 0));
}

#[test]
fn mapping_before_primary_does_not_preload_its_bold() {
    use std::collections::BTreeMap;
    let mapping =
        FontFamilyEntry::Mapping(BTreeMap::from([("U+E000".into(), "sans-serif".into())]));
    let renderer = TextRenderer::new(
        &[mapping, FontFamilyEntry::Family("monospace".into())],
        12.0,
        0,
        crate::config::DEFAULT_LINE_HEIGHT,
        FontStyleRequirements {
            bold: true,
            italic: false,
        },
    );
    assert!(!renderer.lazy_fonts.attempted.contains(&(0, true, false)));
    assert!(renderer.lazy_fonts.attempted.contains(&(1, true, false)));
}

#[test]
fn mapping_order_and_lazy_loading() {
    use std::collections::BTreeMap;
    let paths = find_fonts("monospace", "regular", "roman");
    if paths.is_empty() {
        eprintln!("skipping font integration test: monospace unavailable");
        return;
    }
    let path = paths[0].clone();
    let mapping = FontFamilyEntry::Mapping(BTreeMap::from([("U+0041".into(), path.clone())]));
    let regular = FontFamilyEntry::Family("monospace".into());
    let mut after = TextRenderer::new(
        &[regular.clone(), mapping.clone()],
        12.0,
        0,
        crate::config::DEFAULT_LINE_HEIGHT,
        FontStyleRequirements::default(),
    );
    assert!(!after.lazy_fonts.attempted.contains(&(1, false, false)));
    assert!(!after.text_matches_mapping("hello"));
    assert!(after.text_matches_mapping("A"));
    after.prepare(
        &[StyledText {
            text: "hello".into(),
            style: crate::config::Theme::default().default_style,
            owner: None,
        }],
        30,
        1,
    );
    assert!(after.lazy_fonts.coverage.is_empty());
    assert!(!after.lazy_fonts.attempted.contains(&(1, false, false)));
    assert_eq!(after.family_for_cluster("A"), Some(0));
    assert_eq!(after.lazy_fonts.coverage.get(&(0, 'A')), Some(&true));
    assert!(!after.lazy_fonts.attempted.contains(&(1, false, false)));
    assert_eq!(after.family_for_cluster("B"), Some(0));

    let mut before = TextRenderer::new(
        &[mapping, regular],
        12.0,
        0,
        crate::config::DEFAULT_LINE_HEIGHT,
        FontStyleRequirements::default(),
    );
    assert!(!before.lazy_fonts.attempted.contains(&(0, false, false)));
    assert_eq!(before.family_for_cluster("B"), Some(1));
    assert!(!before.lazy_fonts.attempted.contains(&(0, false, false)));
    assert_eq!(before.family_for_cluster("A"), Some(0));
    assert!(before.lazy_fonts.attempted.contains(&(0, false, false)));
    assert_eq!(before.lazy_fonts.coverage.get(&(0, 'A')), Some(&true));
}

#[test]
fn segment_cache_reuses_unmodified_spans() {
    let mut renderer =
        TextRenderer::from_family_names(&["monospace"], 12.0, FontStyleRequirements::default());
    let style = crate::config::Theme::default().default_style;
    let spans_1 = vec![
        StyledText {
            text: "workspace 1".into(),
            style,
            owner: Some(0),
        },
        StyledText {
            text: "12:00:00".into(),
            style,
            owner: Some(1),
        },
    ];
    renderer.prepare(&spans_1, 30, 1);
    let initial_workspace_glyphs = renderer.prepared.as_ref().unwrap().segments[0]
        .glyphs
        .clone();

    // Second update: clock changes, workspace remains unchanged
    let spans_2 = vec![
        StyledText {
            text: "workspace 1".into(),
            style,
            owner: Some(0),
        },
        StyledText {
            text: "12:00:01".into(),
            style,
            owner: Some(1),
        },
    ];
    renderer.prepare(&spans_2, 30, 1);
    let updated = renderer.prepared.as_ref().unwrap();
    assert_eq!(updated.segments.len(), 2);
    assert_eq!(updated.segments[0].text, "workspace 1");
    assert_eq!(updated.segments[1].text, "12:00:01");
    assert_eq!(
        updated.segments[0].glyphs.len(),
        initial_workspace_glyphs.len()
    );
    assert_eq!(
        updated.segments[0].glyphs[0].glyph_id,
        initial_workspace_glyphs[0].glyph_id
    );
}

#[test]
fn gamma_corrected_subpixel_blending_brightens_midtones_and_balances_channels() {
    use super::draw::{Color, blend_channel_gamma, blend_pixel_subpixel_gamma};

    // 50% coverage of white over black in gamma 2.2 space should be ~186 (much brighter than naive 128)
    let mid = blend_channel_gamma(255, 0, 128);
    assert!(
        (184..=188).contains(&mid),
        "expected ~186 for 50% gamma-2.2 blend, got {mid}"
    );

    // Subpixel blending on an opaque dark background
    let mut pixel = 0xff_00_00_00u32.to_ne_bytes();
    blend_pixel_subpixel_gamma(&mut pixel, Color::rgba(255, 255, 255, 255), 255, 0, 0);
    let result = u32::from_ne_bytes(pixel);
    let out_a = (result >> 24) & 0xff;
    let out_r = (result >> 16) & 0xff;
    let out_g = (result >> 8) & 0xff;
    let out_b = result & 0xff;

    assert_eq!(out_a, 255);
    assert!(
        out_r > out_g,
        "red subpixel should dominate: r={out_r}, g={out_g}"
    );
    assert_eq!(out_g, out_b);
    assert!(
        out_g > 0,
        "3-tap filter should soften pure red edge to prevent fringing"
    );
}

#[test]
fn capsule_shell_and_inner_highlight_render_properly() {
    let mut renderer =
        TextRenderer::from_family_names(&["monospace"], 12.0, FontStyleRequirements::default());
    let cell_width = renderer.cell_width();
    let width = cell_width * 6;
    let height = 24;
    let bar_bg = Rgba {
        red: 0,
        green: 0,
        blue: 0,
        alpha: 255,
    };
    let capsule_color = Rgba {
        red: 40,
        green: 80,
        blue: 120,
        alpha: 255,
    };
    let inner_highlight = Rgba {
        red: 200,
        green: 60,
        blue: 60,
        alpha: 255,
    };

    let mut frame = CellFrame::new(6, test_style());
    // Columns 1..5 form a 4-cell capsule, with column 2 having an inner cell highlight
    for col in 1..5 {
        frame.cells[col].symbol = " ".into();
        frame.cells[col].capsule = Some(CapsuleShell {
            background: capsule_color,
            inset_y: 0,
            radius: Some(6),
        });
        frame.cells[col].owner = Some(0);
        if col == 2 {
            frame.cells[col].style.background = Some(inner_highlight);
        }
    }

    let mut canvas = vec![0; (width * height * 4) as usize];
    renderer.paint_frame(
        PaintTarget {
            canvas: &mut canvas,
            width,
            height,
            scale: 1,
            background: bar_bg,
            radius: 0,
        },
        &frame,
        0,
    );

    let bar_pixel = premultiplied_argb(bar_bg).to_ne_bytes();
    let capsule_pixel = premultiplied_argb(capsule_color).to_ne_bytes();
    let highlight_pixel = premultiplied_argb(inner_highlight).to_ne_bytes();

    let y_mid = height / 2;

    // Top-left corner pixel of the capsule (x = cell_width, y = 0) should be clipped by radius=6 (remains bar_bg)
    let corner_idx = (cell_width * 4) as usize;
    assert_eq!(&canvas[corner_idx..corner_idx + 4], &bar_pixel);

    // Top padding region of the capsule (x = cell_width + cell_width / 2, y = 1) is painted with capsule_color (surface height)
    let col1_top = ((width + cell_width + cell_width / 2) * 4) as usize;
    assert_eq!(&canvas[col1_top..col1_top + 4], &capsule_pixel);

    // Center of column 1 (x = cell_width + cell_width / 2, y = y_mid) should be capsule_color
    let col1_mid = ((y_mid * width + cell_width + cell_width / 2) * 4) as usize;
    assert_eq!(&canvas[col1_mid..col1_mid + 4], &capsule_pixel);

    // Center of column 2 (x = cell_width * 2 + cell_width / 2, y = y_mid) should be inner_highlight
    let col2_mid = ((y_mid * width + cell_width * 2 + cell_width / 2) * 4) as usize;
    assert_eq!(&canvas[col2_mid..col2_mid + 4], &highlight_pixel);

    // In capsule mode, inner highlight seamlessly matches capsule height (y = 1 is inner_highlight)
    let col2_top = ((width + cell_width * 2 + cell_width / 2) * 4) as usize;
    assert_eq!(&canvas[col2_top..col2_top + 4], &highlight_pixel);
}

#[test]
fn rounded_rect_coverage_antialiasing() {
    use super::draw::rounded_rect_coverage;
    // Inside center
    assert_eq!(rounded_rect_coverage(10, 10, 20, 20, 8), 255);
    // Outside corner
    assert_eq!(rounded_rect_coverage(0, 0, 20, 20, 8), 0);
    // An edge pixel on the corner curve should have partial coverage (0 < cov < 255)
    let edge_cov = rounded_rect_coverage(1, 3, 20, 20, 8);
    assert!(
        edge_cov > 0 && edge_cov < 255,
        "edge coverage should be fractional for antialiasing, got {edge_cov}"
    );
}
