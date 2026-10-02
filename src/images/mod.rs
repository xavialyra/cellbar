pub(crate) mod store;
pub use store::ImageStore;

use std::path::{Path, PathBuf};

use serde::Deserialize;

pub const MAX_IMAGE_COLUMNS: usize = 256;
pub const MAX_PARTS: usize = 256;
const MAX_TEXT_BYTES: usize = 256 * 1024;

pub fn default_image_width() -> usize {
    2
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageFit {
    #[default]
    TextMatch,
    Contain,
    Cover,
    Stretch,
    ScaleDown,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageAlign {
    Left,
    #[default]
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageShape {
    #[default]
    Rect,
    Circle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageSpec {
    pub src: PathBuf,
    pub width: usize,
    pub fit: ImageFit,
    pub shape: ImageShape,
    pub align: ImageAlign,
    pub fallback: String,
}

impl ImageSpec {
    pub fn resolve(&mut self, base: &Path) -> Result<(), String> {
        if self.src.as_os_str().is_empty() || self.src.to_string_lossy().contains("://") {
            return Err("image src must be a local file path".into());
        }
        if self.src.as_os_str().len() > 4096 {
            return Err("image path exceeds 4096 bytes".into());
        }
        if self.width == 0 || self.width > MAX_IMAGE_COLUMNS {
            return Err(format!(
                "image width must be in 1..={MAX_IMAGE_COLUMNS} cells"
            ));
        }
        if self.fallback.len() > 1024 {
            return Err("image fallback exceeds 1024 bytes".into());
        }
        if self.src.is_relative() {
            self.src = base.join(&self.src);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Part {
    Text(String),
    Image(ImageSpec),
}

pub fn validate_parts(parts: &mut [Part], base: &Path) -> Result<(), String> {
    if parts.len() > MAX_PARTS {
        return Err(format!("image output exceeds {MAX_PARTS} parts"));
    }
    let mut text_bytes = 0usize;
    for part in parts {
        match part {
            Part::Text(text) => {
                text_bytes = text_bytes
                    .checked_add(text.len())
                    .ok_or("image output text size overflow")?;
                if text.len() > 64 * 1024 {
                    return Err("text part exceeds 64 KiB".into());
                }
            }
            Part::Image(image) => {
                image.resolve(base)?;
                text_bytes = text_bytes
                    .checked_add(image.fallback.len())
                    .ok_or("image output text size overflow")?;
            }
        }
        if text_bytes > MAX_TEXT_BYTES {
            return Err("image output text exceeds 256 KiB".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_rich_output_with_too_much_total_text() {
        let mut parts = vec![Part::Text("x".repeat(1025)); MAX_PARTS];
        let error = validate_parts(&mut parts, Path::new(".")).expect_err("total text limit");
        assert_eq!(error, "image output text exceeds 256 KiB");
    }
}
