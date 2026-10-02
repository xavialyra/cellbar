use std::{collections::BTreeMap, path::Path, str::FromStr};

use serde::{Deserialize, Deserializer};
use thiserror::Error;

use crate::config::{ConfigError, ValidationErrors, read_utf8, valid_name};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum FontFamilyEntry {
    Family(String),
    Mapping(BTreeMap<String, String>),
}

impl From<String> for FontFamilyEntry {
    fn from(s: String) -> Self {
        Self::Family(s)
    }
}

impl From<&str> for FontFamilyEntry {
    fn from(s: &str) -> Self {
        Self::Family(s.to_owned())
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FontConfig {
    #[serde(default = "default_font_families")]
    pub families: Vec<FontFamilyEntry>,
    #[serde(default = "default_font_size")]
    pub size: f32,
    #[serde(default)]
    pub cell_width_adjust: i32,
    #[serde(default = "default_line_height")]
    pub line_height: f32,
}

impl FontConfig {
    pub fn font_names(&self) -> Vec<String> {
        let mut names = Vec::new();
        for entry in &self.families {
            match entry {
                FontFamilyEntry::Family(name) => {
                    if !names.contains(name) {
                        names.push(name.clone());
                    }
                }
                FontFamilyEntry::Mapping(map) => {
                    for font in map.values() {
                        if !names.contains(font) {
                            names.push(font.clone());
                        }
                    }
                }
            }
        }
        names
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceConfig {
    pub background: String,
    #[serde(default)]
    pub padding: SurfacePadding,
    #[serde(default)]
    pub radius: u32,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SurfacePadding {
    #[serde(default = "default_surface_padding")]
    pub horizontal: u32,
    #[serde(default = "default_surface_padding")]
    pub vertical: u32,
}

impl Default for SurfacePadding {
    fn default() -> Self {
        Self {
            horizontal: default_surface_padding(),
            vertical: default_surface_padding(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThemeConfig {
    pub font: FontConfig,
    pub surface: SurfaceConfig,
    #[serde(default)]
    pub colors: BTreeMap<String, Rgba>,
    #[serde(default)]
    pub text: StyleConfig,
    #[serde(default)]
    pub styles: BTreeMap<String, StyleConfig>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StyleConfig {
    #[serde(default)]
    pub foreground: Option<String>,
    #[serde(default)]
    pub background: Option<String>,
    #[serde(default)]
    pub bold: bool,
    #[serde(default)]
    pub italic: bool,
    #[serde(default)]
    pub underline: bool,
    #[serde(default)]
    pub strikethrough: bool,
    #[serde(default)]
    pub inset_y: Option<u32>,
    #[serde(default)]
    pub radius: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DisplayStyle {
    pub foreground: Rgba,
    pub background: Option<Rgba>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub inset_y: u32,
    pub radius: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct Theme {
    pub font: FontConfig,
    pub background: Rgba,
    pub default_style: DisplayStyle,
    pub padding: SurfacePadding,
    pub radius: u32,
    pub(crate) styles: BTreeMap<String, DisplayStyle>,
}

impl Default for Theme {
    fn default() -> Self {
        let background = Rgba {
            red: 0x18,
            green: 0x1a,
            blue: 0x1f,
            alpha: 0xff,
        };
        let foreground = Rgba {
            red: 0xd8,
            green: 0xde,
            blue: 0xe9,
            alpha: 0xff,
        };
        let default_style = DisplayStyle {
            foreground,
            background: None,
            bold: false,
            italic: false,
            underline: false,
            strikethrough: false,
            inset_y: 0,
            radius: None,
        };
        Self {
            font: FontConfig {
                families: default_font_families(),
                size: default_font_size(),
                cell_width_adjust: 0,
                line_height: default_line_height(),
            },
            background,
            default_style,
            padding: SurfacePadding::default(),
            radius: 0,
            styles: BTreeMap::new(),
        }
    }
}

impl Theme {
    pub fn cell_height(&self) -> u32 {
        cell_height(self.font.size, self.font.line_height)
    }

    pub fn bar_height(&self) -> u32 {
        self.cell_height() + self.padding.vertical.saturating_mul(2)
    }

    pub fn style_for(&self, name: Option<&str>) -> Option<DisplayStyle> {
        match name {
            Some(name) => self.styles.get(name).copied(),
            None => Some(self.default_style),
        }
    }

    pub(crate) fn validate(&self, errors: &mut Vec<String>) {
        if self.font.families.is_empty() {
            errors.push("theme.font.families must contain at least one font".to_owned());
        }
        for (index, entry) in self.font.families.iter().enumerate() {
            match entry {
                FontFamilyEntry::Family(family) => {
                    if family.trim().is_empty() {
                        errors.push(format!("theme.font.families[{index}] must not be empty"));
                    }
                }
                FontFamilyEntry::Mapping(map) => {
                    if map.is_empty() {
                        errors.push(format!(
                            "theme.font.families[{index}] mapping must not be empty"
                        ));
                    }
                    for (pattern, target) in map {
                        if pattern.trim().is_empty() {
                            errors.push(format!(
                                "theme.font.families[{index}] mapping pattern must not be empty"
                            ));
                        }
                        if target.trim().is_empty() {
                            errors.push(format!(
                                "theme.font.families[{index}] mapping target for '{pattern}' must not be empty"
                            ));
                        }
                    }
                }
            }
        }
        if !self.font.size.is_finite() || !(1.0..=256.0).contains(&self.font.size) {
            errors.push("theme.font.size must be between 1 and 256".to_owned());
        }
        if !self.font.line_height.is_finite() || !(0.5..=4.0).contains(&self.font.line_height) {
            errors.push(
                "theme.font.line_height must be between 0.5 and 4 multiples of font.size"
                    .to_owned(),
            );
        }
        if !(-256..=256).contains(&self.font.cell_width_adjust) {
            errors.push(
                "theme.font.cell_width_adjust must be between -256 and 256 logical pixels"
                    .to_owned(),
            );
        }
        if self.padding.horizontal > 1024 {
            errors.push(
                "theme.surface.padding.horizontal must be at most 1024 logical pixels".to_owned(),
            );
        }
        if self.padding.vertical > 128 {
            errors.push(
                "theme.surface.padding.vertical must be at most 128 logical pixels".to_owned(),
            );
        }
        if self.bar_height() > 512 {
            errors.push("derived bar height must be at most 512 logical pixels".to_owned());
        }
        if self.radius > 1024 {
            errors.push("theme.surface.radius must be at most 1024 logical pixels".to_owned());
        }
        for (name, style) in &self.styles {
            if style.inset_y > 256 {
                errors.push(format!(
                    "theme.styles.{name}.inset_y must be at most 256 logical pixels"
                ));
            }
            if let Some(r) = style.radius
                && r > 512
            {
                errors.push(format!(
                    "theme.styles.{name}.radius must be at most 512 logical pixels"
                ));
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgba {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
    pub alpha: u8,
}

impl FromStr for Rgba {
    type Err = ColorParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let hex = value.strip_prefix('#').ok_or(ColorParseError)?;
        if hex.len() != 6 && hex.len() != 8 {
            return Err(ColorParseError);
        }
        let channel = |range| u8::from_str_radix(&hex[range], 16).map_err(|_| ColorParseError);
        Ok(Self {
            red: channel(0..2)?,
            green: channel(2..4)?,
            blue: channel(4..6)?,
            alpha: if hex.len() == 8 { channel(6..8)? } else { 255 },
        })
    }
}

impl<'de> Deserialize<'de> for Rgba {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("expected #RRGGBB or #RRGGBBAA")]
pub struct ColorParseError;

impl ThemeConfig {
    pub(crate) fn resolve(self) -> Result<Theme, Vec<String>> {
        let mut errors = Vec::new();
        for name in self.colors.keys() {
            if !valid_name(name) {
                errors.push(format!(
                    "theme color {name:?}: name must use ASCII letters, digits, '-' or '_'"
                ));
            }
        }
        for name in self.styles.keys() {
            if !valid_name(name) {
                errors.push(format!(
                    "theme style {name:?}: name must use ASCII letters, digits, '-' or '_'"
                ));
            }
            if name == "default" {
                errors.push(
                    "theme style \"default\" is reserved; set the base text style with \
                     theme.text"
                        .to_owned(),
                );
            }
        }

        let background = self.resolve_required(
            "theme.surface.background",
            &self.surface.background,
            default_background(),
            &mut errors,
        );
        let text_foreground = match &self.text.foreground {
            Some(reference) => self.resolve_required(
                "theme.text.foreground",
                reference,
                default_foreground(),
                &mut errors,
            ),
            None => default_foreground(),
        };
        let default_style = DisplayStyle {
            foreground: text_foreground,
            background: self.text.background.as_deref().and_then(|reference| {
                self.resolve_optional("theme.text.background", reference, &mut errors)
            }),
            bold: self.text.bold,
            italic: self.text.italic,
            underline: self.text.underline,
            strikethrough: self.text.strikethrough,
            inset_y: self.text.inset_y.unwrap_or(0),
            radius: self.text.radius,
        };

        let mut styles = BTreeMap::new();
        for (name, style) in &self.styles {
            styles.insert(
                name.clone(),
                DisplayStyle {
                    foreground: style
                        .foreground
                        .as_deref()
                        .map_or(text_foreground, |reference| {
                            self.resolve_required(
                                &format!("theme.styles.{name}.foreground"),
                                reference,
                                text_foreground,
                                &mut errors,
                            )
                        }),
                    background: style.background.as_deref().and_then(|reference| {
                        self.resolve_optional(
                            &format!("theme.styles.{name}.background"),
                            reference,
                            &mut errors,
                        )
                    }),
                    bold: style.bold,
                    italic: style.italic,
                    underline: style.underline,
                    strikethrough: style.strikethrough,
                    inset_y: style.inset_y.unwrap_or(0),
                    radius: style.radius,
                },
            );
        }

        let theme = Theme {
            font: self.font,
            background,
            default_style,
            padding: self.surface.padding,
            radius: self.surface.radius,
            styles,
        };
        theme.validate(&mut errors);

        if errors.is_empty() {
            Ok(theme)
        } else {
            Err(errors)
        }
    }

    fn resolve_required(
        &self,
        label: &str,
        reference: &str,
        fallback: Rgba,
        errors: &mut Vec<String>,
    ) -> Rgba {
        match self.resolve_color(reference) {
            Some(color) => color,
            None => {
                errors.push(format!(
                    "{label}: {reference:?} is not a #RRGGBB/#RRGGBBAA color or a name in theme.colors"
                ));
                fallback
            }
        }
    }

    fn resolve_optional(
        &self,
        label: &str,
        reference: &str,
        errors: &mut Vec<String>,
    ) -> Option<Rgba> {
        match self.resolve_color(reference) {
            Some(color) => Some(color),
            None => {
                errors.push(format!(
                    "{label}: {reference:?} is not a #RRGGBB/#RRGGBBAA color or a name in theme.colors"
                ));
                None
            }
        }
    }

    fn resolve_color(&self, reference: &str) -> Option<Rgba> {
        self.colors
            .get(reference)
            .copied()
            .or_else(|| reference.parse().ok())
    }
}

pub(crate) fn load_theme(path: &Path) -> Result<Theme, ConfigError> {
    let text = read_utf8(path, |path, source| ConfigError::ThemeRead { path, source })?;
    let theme: ThemeConfig = toml::from_str(&text).map_err(|source| ConfigError::ThemeParse {
        path: path.to_owned(),
        source,
    })?;
    theme
        .resolve()
        .map_err(|errors| ConfigError::Validation(ValidationErrors(errors)))
}

pub(crate) fn default_background() -> Rgba {
    Rgba {
        red: 0x18,
        green: 0x1a,
        blue: 0x1f,
        alpha: 0xff,
    }
}

pub(crate) fn default_foreground() -> Rgba {
    Rgba {
        red: 0xd8,
        green: 0xde,
        blue: 0xe9,
        alpha: 0xff,
    }
}

pub(crate) fn default_surface_padding() -> u32 {
    8
}

pub(crate) fn default_font_families() -> Vec<FontFamilyEntry> {
    vec![FontFamilyEntry::Family("monospace".to_owned())]
}

pub(crate) fn default_font_size() -> f32 {
    12.0
}

/// Default line height as a multiple of `font.size`.
pub const DEFAULT_LINE_HEIGHT: f32 = 1.25;

pub(crate) fn default_line_height() -> f32 {
    DEFAULT_LINE_HEIGHT
}

pub fn cell_height(font_size: f32, line_height: f32) -> u32 {
    (font_size * line_height).ceil().max(1.0) as u32
}
