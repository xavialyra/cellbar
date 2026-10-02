use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
    time::Duration,
};

use serde::Deserialize;

use crate::{
    config::{
        ActionSpec, ConfigError, DEFAULT_EVENT_DEBOUNCE, HumanDuration, read_utf8,
        resolve_action_spec, resolve_program, setting_value, valid_name,
    },
    events::system,
};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderManifest {
    pub provider: ProviderConfig,
    #[serde(default)]
    pub triggers: TriggerConfig,
    #[serde(default)]
    pub settings: BTreeMap<String, toml::Value>,
    #[serde(default)]
    pub actions: BTreeMap<String, ActionSpec>,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfig {
    #[serde(default)]
    pub program: Option<String>,
    #[serde(default)]
    pub expression: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub timeout: Option<HumanDuration>,
    #[serde(skip)]
    pub triggers: Vec<Trigger>,
    #[serde(skip)]
    pub on_activate: bool,
    #[serde(skip)]
    pub every: Option<Duration>,
    #[serde(skip)]
    pub debounce: Duration,
    #[serde(skip)]
    pub settings: BTreeMap<String, toml::Value>,
    #[serde(skip)]
    pub actions: BTreeMap<String, ActionSpec>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerConfig {
    #[serde(default)]
    pub on_activate: bool,
    #[serde(default)]
    pub every: Option<HumanDuration>,
    #[serde(default)]
    pub debounce: Option<HumanDuration>,
    #[serde(default, rename = "on_dbus")]
    pub dbus: Vec<DbusTrigger>,
    #[serde(default, rename = "on_netlink")]
    pub netlink: Vec<NetlinkTrigger>,
    #[serde(default, rename = "on_uevent")]
    pub uevent: Vec<UeventTrigger>,
    #[serde(default, rename = "on_pipewire")]
    pub pipewire: Vec<PipewireTrigger>,
    #[serde(default, rename = "on_wayland")]
    pub wayland: Vec<WaylandTrigger>,
    #[serde(default, rename = "on_source")]
    pub source: Vec<SourceTrigger>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DbusTrigger {
    pub id: String,
    pub bus: EventBus,
    pub match_rule: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetlinkTrigger {
    pub id: String,
    pub family: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UeventTrigger {
    pub id: String,
    #[serde(default)]
    pub subsystem: Option<String>,
    #[serde(default)]
    pub action: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PipewireTrigger {
    pub id: String,
    pub event: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WaylandTrigger {
    pub id: String,
    pub event: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceTrigger {
    pub id: String,
    pub source: String,
    pub event: String,
}

#[derive(Debug, Clone)]
pub struct Trigger {
    pub id: String,
    pub kind: EventSourceKind,
    pub event: String,
    pub bus: Option<EventBus>,
    pub match_rule: Option<String>,
    pub subsystem: Option<String>,
    pub action: Option<String>,
    pub source: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventSourceKind {
    Dbus,
    Netlink,
    Uevent,
    Pipewire,
    Wayland,
    Source,
}

pub type SubscriptionBus = EventBus;
pub type SubscriptionType = EventSourceKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EventBus {
    Session,
    System,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalSourceManifest {
    pub source: ExternalSourceConfig,
    #[serde(default)]
    pub emits: Vec<SourceEventSpec>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalSourceConfig {
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub restart_after: HumanDuration,
    #[serde(default)]
    pub scope: SourceScope,
    #[serde(skip)]
    pub emits: HashSet<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceEventSpec {
    pub event: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceScope {
    #[default]
    Global,
    Output,
}

impl ProviderConfig {
    pub(crate) fn from_manifest(manifest: ProviderManifest) -> Self {
        Self {
            program: manifest.provider.program,
            expression: manifest.provider.expression,
            args: manifest.provider.args,
            timeout: manifest.provider.timeout,
            triggers: manifest.triggers.normalize(),
            on_activate: manifest.triggers.on_activate,
            every: manifest.triggers.every.map(|duration| duration.0),
            debounce: manifest
                .triggers
                .debounce
                .map(|duration| duration.0)
                .unwrap_or(DEFAULT_EVENT_DEBOUNCE),
            settings: manifest.settings,
            actions: manifest.actions,
        }
    }

    pub(crate) fn validate_definition(
        &self,
        errors: &mut Vec<String>,
        id: &str,
        sources: &BTreeMap<String, ExternalSourceConfig>,
    ) {
        let label = format!("provider {id:?}");
        match (&self.program, &self.expression) {
            (Some(program), None) => {
                if program.trim().is_empty() {
                    errors.push(format!("{label}: program must not be empty"));
                }
                if self.timeout.is_some_and(|duration| duration.0.is_zero()) {
                    errors.push(format!("{label}: timeout must be greater than zero"));
                }
                if let Err(error) = self.validate_command_placeholders() {
                    errors.push(format!("{label}: {error}"));
                }
            }
            (None, Some(expression)) => {
                if expression.trim().is_empty() {
                    errors.push(format!("{label}: expression must not be empty"));
                } else if let Err(error) = crate::expression::compile(expression) {
                    errors.push(format!("{label}: invalid expression: {error}"));
                }
                if !self.args.is_empty() {
                    errors.push(format!("{label}: expression providers cannot define args"));
                }
                if self.timeout.is_some() {
                    errors.push(format!(
                        "{label}: expression providers cannot define timeout"
                    ));
                }
            }
            (Some(_), Some(_)) => errors.push(format!(
                "{label}: program and expression are mutually exclusive"
            )),
            (None, None) => errors.push(format!(
                "{label}: exactly one of program or expression is required"
            )),
        }
        if self.every.is_some_and(|duration| duration.is_zero()) {
            errors.push(format!("{label}: triggers.every must be greater than zero"));
        }
        if self.debounce.is_zero() {
            errors.push(format!(
                "{label}: triggers.debounce must be greater than zero"
            ));
        }
        if !self.on_activate && self.every.is_none() && self.triggers.is_empty() {
            errors.push(format!(
                "{label}: requires triggers.on_activate, triggers.every, or an on_* trigger"
            ));
        }
        validate_setting_specs(errors, &label, &self.settings);
        let mut trigger_ids = HashSet::new();
        for trigger in &self.triggers {
            if !trigger_ids.insert(&trigger.id) {
                errors.push(format!(
                    "{label}: trigger id {:?} is duplicated",
                    trigger.id
                ));
            }
            if !valid_name(&trigger.id) {
                errors.push(format!(
                    "{label}: trigger id {:?} must use ASCII letters, digits, '-' or '_'",
                    trigger.id
                ));
            }
            match trigger.kind {
                EventSourceKind::Dbus => {
                    if let Some(rule) = &trigger.match_rule
                        && !validate_dbus_match_rule(rule)
                    {
                        errors.push(format!(
                            "{label}: trigger {:?} has invalid D-Bus match: {rule:?}",
                            trigger.id
                        ));
                    }
                }
                EventSourceKind::Netlink if trigger.event != "route" => errors.push(format!(
                    "{label}: trigger {:?} supports only netlink family \"route\"",
                    trigger.id
                )),
                EventSourceKind::Pipewire
                    if ![
                        "volume",
                        "default-sink",
                        "sink-volume",
                        "default-source",
                        "source-volume",
                    ]
                    .contains(&trigger.event.as_str()) =>
                {
                    errors.push(format!(
                        "{label}: trigger {:?} has unsupported PipeWire event {:?}",
                        trigger.id, trigger.event
                    ))
                }
                EventSourceKind::Wayland
                    if !["toplevel", "workspace"].contains(&trigger.event.as_str()) =>
                {
                    errors.push(format!(
                        "{label}: trigger {:?} has unsupported Wayland event {:?}",
                        trigger.id, trigger.event
                    ))
                }
                EventSourceKind::Source => {
                    let source = trigger.source.as_deref().unwrap_or_default();
                    if system::is_builtin_source(source) {
                        if !system::supports_event(source, &trigger.event) {
                            errors.push(format!(
                                "{label}: builtin source {source:?} does not support event {:?}",
                                trigger.event
                            ));
                        }
                    } else {
                        match sources.get(source) {
                            Some(source_config)
                                if !source_config.emits.contains(&trigger.event) =>
                            {
                                errors.push(format!(
                                    "{label}: source {source:?} does not declare event {:?}",
                                    trigger.event
                                ))
                            }
                            Some(_) => {}
                            None => errors.push(format!("{label}: unknown source {source:?}")),
                        }
                    }
                }
                _ => {}
            }
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

        for (event_name, spec) in &self.actions {
            let evt_key = event_name
                .split_once(':')
                .map(|(e, _)| e)
                .unwrap_or(event_name.as_str());
            match evt_key {
                "click" | "left_click" | "right_click" | "middle_click" | "scroll_up"
                | "scroll_down" | "scroll_left" | "scroll_right" => {
                    validate_action_spec(errors, event_name, spec);
                }
                _ => {
                    errors.push(format!("{label}: unknown action event {event_name:?}"));
                }
            }
        }
    }

    fn validate_command_placeholders(&self) -> Result<(), String> {
        let mut values = Vec::new();
        if let Some(program) = &self.program {
            values.push(program.as_str());
        }
        values.extend(self.args.iter().map(String::as_str));
        for argument in values {
            for placeholder in placeholders(argument) {
                if placeholder == "context" || placeholder == "output" {
                    continue;
                }
                let Some(name) = placeholder.strip_prefix("setting.") else {
                    return Err(format!("unknown placeholder ${{{placeholder}}}"));
                };
                if !self.settings.contains_key(name) {
                    return Err(format!("unknown provider setting {name:?}"));
                }
            }
        }
        Ok(())
    }

    pub fn command_for(
        &self,
        settings: &toml::Table,
        preserve_context: bool,
    ) -> Result<Vec<String>, String> {
        let program = self
            .program
            .as_deref()
            .ok_or_else(|| "expression provider has no program".to_owned())?;
        let values = self.settings_for(settings)?;
        let mut command = Vec::with_capacity(self.args.len() + 1);
        command.push(substitute_placeholders(program, &values, preserve_context)?);
        for argument in &self.args {
            command.push(substitute_placeholders(
                argument,
                &values,
                preserve_context,
            )?);
        }
        Ok(command)
    }

    pub(crate) fn validate_settings(
        &self,
        errors: &mut Vec<String>,
        label: &str,
        settings: &toml::Table,
    ) {
        for name in settings.keys() {
            if !self.settings.contains_key(name) {
                errors.push(format!("{label}: unknown provider setting {name:?}"));
            }
        }
    }

    pub fn settings_for(&self, settings: &toml::Table) -> Result<toml::Table, String> {
        let mut values: toml::Table = self.settings.clone().into_iter().collect();
        for (name, value) in settings {
            if !self.settings.contains_key(name) {
                return Err(format!("unknown provider setting {name:?}"));
            }
            values.insert(name.clone(), value.clone());
        }
        Ok(values)
    }

    pub fn source_ids(&self) -> impl Iterator<Item = String> + '_ {
        self.triggers
            .iter()
            .filter_map(|trigger| trigger.source.clone())
    }
}

impl TriggerConfig {
    pub(crate) fn normalize(&self) -> Vec<Trigger> {
        let mut triggers = Vec::new();
        triggers.extend(self.dbus.iter().map(|trigger| Trigger {
            id: trigger.id.clone(),
            kind: EventSourceKind::Dbus,
            event: "signal".to_owned(),
            bus: Some(trigger.bus),
            match_rule: Some(trigger.match_rule.clone()),
            subsystem: None,
            action: None,
            source: None,
        }));
        triggers.extend(self.netlink.iter().map(|trigger| Trigger {
            id: trigger.id.clone(),
            kind: EventSourceKind::Netlink,
            event: trigger.family.clone(),
            bus: None,
            match_rule: None,
            subsystem: None,
            action: None,
            source: None,
        }));
        triggers.extend(self.uevent.iter().map(|trigger| Trigger {
            id: trigger.id.clone(),
            kind: EventSourceKind::Uevent,
            event: "change".to_owned(),
            bus: None,
            match_rule: None,
            subsystem: trigger.subsystem.clone(),
            action: trigger.action.clone(),
            source: None,
        }));
        triggers.extend(self.pipewire.iter().map(|trigger| Trigger {
            id: trigger.id.clone(),
            kind: EventSourceKind::Pipewire,
            event: trigger.event.clone(),
            bus: None,
            match_rule: None,
            subsystem: None,
            action: None,
            source: None,
        }));
        triggers.extend(self.wayland.iter().map(|trigger| Trigger {
            id: trigger.id.clone(),
            kind: EventSourceKind::Wayland,
            event: trigger.event.clone(),
            bus: None,
            match_rule: None,
            subsystem: None,
            action: None,
            source: None,
        }));
        triggers.extend(self.source.iter().map(|trigger| Trigger {
            id: trigger.id.clone(),
            kind: EventSourceKind::Source,
            event: trigger.event.clone(),
            bus: None,
            match_rule: None,
            subsystem: None,
            action: None,
            source: Some(trigger.source.clone()),
        }));
        triggers
    }
}

impl ExternalSourceConfig {
    pub(crate) fn from_manifest(manifest: ExternalSourceManifest) -> Self {
        Self {
            program: manifest.source.program,
            args: manifest.source.args,
            restart_after: manifest.source.restart_after,
            scope: manifest.source.scope,
            emits: manifest
                .emits
                .into_iter()
                .map(|event| event.event)
                .collect(),
        }
    }

    pub(crate) fn validate_definition(&self, errors: &mut Vec<String>, id: &str) {
        let label = format!("source {id:?}");
        if self.program.is_empty() {
            errors.push(format!("{label}: program must not be empty"));
        }
        if self.restart_after.0.is_zero() {
            errors.push(format!("{label}: restart_after must be greater than zero"));
        }
        if self.emits.is_empty() {
            errors.push(format!("{label}: must declare at least one emitted event"));
        }
        for event in &self.emits {
            if !valid_name(event) {
                errors.push(format!(
                    "{label}: emitted event {event:?} must use ASCII letters, digits, '-' or '_'"
                ));
            }
        }
        if self.args.iter().any(|arg| !placeholders(arg).is_empty())
            || !placeholders(&self.program).is_empty()
        {
            errors.push(format!(
                "{label}: program and args cannot contain placeholders"
            ));
        }
    }
}

pub(crate) fn load_provider(config_dir: &Path, id: &str) -> Result<ProviderConfig, ConfigError> {
    let candidates = [
        config_dir.join("components").join(id).join("manifest.toml"),
        config_dir
            .parent()
            .map(|p| p.join("components").join(id).join("manifest.toml"))
            .unwrap_or_default(),
    ];
    let path = candidates
        .into_iter()
        .find(|p| !p.as_os_str().is_empty() && p.exists())
        .unwrap_or_else(|| config_dir.join("components").join(id).join("manifest.toml"));
    let text = read_utf8(&path, |path, source| ConfigError::ProviderRead {
        path,
        source,
    })?;
    let mut manifest: ProviderManifest =
        toml::from_str(&text).map_err(|source| ConfigError::ProviderParse {
            path: path.clone(),
            source,
        })?;
    if let Some(program) = &mut manifest.provider.program {
        resolve_program(&path, program);
    }
    for action in manifest.actions.values_mut() {
        resolve_action_spec(&path, action);
    }
    Ok(ProviderConfig::from_manifest(manifest))
}

pub(crate) fn load_source(
    config_dir: &Path,
    id: &str,
) -> Result<ExternalSourceConfig, ConfigError> {
    let path = config_dir.join("sources").join(id).join("manifest.toml");
    let text = read_utf8(&path, |path, source| ConfigError::SourceRead {
        path,
        source,
    })?;
    let mut manifest: ExternalSourceManifest =
        toml::from_str(&text).map_err(|source| ConfigError::SourceParse {
            path: path.clone(),
            source,
        })?;
    resolve_program(&path, &mut manifest.source.program);
    Ok(ExternalSourceConfig::from_manifest(manifest))
}

pub(crate) fn validate_setting_specs(
    errors: &mut Vec<String>,
    label: &str,
    settings: &BTreeMap<String, toml::Value>,
) {
    for name in settings.keys() {
        if !valid_name(name) {
            errors.push(format!(
                "{label}: setting {name:?} must use ASCII letters, digits, '-' or '_'"
            ));
        }
    }
}

pub(crate) fn placeholders(argument: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = argument;
    while let Some(start) = rest.find("${") {
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else { break };
        names.push(after[..end].to_owned());
        rest = &after[end + 1..];
    }
    names
}

pub(crate) fn substitute_placeholders(
    argument: &str,
    settings: &toml::Table,
    preserve_context: bool,
) -> Result<String, String> {
    let mut output = String::with_capacity(argument.len());
    let mut rest = argument;
    while let Some(start) = rest.find("${") {
        output.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            return Err(format!("unterminated placeholder in {argument:?}"));
        };
        let name = &after[..end];
        if name == "context" {
            if preserve_context {
                output.push_str("${context}");
            } else {
                output.push_str("null");
            }
        } else if name == "output" {
            output.push_str("${output}");
        } else if let Some(setting) = name.strip_prefix("setting.") {
            let value = settings
                .get(setting)
                .ok_or_else(|| format!("setting {setting:?} is missing"))?;
            output.push_str(&setting_value(value));
        } else {
            return Err(format!("unknown placeholder ${{{name}}}"));
        }
        rest = &after[end + 1..];
    }
    output.push_str(rest);
    Ok(output)
}

pub(crate) fn substitute_context_placeholder(argument: &str, context: &str) -> String {
    argument.replace("${context}", context)
}

fn validate_dbus_match_rule(rule: &str) -> bool {
    let trimmed = rule.trim();
    if trimmed.is_empty() {
        return false;
    }
    // Match rules are comma-separated key='value' or key="value" pairs
    for item in trimmed.split(',') {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        let Some((key, val)) = item.split_once('=') else {
            return false;
        };
        let key = key.trim();
        let val = val.trim();
        let is_arg_key = if let Some(suffix) = key.strip_prefix("arg") {
            let num_part = suffix
                .strip_suffix("namespace")
                .or_else(|| suffix.strip_suffix("path"))
                .unwrap_or(suffix);
            num_part.parse::<u32>().is_ok()
        } else {
            false
        };
        let valid_key = matches!(
            key,
            "type"
                | "sender"
                | "interface"
                | "member"
                | "path"
                | "path_namespace"
                | "destination"
                | "eavesdrop"
        ) || is_arg_key;
        if !valid_key {
            return false;
        }
        let is_quoted = (val.starts_with('\'') && val.ends_with('\'') && val.len() >= 2)
            || (val.starts_with('"') && val.ends_with('"') && val.len() >= 2);
        if !is_quoted
            && !val
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
        {
            return false;
        }
    }
    true
}
