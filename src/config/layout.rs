use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};

use serde::Deserialize;

use crate::{
    config::{HumanDuration, resolve_program, setting_value},
    images::{ImageAlign, ImageFit, ImageShape, default_image_width},
    interaction::{InteractionEvent, MouseAxis, MouseButton},
};

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum BarPosition {
    #[default]
    Top,
    Bottom,
}

impl BarPosition {
    pub fn as_str(&self) -> &'static str {
        match self {
            BarPosition::Top => "top",
            BarPosition::Bottom => "bottom",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum BarLayer {
    Background,
    Bottom,
    #[default]
    Top,
    Overlay,
}

impl BarLayer {
    pub fn as_str(&self) -> &'static str {
        match self {
            BarLayer::Background => "background",
            BarLayer::Bottom => "bottom",
            BarLayer::Top => "top",
            BarLayer::Overlay => "overlay",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BarConfig {
    #[serde(default)]
    pub height: Option<u32>,
    #[serde(rename = "theme")]
    pub theme_path: PathBuf,
    #[serde(skip)]
    pub theme: Option<crate::config::Theme>,
    #[serde(default)]
    pub outputs: Vec<String>,
    #[serde(default)]
    pub position: BarPosition,
    #[serde(default)]
    pub margin: BarMargins,
    #[serde(default)]
    pub left: Vec<String>,
    #[serde(default)]
    pub center: Vec<String>,
    #[serde(default)]
    pub right: Vec<String>,
}

impl Default for BarConfig {
    fn default() -> Self {
        Self {
            height: None,
            theme_path: PathBuf::new(),
            theme: None,
            outputs: Vec::new(),
            position: BarPosition::Top,
            margin: BarMargins::default(),
            left: Vec::new(),
            center: Vec::new(),
            right: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BarMargins {
    #[serde(default)]
    pub top: u32,
    #[serde(default)]
    pub right: u32,
    #[serde(default)]
    pub bottom: u32,
    #[serde(default)]
    pub left: u32,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayoutConfig {
    #[serde(default)]
    pub left: Vec<String>,
    #[serde(default)]
    pub center: Vec<String>,
    #[serde(default)]
    pub right: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Region {
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DisplayAlign {
    #[default]
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum ActionSpec {
    Command(String),
    Full(ActionConfig),
}

impl ActionSpec {
    pub fn to_command_args(&self) -> (Vec<String>, Option<String>, Option<Duration>) {
        match self {
            Self::Command(cmd) => {
                let parts = vec!["sh".to_owned(), "-c".to_owned(), cmd.clone()];
                (parts, None, None)
            }
            Self::Full(cfg) => {
                let mut parts = vec![cfg.command.clone()];
                parts.extend(cfg.args.iter().cloned());
                (parts, cfg.refresh.clone(), cfg.debounce.map(|d| d.0))
            }
        }
    }

    pub fn substitute_settings(&mut self, settings: &toml::Table) {
        for (name, value) in settings {
            let placeholder = format!("${{setting.{name}}}");
            let rendered = setting_value(value);
            match self {
                Self::Command(cmd) => {
                    if cmd.contains(&placeholder) {
                        *cmd = cmd.replace(&placeholder, &rendered);
                    }
                }
                Self::Full(cfg) => {
                    if cfg.command.contains(&placeholder) {
                        cfg.command = cfg.command.replace(&placeholder, &rendered);
                    }
                    for arg in &mut cfg.args {
                        if arg.contains(&placeholder) {
                            *arg = arg.replace(&placeholder, &rendered);
                        }
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub refresh: Option<String>,
    #[serde(default)]
    pub debounce: Option<HumanDuration>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ComponentConfig {
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub image: Option<PathBuf>,
    #[serde(default = "default_image_width")]
    pub width: usize,
    #[serde(default)]
    pub fit: ImageFit,
    #[serde(default)]
    pub shape: ImageShape,
    #[serde(default)]
    pub image_align: ImageAlign,
    #[serde(default)]
    pub fallback: String,
    #[serde(default, alias = "type", alias = "provider")]
    pub component: Option<String>,
    #[serde(default)]
    pub min_width: Option<usize>,
    #[serde(default)]
    pub align: DisplayAlign,
    #[serde(default)]
    pub max_width: Option<usize>,
    #[serde(default)]
    pub settings: toml::Table,
    #[serde(default)]
    pub on_click: Option<ActionSpec>,
    #[serde(default)]
    pub on_right_click: Option<ActionSpec>,
    #[serde(default)]
    pub on_middle_click: Option<ActionSpec>,
    #[serde(default)]
    pub on_scroll_up: Option<ActionSpec>,
    #[serde(default)]
    pub on_scroll_down: Option<ActionSpec>,
    #[serde(default)]
    pub on_scroll_left: Option<ActionSpec>,
    #[serde(default)]
    pub on_scroll_right: Option<ActionSpec>,
    #[serde(default)]
    pub actions: BTreeMap<String, ActionSpec>,
    #[serde(flatten)]
    pub extra_settings: toml::Table,
}

pub type DisplayConfig = ComponentConfig;

impl ComponentConfig {
    pub fn merge_flattened_settings(&mut self) {
        for (k, v) in std::mem::take(&mut self.extra_settings) {
            self.settings.entry(k).or_insert(v);
        }
    }

    pub fn resolved_actions(&self) -> Vec<(Option<String>, InteractionEvent, &ActionSpec)> {
        let mut list = Vec::new();
        if let Some(spec) = &self.on_click {
            list.push((None, InteractionEvent::Click(MouseButton::Left), spec));
        }
        if let Some(spec) = &self.on_right_click {
            list.push((None, InteractionEvent::Click(MouseButton::Right), spec));
        }
        if let Some(spec) = &self.on_middle_click {
            list.push((None, InteractionEvent::Click(MouseButton::Middle), spec));
        }
        if let Some(spec) = &self.on_scroll_up {
            list.push((None, InteractionEvent::Scroll(MouseAxis::ScrollUp), spec));
        }
        if let Some(spec) = &self.on_scroll_down {
            list.push((None, InteractionEvent::Scroll(MouseAxis::ScrollDown), spec));
        }
        if let Some(spec) = &self.on_scroll_left {
            list.push((None, InteractionEvent::Scroll(MouseAxis::ScrollLeft), spec));
        }
        if let Some(spec) = &self.on_scroll_right {
            list.push((None, InteractionEvent::Scroll(MouseAxis::ScrollRight), spec));
        }
        for (event_name, spec) in &self.actions {
            let (evt_key, target_pattern) = if let Some((evt, pat)) = event_name.split_once(':') {
                (evt, Some(pat.to_owned()))
            } else {
                (event_name.as_str(), None)
            };
            let event = match evt_key {
                "click" | "left_click" => InteractionEvent::Click(MouseButton::Left),
                "right_click" => InteractionEvent::Click(MouseButton::Right),
                "middle_click" => InteractionEvent::Click(MouseButton::Middle),
                "scroll_up" => InteractionEvent::Scroll(MouseAxis::ScrollUp),
                "scroll_down" => InteractionEvent::Scroll(MouseAxis::ScrollDown),
                "scroll_left" => InteractionEvent::Scroll(MouseAxis::ScrollLeft),
                "scroll_right" => InteractionEvent::Scroll(MouseAxis::ScrollRight),
                _ => continue,
            };
            list.push((target_pattern, event, spec));
        }
        list
    }
}

pub(crate) fn resolve_action_spec(manifest_path: &Path, action: &mut ActionSpec) {
    let base_dir = manifest_path.parent().unwrap_or(Path::new("."));
    let provider_dir_str = base_dir.to_string_lossy();
    match action {
        ActionSpec::Command(cmd) => {
            if cmd.contains("${provider_dir}") {
                *cmd = cmd.replace("${provider_dir}", &provider_dir_str);
            }
            if cmd.starts_with("./") || cmd.starts_with("../") {
                if let Some((first, rest)) = cmd.split_once(char::is_whitespace) {
                    let mut resolved = first.to_owned();
                    resolve_program(manifest_path, &mut resolved);
                    *cmd = format!("{resolved} {rest}");
                } else {
                    resolve_program(manifest_path, cmd);
                }
            }
        }
        ActionSpec::Full(cfg) => {
            if cfg.command.contains("${provider_dir}") {
                cfg.command = cfg.command.replace("${provider_dir}", &provider_dir_str);
            }
            resolve_program(manifest_path, &mut cfg.command);
            for arg in &mut cfg.args {
                if arg.contains("${provider_dir}") {
                    *arg = arg.replace("${provider_dir}", &provider_dir_str);
                }
            }
        }
    }
}
