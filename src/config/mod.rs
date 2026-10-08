pub mod layout;
pub mod provider;
pub mod theme;

#[cfg(test)]
mod tests;

pub use layout::{
    ActionConfig, ActionSpec, BarConfig, BarLayer, BarMargins, BarPosition, ComponentConfig,
    DisplayAlign, DisplayConfig, LayoutConfig, Region,
};
pub(crate) use provider::substitute_context_placeholder;
pub use provider::{
    DbusTrigger, EventBus, EventSourceKind, ExternalSourceConfig, ExternalSourceManifest,
    NetlinkTrigger, PipewireTrigger, ProviderConfig, ProviderManifest, SourceEventSpec,
    SourceScope, SourceTrigger, SubscriptionBus, SubscriptionType, Trigger, TriggerConfig,
    UeventTrigger, WaylandTrigger,
};
pub use theme::{
    ColorParseError, DEFAULT_LINE_HEIGHT, DisplayStyle, FontConfig, FontFamilyEntry, Rgba,
    StyleConfig, SurfaceConfig, SurfacePadding, Theme, ThemeConfig, cell_height,
};

use std::{
    collections::{BTreeMap, HashSet},
    env,
    ffi::OsString,
    fmt, fs,
    path::{Path, PathBuf},
    time::Duration,
};

use serde::{Deserialize, Deserializer};
use thiserror::Error;

use crate::{events::system, images::ImageSpec};

use layout::resolve_action_spec;
use provider::{load_provider, load_source};
use theme::load_theme;

pub const MAX_CONFIG_BYTES: usize = 1024 * 1024;
pub const DEFAULT_EVENT_DEBOUNCE: Duration = Duration::ZERO;

#[derive(Debug, Clone, Default)]
pub struct Config {
    pub show: Vec<String>,
    pub raw_show: Option<Vec<String>>,
    pub bar_definitions: BTreeMap<String, BarConfig>,
    pub components: BTreeMap<String, ComponentConfig>,
    pub displays: BTreeMap<String, ComponentConfig>,
    pub providers: BTreeMap<String, ProviderConfig>,
    pub sources: BTreeMap<String, ExternalSourceConfig>,
    pub config_dir: PathBuf,
    pub loaded_themes: BTreeMap<PathBuf, Theme>,
    pub theme: Theme,
    pub bar: BarConfig,
}

impl<'de> Deserialize<'de> for Config {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let root = toml::Value::deserialize(deserializer)?;
        let mut table = match root {
            toml::Value::Table(t) => t,
            _ => {
                return Err(serde::de::Error::custom(
                    "root configuration must be a TOML table",
                ));
            }
        };

        let raw_show: Option<Vec<String>> = if let Some(v) = table.remove("show") {
            Some(v.try_into().map_err(serde::de::Error::custom)?)
        } else {
            None
        };

        let bar_definitions: BTreeMap<String, BarConfig> = if let Some(v) = table.remove("bar") {
            v.try_into().map_err(serde::de::Error::custom)?
        } else {
            BTreeMap::new()
        };

        let mut components: BTreeMap<String, ComponentConfig> = BTreeMap::new();

        // Under route 1 (pure layout architecture), components are configured directly in their component manifests.
        if table.contains_key("component") {
            return Err(serde::de::Error::custom(
                "components are configured directly in components/<name>/manifest.toml; config.toml only defines bar layouts",
            ));
        }

        if table.contains_key("display") {
            return Err(serde::de::Error::custom(
                "[display] is removed; components are configured directly in components/<name>/manifest.toml",
            ));
        }

        // Strict top-level namespace: any remaining fields are rejected as unknown fields.
        const THEME_RESERVED_TABLES: &[&str] = &["font", "colors", "surface", "styles"];
        if let Some((id, _)) = table.into_iter().next() {
            if THEME_RESERVED_TABLES.contains(&id.as_str()) {
                return Err(serde::de::Error::custom(format!(
                    "unknown field `{id}`: visual settings must belong to the theme file"
                )));
            } else {
                return Err(serde::de::Error::custom(format!("unknown field `{id}`")));
            }
        }

        // Automatically populate components for all referenced non-literal widget IDs from bar layouts
        for bar in bar_definitions.values() {
            for entry in [&bar.left, &bar.center, &bar.right].into_iter().flatten() {
                if valid_name(entry) && !components.contains_key(entry) {
                    components.insert(
                        entry.clone(),
                        ComponentConfig {
                            component: Some(entry.clone()),
                            ..Default::default()
                        },
                    );
                }
            }
        }

        let show = raw_show
            .clone()
            .unwrap_or_else(|| bar_definitions.keys().cloned().collect());
        let displays = components.clone();

        Ok(Config {
            show,
            raw_show,
            bar_definitions,
            components,
            displays,
            providers: BTreeMap::new(),
            sources: BTreeMap::new(),
            config_dir: PathBuf::new(),
            loaded_themes: BTreeMap::new(),
            theme: Theme::default(),
            bar: BarConfig::default(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HumanDuration(pub Duration);

impl<'de> Deserialize<'de> for HumanDuration {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        humantime::parse_duration(&value)
            .map(Self)
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("cannot read configuration {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot read theme {path}: {source}")]
    ThemeRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid theme {path}: {source}")]
    ThemeParse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("cannot read provider manifest {path}: {source}")]
    ProviderRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid provider manifest {path}: {source}")]
    ProviderParse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("cannot read source manifest {path}: {source}")]
    SourceRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid source manifest {path}: {source}")]
    SourceParse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("configuration is too large: {size} bytes (maximum {MAX_CONFIG_BYTES})")]
    TooLarge { size: usize },
    #[error("invalid TOML: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("configuration validation failed:\n{0}")]
    Validation(ValidationErrors),
    #[error("cannot determine the default configuration path: set XDG_CONFIG_HOME or HOME")]
    MissingConfigDirectory,
}

impl ConfigError {
    pub fn display_with_path(&self, main_path: &Path) -> String {
        match self {
            ConfigError::Read { source, .. } => {
                format!(
                    "{}: cannot read configuration: {source}",
                    main_path.display()
                )
            }
            ConfigError::Parse(err) => {
                format!("{}: invalid TOML: {err}", main_path.display())
            }
            ConfigError::Validation(err) => {
                format!(
                    "{}: configuration validation failed:\n{err}",
                    main_path.display()
                )
            }
            other => format!("{}: {other}", main_path.display()),
        }
    }
}

#[derive(Debug)]
pub struct ValidationErrors(pub(crate) Vec<String>);

impl fmt::Display for ValidationErrors {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, message) in self.0.iter().enumerate() {
            if index > 0 {
                writeln!(formatter)?;
            }
            write!(formatter, "- {message}")?;
        }
        Ok(())
    }
}

impl Config {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let abs_path = normalize_path(path.as_ref());
        let text = read_utf8(&abs_path, |path, source| ConfigError::Read { path, source })?;
        let mut config: Self = toml::from_str(&text)?;
        let directory = abs_path.parent().unwrap_or(Path::new("."));
        config.config_dir = directory.to_path_buf();
        if config.bar_definitions.is_empty() {
            return Err(ConfigError::Validation(ValidationErrors(vec![
                "at least one [bar.<name>] must be defined".to_owned(),
            ])));
        }

        // If show list is not specified in config (None), default to all defined bars in sorted order.
        // If show list is explicitly specified (e.g. show = [] or show = ["main"]), use that list.
        config.show = config
            .raw_show
            .take()
            .unwrap_or_else(|| config.bar_definitions.keys().cloned().collect());

        for bar_id in &config.show {
            if !config.bar_definitions.contains_key(bar_id) {
                return Err(ConfigError::Validation(ValidationErrors(vec![format!(
                    "show lists undefined bar {bar_id:?}"
                )])));
            }
        }

        // Load and cache themes for all defined bars
        for (bar_id, bar) in &mut config.bar_definitions {
            if bar.theme_path.as_os_str().is_empty() {
                return Err(ConfigError::Validation(ValidationErrors(vec![format!(
                    "bar.{bar_id}: theme must name a theme file"
                )])));
            }
            let resolved_theme_path = resolve_path(directory, &bar.theme_path);
            if !config.loaded_themes.contains_key(&resolved_theme_path) {
                let theme = load_theme(&resolved_theme_path)?;
                config
                    .loaded_themes
                    .insert(resolved_theme_path.clone(), theme);
            }
            bar.theme = Some(config.loaded_themes[&resolved_theme_path].clone());
            bar.theme_path = resolved_theme_path;
        }

        let primary_bar_id = config
            .show
            .first()
            .or_else(|| config.bar_definitions.keys().next());
        if let Some(first_id) = primary_bar_id
            && let Some(first_bar) = config.bar_definitions.get(first_id)
        {
            config.bar = first_bar.clone();
            if let Some(first_theme) = &first_bar.theme {
                config.theme = first_theme.clone();
            }
        }

        let provider_ids = config.provider_ids();
        for id in provider_ids {
            let provider = load_provider(directory, &id)?;
            config.providers.insert(id, provider);
        }
        let source_ids = config
            .providers
            .values()
            .flat_map(|provider| provider.source_ids())
            .filter(|id| !system::is_builtin_source(id))
            .collect::<HashSet<_>>();
        for id in source_ids {
            let source = load_source(directory, &id)?;
            config.sources.insert(id, source);
        }
        for comp in config.components.values_mut() {
            if let Some(action) = &mut comp.on_click {
                resolve_action_spec(&abs_path, action);
            }
            if let Some(action) = &mut comp.on_right_click {
                resolve_action_spec(&abs_path, action);
            }
            if let Some(action) = &mut comp.on_middle_click {
                resolve_action_spec(&abs_path, action);
            }
            if let Some(action) = &mut comp.on_scroll_up {
                resolve_action_spec(&abs_path, action);
            }
            if let Some(action) = &mut comp.on_scroll_down {
                resolve_action_spec(&abs_path, action);
            }
            if let Some(action) = &mut comp.on_scroll_left {
                resolve_action_spec(&abs_path, action);
            }
            if let Some(action) = &mut comp.on_scroll_right {
                resolve_action_spec(&abs_path, action);
            }
            for action in comp.actions.values_mut() {
                resolve_action_spec(&abs_path, action);
            }
        }
        config.displays = config.components.clone();
        config.validate(true)?;
        Ok(config)
    }

    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        if text.len() > MAX_CONFIG_BYTES {
            return Err(ConfigError::TooLarge { size: text.len() });
        }
        let mut config: Self = toml::from_str(text)?;
        config.show = config
            .raw_show
            .take()
            .unwrap_or_else(|| config.bar_definitions.keys().cloned().collect());
        let primary_bar_id = config
            .show
            .first()
            .or_else(|| config.bar_definitions.keys().next());
        if let Some(first_id) = primary_bar_id
            && let Some(first_bar) = config.bar_definitions.get(first_id)
        {
            config.bar = first_bar.clone();
        }
        config.validate(false)?;
        Ok(config)
    }

    pub fn referenced_display_ids(&self) -> HashSet<&str> {
        let mut referenced = HashSet::new();
        for bar in self.bar_definitions.values() {
            for displays in [&bar.left, &bar.center, &bar.right] {
                for entry in displays {
                    if !self.is_literal_entry(entry) {
                        referenced.insert(entry.as_str());
                    }
                }
            }
        }
        referenced
    }

    fn provider_ids(&self) -> HashSet<String> {
        let referenced = self.referenced_display_ids();
        self.components
            .iter()
            .filter(|(id, _)| referenced.contains(id.as_str()))
            .filter_map(|(_, comp)| comp.component.as_deref())
            .map(str::to_owned)
            .collect()
    }

    pub fn is_literal_entry(&self, entry: &str) -> bool {
        !self.components.contains_key(entry) && !valid_name(entry)
    }

    pub fn has_literals(&self) -> bool {
        self.bar_definitions
            .values()
            .flat_map(|bar| [&bar.left, &bar.center, &bar.right])
            .flatten()
            .any(|entry| self.is_literal_entry(entry))
    }

    pub fn font_style_requirements(&self) -> (bool, bool) {
        self.theme
            .styles
            .values()
            .fold((false, false), |(bold, italic), style| {
                (bold || style.bold, italic || style.italic)
            })
    }

    fn validate(&self, validate_theme: bool) -> Result<(), ConfigError> {
        let mut errors = Vec::new();

        if self.bar_definitions.is_empty() {
            errors.push("at least one [bar.<name>] must be defined".to_owned());
        }

        for bar_id in &self.show {
            if !self.bar_definitions.contains_key(bar_id) {
                errors.push(format!("show lists undefined bar {bar_id:?}"));
            }
        }

        for (bar_id, bar) in &self.bar_definitions {
            let bar_label = format!("bar {bar_id:?}");
            if !valid_name(bar_id) {
                errors.push(format!(
                    "{bar_label}: id must use 1-64 ASCII letters, digits, '-' or '_'"
                ));
            }
            if bar.theme_path.as_os_str().is_empty() {
                errors.push(format!("{bar_label}: theme must name a theme file"));
            }
            if validate_theme && let Some(theme) = &bar.theme {
                theme.validate(&mut errors);
            }
            if let Some(height) = bar.height {
                if height == 0 {
                    errors.push(format!("{bar_label}.height must be greater than zero"));
                } else if height > 512 {
                    errors.push(format!(
                        "{bar_label}.height must be at most 512 logical pixels"
                    ));
                }
            }
            for (name, value) in [
                ("top", bar.margin.top),
                ("right", bar.margin.right),
                ("bottom", bar.margin.bottom),
                ("left", bar.margin.left),
            ] {
                if value > 1024 {
                    errors.push(format!(
                        "{bar_label}.margin.{name} must be at most 1024 logical pixels"
                    ));
                }
            }

            let mut bar_referenced = HashSet::new();
            for (region, displays) in [
                ("left", &bar.left),
                ("center", &bar.center),
                ("right", &bar.right),
            ] {
                for entry in displays {
                    if self.is_literal_entry(entry) {
                        if entry.is_empty() {
                            errors.push(format!("{bar_label}.{region} literal must not be empty"));
                        }
                        if entry.len() > 1024 {
                            errors.push(format!(
                                "{bar_label}.{region} literal must be at most 1024 bytes"
                            ));
                        }
                        if let Err(error) = crate::markup::parse_markup(entry, &self.config_dir) {
                            errors.push(format!(
                                "{bar_label}.{region} literal must be valid markup: {error}"
                            ));
                        }
                        continue;
                    }
                    let label = format!("{bar_label}.{region} display {entry:?}");
                    if !valid_name(entry) {
                        errors.push(format!(
                            "{label}: id must use 1-64 ASCII letters, digits, '-' or '_'"
                        ));
                    }
                    if !bar_referenced.insert(entry.as_str()) {
                        errors.push(format!("{label}: display is listed more than once"));
                    }
                    if !self.displays.contains_key(entry) {
                        errors.push(format!("{label}: display is not defined"));
                    }
                }
            }
        }

        let referenced = self.referenced_display_ids();

        for (id, display) in &self.displays {
            let label = format!("display {id:?}");
            if !valid_name(id) {
                errors.push(format!(
                    "{label}: id must use 1-64 ASCII letters, digits, '-' or '_'"
                ));
            }
            if display.min_width == Some(0) {
                errors.push(format!("{label}: min_width must be greater than zero"));
            }
            if display.max_width == Some(0) {
                errors.push(format!("{label}: max_width must be greater than zero"));
            }
            if let (Some(min_width), Some(max_width)) = (display.min_width, display.max_width)
                && min_width > max_width
            {
                errors.push(format!("{label}: min_width must not exceed max_width"));
            }
            match (&display.text, &display.component, &display.image) {
                (Some(text), None, None) if display.settings.is_empty() => {
                    if !text.is_empty()
                        && let Err(error) = crate::markup::parse_markup(text, &self.config_dir)
                    {
                        errors.push(format!("{label}: text must be valid markup: {error}"));
                    }
                }
                (Some(_), None, None) => {
                    errors.push(format!("{label}: text displays cannot define settings"))
                }
                (None, Some(provider), None) => {
                    if referenced.contains(id.as_str()) {
                        self.validate_provider_use(
                            &mut errors,
                            &label,
                            provider,
                            &display.settings,
                        );
                    }
                }
                (None, None, Some(src)) if display.settings.is_empty() => {
                    let mut image = ImageSpec {
                        src: src.clone(),
                        width: display.width,
                        fit: display.fit,
                        shape: display.shape,
                        align: display.image_align,
                        fallback: display.fallback.clone(),
                    };
                    if let Err(error) = image.resolve(Path::new(".")) {
                        errors.push(format!("{label}: {error}"));
                    }
                }
                (None, None, Some(_)) => {
                    errors.push(format!("{label}: image displays cannot define settings"))
                }
                (None, None, None) => errors.push(format!(
                    "{label}: exactly one of text, image or component is required"
                )),
                _ => errors.push(format!(
                    "{label}: text, image and component are mutually exclusive"
                )),
            }

            let validate_action_spec =
                |errors: &mut Vec<String>, event_name: &str, spec: &ActionSpec| match spec {
                    ActionSpec::Command(cmd) => {
                        if cmd.trim().is_empty() {
                            errors.push(format!(
                                "{label}: action {event_name:?} command must not be empty"
                            ));
                        }
                    }
                    ActionSpec::Full(cfg) => {
                        if cfg.command.trim().is_empty() {
                            errors.push(format!(
                                "{label}: action {event_name:?} command must not be empty"
                            ));
                        }
                    }
                };

            for (event_name, spec) in &display.actions {
                let evt_key = event_name
                    .split_once(':')
                    .map(|(e, _)| e)
                    .unwrap_or(event_name.as_str());
                match evt_key {
                    "click" | "left_click" | "right_click" | "middle_click" | "scroll_up"
                    | "scroll_down" | "scroll_left" | "scroll_right" => {
                        validate_action_spec(&mut errors, event_name, spec);
                    }
                    _ => {
                        errors.push(format!("{label}: unknown action event {event_name:?}"));
                    }
                }
            }
            if let Some(spec) = &display.on_click {
                validate_action_spec(&mut errors, "on_click", spec);
            }
            if let Some(spec) = &display.on_right_click {
                validate_action_spec(&mut errors, "on_right_click", spec);
            }
            if let Some(spec) = &display.on_middle_click {
                validate_action_spec(&mut errors, "on_middle_click", spec);
            }
            if let Some(spec) = &display.on_scroll_up {
                validate_action_spec(&mut errors, "on_scroll_up", spec);
            }
            if let Some(spec) = &display.on_scroll_down {
                validate_action_spec(&mut errors, "on_scroll_down", spec);
            }
            if let Some(spec) = &display.on_scroll_left {
                validate_action_spec(&mut errors, "on_scroll_left", spec);
            }
            if let Some(spec) = &display.on_scroll_right {
                validate_action_spec(&mut errors, "on_scroll_right", spec);
            }
        }

        for (id, provider) in &self.providers {
            provider.validate_definition(&mut errors, id, &self.sources);
        }
        for (id, source) in &self.sources {
            source.validate_definition(&mut errors, id);
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(ConfigError::Validation(ValidationErrors(errors)))
        }
    }

    fn validate_provider_use(
        &self,
        errors: &mut Vec<String>,
        label: &str,
        provider: &str,
        settings: &toml::Table,
    ) {
        match self.providers.get(provider) {
            Some(manifest) => manifest.validate_settings(errors, label, settings),
            None => errors.push(format!("{label}: unknown provider {provider:?}")),
        }
    }
}

pub(crate) fn read_utf8(
    path: &Path,
    error: impl FnOnce(PathBuf, std::io::Error) -> ConfigError,
) -> Result<String, ConfigError> {
    let bytes = fs::read(path).map_err(|source| error(path.to_path_buf(), source))?;
    if bytes.len() > MAX_CONFIG_BYTES {
        return Err(ConfigError::TooLarge { size: bytes.len() });
    }
    String::from_utf8(bytes).map_err(|error| {
        ConfigError::Validation(ValidationErrors(vec![format!(
            "{} is not valid UTF-8: {error}",
            path.display()
        )]))
    })
}

pub fn normalize_path(path: &Path) -> PathBuf {
    use std::path::Component;
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    let mut normalized = PathBuf::new();
    for component in abs.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            c => normalized.push(c.as_os_str()),
        }
    }
    normalized
}

pub(crate) fn resolve_path(directory: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_owned()
    } else {
        directory.join(path)
    }
}

pub(crate) fn resolve_program(manifest_path: &Path, program: &mut String) {
    if !Path::new(program).is_absolute() && (program.contains('/') || program.starts_with('.')) {
        *program = manifest_path
            .parent()
            .unwrap_or(Path::new("."))
            .join(&*program)
            .to_string_lossy()
            .into_owned();
    }
}

pub(crate) fn setting_value(value: &toml::Value) -> String {
    match value {
        toml::Value::String(value) => value.clone(),
        toml::Value::Integer(value) => value.to_string(),
        toml::Value::Float(value) => value.to_string(),
        toml::Value::Boolean(value) => value.to_string(),
        toml::Value::Datetime(value) => value.to_string(),
        toml::Value::Array(_) | toml::Value::Table(_) => value.to_string(),
    }
}

pub(crate) fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

pub fn default_path() -> Result<PathBuf, ConfigError> {
    let config_dir = nonempty_env("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| nonempty_env("HOME").map(|h| PathBuf::from(h).join(".config")));

    if let Some(base) = config_dir {
        return Ok(base.join("cellbar/config.toml"));
    }
    Err(ConfigError::MissingConfigDirectory)
}

fn nonempty_env(name: &str) -> Option<OsString> {
    env::var_os(name).filter(|value| !value.is_empty())
}
