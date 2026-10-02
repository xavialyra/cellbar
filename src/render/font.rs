use std::{
    collections::{HashMap, HashSet},
    process::Command,
};

use crate::config::{DisplayStyle, FontFamilyEntry};

use super::draw::Color;

#[derive(Clone, Debug)]
pub struct ShapedGlyph {
    pub font_id: fontdb::ID,
    pub glyph_id: u16,
    pub x: f32,
    #[allow(dead_code)]
    pub y: f32,
    pub w: f32,
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct GlyphKey {
    pub font_id: fontdb::ID,
    pub glyph_id: u16,
    pub font_size_bits: u32,
    pub subpixel_x: u8,
    pub bold: bool,
    pub italic: bool,
}

pub struct CachedGlyph {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    pub content: swash::scale::image::Content,
    pub data: Vec<u8>,
}

pub const MAX_CACHED_GLYPHS: usize = 256;

pub struct SwashCache {
    pub entries: HashMap<GlyphKey, (u64, CachedGlyph)>,
    pub access_counter: u64,
}

pub fn font_cache_id(id: fontdb::ID, index: u32) -> [u64; 2] {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut hasher);
    [hasher.finish(), index as u64]
}

impl SwashCache {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            access_counter: 0,
        }
    }

    pub fn get_glyph(
        &mut self,
        font_db: &fontdb::Database,
        scale_context: &mut swash::scale::ScaleContext,
        key: GlyphKey,
    ) -> Option<&CachedGlyph> {
        self.access_counter = self.access_counter.wrapping_add(1);
        let counter = self.access_counter;

        if let Some(entry) = self.entries.get_mut(&key) {
            entry.0 = counter;
        } else {
            if self.entries.len() >= MAX_CACHED_GLYPHS {
                let mut timestamps: Vec<u64> = self.entries.values().map(|(t, _)| *t).collect();
                timestamps.sort_unstable();
                let cutoff = timestamps.get(MAX_CACHED_GLYPHS / 4).copied().unwrap_or(0);
                self.entries.retain(|_, (t, _)| *t > cutoff);
            }

            let rendered = font_db
                .with_face_data(key.font_id, |data, index| {
                    let font_ref = swash::FontRef::from_index(data, index as usize)?;
                    let font_size = f32::from_bits(key.font_size_bits);
                    let mut scaler = scale_context
                        .builder_with_id(font_ref, font_cache_id(key.font_id, index))
                        .size(font_size)
                        .hint(true)
                        .build();
                    let subpixel = (key.subpixel_x as f32) / 4.0;
                    swash::scale::Render::new(&[
                        swash::scale::Source::ColorOutline(0),
                        swash::scale::Source::ColorBitmap(swash::scale::StrikeWith::BestFit),
                        swash::scale::Source::Outline,
                    ])
                    .format(swash::zeno::Format::Subpixel)
                    .offset(swash::zeno::Vector::new(subpixel, 0.0))
                    .render(&mut scaler, key.glyph_id)
                })
                .flatten();

            if let Some(rendered) = rendered {
                let entry = CachedGlyph {
                    left: rendered.placement.left,
                    top: rendered.placement.top,
                    width: rendered.placement.width,
                    height: rendered.placement.height,
                    content: rendered.content,
                    data: rendered.data,
                };
                self.entries.insert(key, (counter, entry));
            }
        }

        self.entries.get(&key).map(|(_, cached)| cached)
    }

    #[allow(dead_code)]
    pub fn with_pixels<F>(
        &mut self,
        font_db: &fontdb::Database,
        scale_context: &mut swash::scale::ScaleContext,
        key: GlyphKey,
        color: Color,
        mut callback: F,
    ) where
        F: FnMut(i32, i32, Color),
    {
        let Some(cached) = self.get_glyph(font_db, scale_context, key) else {
            return;
        };

        let (left, top, w, h) = (cached.left, cached.top, cached.width, cached.height);
        match cached.content {
            swash::scale::image::Content::Mask => {
                for y in 0..h {
                    for x in 0..w {
                        let alpha = cached.data[(y * w + x) as usize];
                        if alpha > 0 {
                            let mut pixel_color = color;
                            pixel_color.3 = ((pixel_color.3 as u16 * alpha as u16) / 255) as u8;
                            callback(left + x as i32, -top + y as i32, pixel_color);
                        }
                    }
                }
            }
            swash::scale::image::Content::SubpixelMask => {
                for y in 0..h {
                    for x in 0..w {
                        let idx = ((y * w + x) * 4) as usize;
                        let alpha = ((cached.data[idx] as u32
                            + cached.data[idx + 1] as u32
                            + cached.data[idx + 2] as u32)
                            / 3) as u8;
                        if alpha > 0 {
                            let mut pixel_color = color;
                            pixel_color.3 = ((pixel_color.3 as u16 * alpha as u16) / 255) as u8;
                            callback(left + x as i32, -top + y as i32, pixel_color);
                        }
                    }
                }
            }
            swash::scale::image::Content::Color => {
                for y in 0..h {
                    for x in 0..w {
                        let idx = ((y * w + x) * 4) as usize;
                        let r = cached.data[idx];
                        let g = cached.data[idx + 1];
                        let b = cached.data[idx + 2];
                        let a = cached.data[idx + 3];
                        if a > 0 {
                            callback(left + x as i32, -top + y as i32, Color::rgba(r, g, b, a));
                        }
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy, Default)]
pub struct FontStyleRequirements {
    pub bold: bool,
    pub italic: bool,
}

// Load fallback faces after a missing glyph is shaped, and styled variants
// only after shaping selects a face from that family.
#[derive(Default)]
pub struct LazyFontStyles {
    pub families: Vec<String>,
    pub file_families: HashMap<String, usize>,
    pub family_paths: Vec<Vec<String>>,
    pub attempted: HashSet<(usize, bool, bool)>,
    pub coverage: HashMap<(usize, char), bool>,
    pub generation: usize,
}

impl LazyFontStyles {
    pub fn load_regular(&mut self, db: &mut fontdb::Database, family: usize) {
        if !self.attempted.insert((family, false, false)) {
            return;
        }
        for path in find_fonts(&self.families[family], "regular", "roman") {
            if !self.file_families.contains_key(&path) {
                match db.load_font_file(&path) {
                    Ok(()) => {
                        self.file_families.insert(path.clone(), family);
                        self.generation += 1;
                    }
                    Err(error) => eprintln!("cellbar: cannot load font {path:?}: {error}"),
                }
            }
            if self.file_families.contains_key(&path) {
                self.family_paths[family].push(path);
            }
        }
    }

    pub fn has_glyph(&mut self, db: &fontdb::Database, family: usize, c: char) -> bool {
        if let Some(&found) = self.coverage.get(&(family, c)) {
            return found;
        }
        let found = db.faces().any(|face| {
            let path = match &face.source {
                fontdb::Source::File(path) | fontdb::Source::SharedFile(path, _) => path,
                fontdb::Source::Binary(_) => return false,
            };
            self.family_paths[family]
                .iter()
                .any(|name| path.to_string_lossy() == name.as_str())
                && db
                    .with_face_data(face.id, |data, index| {
                        ttf_parser::Face::parse(data, index)
                            .is_ok_and(|font| font.glyph_index(c).is_some())
                    })
                    .unwrap_or(false)
        });
        if self.coverage.len() >= 4096 {
            self.coverage.clear();
        }
        self.coverage.insert((family, c), found);
        found
    }

    pub fn load_used_styles(
        &mut self,
        db: &mut fontdb::Database,
        glyphs: &[ShapedGlyph],
        style: DisplayStyle,
    ) -> bool {
        if !style.bold && !style.italic {
            return false;
        }
        let mut families = Vec::new();
        for glyph in glyphs {
            let Some(face) = db.face(glyph.font_id) else {
                continue;
            };
            let path = match &face.source {
                fontdb::Source::File(path) | fontdb::Source::SharedFile(path, _) => path,
                fontdb::Source::Binary(_) => continue,
            };
            if let Some(&family) = self.file_families.get(path.to_string_lossy().as_ref())
                && !families.contains(&family)
            {
                families.push(family);
            }
        }
        let mut changed = false;
        for family in families {
            if !self.attempted.insert((family, style.bold, style.italic)) {
                continue;
            }
            let weight = if style.bold { "bold" } else { "regular" };
            let slant = if style.italic { "italic" } else { "roman" };
            for path in find_fonts(&self.families[family], weight, slant) {
                if self.file_families.contains_key(&path) {
                    continue;
                }
                match db.load_font_file(&path) {
                    Ok(()) => {
                        self.file_families.insert(path, family);
                        changed = true;
                    }
                    Err(error) => eprintln!("cellbar: cannot load font {path:?}: {error}"),
                }
            }
        }
        if changed {
            self.generation += 1;
        }
        changed
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CharMatcher {
    Range(std::ops::RangeInclusive<u32>),
    Char(char),
}

impl CharMatcher {
    #[inline]
    pub fn matches(&self, c: char) -> bool {
        match self {
            Self::Range(range) => range.contains(&(c as u32)),
            Self::Char(target) => *target == c,
        }
    }
}

pub fn parse_char_matchers(pattern: &str) -> Vec<CharMatcher> {
    let mut matchers = Vec::new();
    for token in pattern.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let mut is_range = false;
        for sep in ["-", "..="] {
            if let Some((start_s, end_s)) = token.split_once(sep) {
                let start_hex = start_s
                    .trim()
                    .strip_prefix("U+")
                    .or_else(|| start_s.trim().strip_prefix("u+"))
                    .or_else(|| start_s.trim().strip_prefix("0x"))
                    .or_else(|| start_s.trim().strip_prefix("0X"))
                    .unwrap_or(start_s.trim());
                let end_hex = end_s
                    .trim()
                    .strip_prefix("U+")
                    .or_else(|| end_s.trim().strip_prefix("u+"))
                    .or_else(|| end_s.trim().strip_prefix("0x"))
                    .or_else(|| end_s.trim().strip_prefix("0X"))
                    .unwrap_or(end_s.trim());
                if let (Ok(start), Ok(end)) = (
                    u32::from_str_radix(start_hex, 16),
                    u32::from_str_radix(end_hex, 16),
                ) && start <= end
                    && end <= 0x10FFFF
                {
                    matchers.push(CharMatcher::Range(start..=end));
                    is_range = true;
                    break;
                }
            }
        }
        if is_range {
            continue;
        }
        let single_hex = token
            .strip_prefix("U+")
            .or_else(|| token.strip_prefix("u+"))
            .or_else(|| token.strip_prefix("0x"))
            .or_else(|| token.strip_prefix("0X"));
        if let Some(hex) = single_hex
            && let Ok(cp) = u32::from_str_radix(hex.trim(), 16)
            && cp <= 0x10FFFF
        {
            matchers.push(CharMatcher::Range(cp..=cp));
            continue;
        }
        for c in token.chars() {
            matchers.push(CharMatcher::Char(c));
        }
    }
    matchers
}

#[derive(Debug, Clone)]
pub enum FontCandidate {
    Family(usize),
    Mapping {
        matchers: Vec<CharMatcher>,
        family: usize,
    },
}

#[derive(Debug, Clone)]
pub struct ResolvedFontConfig {
    pub primary_family: String,
    #[allow(dead_code)]
    pub fallback_families: Vec<String>,
    pub all_load_families: Vec<String>,
    pub candidates: Vec<FontCandidate>,
    pub regular_families: Vec<usize>,
}

impl ResolvedFontConfig {
    pub fn from_entries(entries: &[FontFamilyEntry]) -> Self {
        let mut primary_family = None;
        let mut fallback_families = Vec::new();
        let mut all_load_families = Vec::new();
        let mut candidates = Vec::new();
        let mut regular_families = Vec::new();

        let mut family_index = |name: &str| {
            if let Some(index) = all_load_families.iter().position(|n| n == name) {
                index
            } else {
                let index = all_load_families.len();
                all_load_families.push(name.to_owned());
                index
            }
        };

        for entry in entries {
            match entry {
                FontFamilyEntry::Family(name) => {
                    if name.trim().is_empty() {
                        continue;
                    }
                    if primary_family.is_none() {
                        primary_family = Some(name.clone());
                    } else if !fallback_families.contains(name) {
                        fallback_families.push(name.clone());
                    }
                    let index = family_index(name);
                    candidates.push(FontCandidate::Family(index));
                    if !regular_families.contains(&index) {
                        regular_families.push(index);
                    }
                }
                FontFamilyEntry::Mapping(map) => {
                    for (pattern, target) in map {
                        if pattern.trim().is_empty() || target.trim().is_empty() {
                            continue;
                        }
                        let matchers = parse_char_matchers(pattern);
                        if !matchers.is_empty() {
                            let family = family_index(target);
                            candidates.push(FontCandidate::Mapping { matchers, family });
                        }
                    }
                }
            }
        }

        let primary_family = primary_family.unwrap_or_else(|| "monospace".to_owned());
        if candidates.is_empty() {
            let index = family_index(&primary_family);
            candidates.push(FontCandidate::Family(index));
            regular_families.push(index);
        }

        Self {
            primary_family,
            fallback_families,
            all_load_families,
            candidates,
            regular_families,
        }
    }
}

pub fn load_fonts(
    resolved: &ResolvedFontConfig,
    requirements: FontStyleRequirements,
) -> (fontdb::Database, LazyFontStyles) {
    let mut database = fontdb::Database::new();
    let mut loaded = Vec::new();
    let mut lazy = LazyFontStyles {
        families: resolved.all_load_families.clone(),
        family_paths: vec![Vec::new(); resolved.all_load_families.len()],
        ..LazyFontStyles::default()
    };
    let primary_family = resolved.regular_families.first().copied();

    for &family in resolved.regular_families.iter().take(1) {
        let query = &resolved.all_load_families[family];
        let mut variants = vec![("regular", "roman")];
        if Some(family) == primary_family && requirements.bold {
            variants.push(("bold", "roman"));
        }
        if Some(family) == primary_family && requirements.italic {
            variants.push(("regular", "italic"));
        }
        if Some(family) == primary_family && requirements.bold && requirements.italic {
            variants.push(("bold", "italic"));
        }

        let mut found = false;
        for (weight, slant) in variants {
            lazy.attempted
                .insert((family, weight == "bold", slant == "italic"));
            for path in find_fonts(query, weight, slant) {
                found = true;
                if loaded.iter().any(|loaded| loaded == &path) {
                    if !lazy.family_paths[family].contains(&path) {
                        lazy.family_paths[family].push(path);
                    }
                    continue;
                }
                if let Err(error) = database.load_font_file(&path) {
                    eprintln!("cellbar: cannot load font {path:?}: {error}");
                } else {
                    lazy.file_families.insert(path.clone(), family);
                    lazy.family_paths[family].push(path.clone());
                    loaded.push(path);
                }
            }
        }
        if !found {
            eprintln!("cellbar: fc-match found no fonts for {query:?}");
        }
    }
    if loaded.is_empty() {
        eprintln!(
            "cellbar: no configured fonts could be loaded; attempting generic monospace match"
        );
        for path in find_fonts("monospace", "regular", "roman") {
            if let Err(error) = database.load_font_file(&path) {
                eprintln!("cellbar: cannot load fallback font {path:?}: {error}");
            } else {
                lazy.file_families.insert(path.clone(), 0);
                lazy.family_paths[0].push(path.clone());
                loaded.push(path);
                break;
            }
        }
    }
    if loaded.is_empty() {
        eprintln!("cellbar: warning: no fonts could be loaded; text rendering will be blank");
    }
    (database, lazy)
}

pub fn find_fonts(query: &str, weight: &str, slant: &str) -> Vec<String> {
    let query_path = if query.starts_with("~/") {
        std::env::var("HOME")
            .ok()
            .map(|h| format!("{h}/{}", &query[2..]))
    } else {
        None
    };
    let candidate_path = query_path.as_deref().unwrap_or(query);
    if std::path::Path::new(candidate_path).is_file() {
        return vec![candidate_path.to_owned()];
    }

    let pattern = format!("{query}:weight={weight}:slant={slant}");

    static CACHE: std::sync::Mutex<Option<std::collections::HashMap<String, Vec<String>>>> =
        std::sync::Mutex::new(None);
    if let Ok(guard) = CACHE.lock()
        && let Some(cache) = guard.as_ref()
        && let Some(paths) = cache.get(&pattern)
    {
        return paths.clone();
    }

    let Ok(output) = Command::new("fc-match")
        .args([&pattern, "-f", "%{file}\n"])
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let file = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let result = if std::path::Path::new(&file).is_file() {
        vec![file]
    } else {
        Vec::new()
    };

    if let Ok(mut guard) = CACHE.lock() {
        let cache = guard.get_or_insert_with(std::collections::HashMap::new);
        cache.insert(pattern, result.clone());
    }

    result
}
