use std::{
    borrow::Cow,
    cell::RefCell,
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Child,
    rc::Rc,
    time::{Duration, Instant},
};

use serde_json::Value;
use smithay_client_toolkit::reexports::calloop::RegistrationToken;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::{
    cell_frame::text_cell_width,
    config::{ActionSpec, Config, DisplayAlign, Region, Trigger},
    expression,
    images::{ImageSpec, Part},
    interaction::InteractionEvent,
    markup::{MarkupPart, MarkupSpan, parse_markup},
};

pub const DEFAULT_TICK_DELAY: Duration = Duration::from_secs(60);
const PROCESS_POLL_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone)]
pub struct WidgetDefinition {
    pub id: String,
    pub region: Region,
    pub min_width: Option<usize>,
    pub align: DisplayAlign,
    pub max_width: Option<usize>,
    pub content: WidgetDefinitionContent,
    pub actions: Vec<(Option<String>, InteractionEvent, ActionSpec)>,
}

#[derive(Clone)]
pub enum WidgetDefinitionContent {
    Markup(Vec<MarkupSpan>),
    Image(ImageSpec),
    Provider(ProviderDefinition),
}

impl WidgetDefinitionContent {
    pub fn provider(&self) -> Option<&ProviderDefinition> {
        match self {
            Self::Provider(provider) => Some(provider),
            Self::Markup(_) | Self::Image(_) => None,
        }
    }
}

#[derive(Clone)]
pub enum ProviderDefinition {
    Command(ProcessDefinition),
    Expression(ExpressionDefinition),
}

#[derive(Clone)]
pub struct ProcessDefinition {
    pub id: String,
    pub command: Vec<String>,
    pub base: PathBuf,
    pub timeout: Option<Duration>,
    pub source_interval: Option<Duration>,
    pub on_activate: bool,
    pub every: Option<Duration>,
    pub debounce: Duration,
    pub triggers: Vec<Trigger>,
}

#[derive(Clone)]
pub struct ExpressionDefinition {
    pub id: String,
    pub ast: crate::expression::Ast,
    pub settings: Value,
    pub base: PathBuf,
    pub source_interval: Option<Duration>,
    pub on_activate: bool,
    pub every: Option<Duration>,
    pub debounce: Duration,
    pub triggers: Vec<Trigger>,
}

pub struct WidgetState {
    pub id: String,
    pub region: Region,
    pub min_width: Option<usize>,
    pub align: DisplayAlign,
    pub max_width: Option<usize>,
    pub content: WidgetContent,
    pub pushed_markup: Option<Vec<MarkupSpan>>,
}

impl ProviderDefinition {
    pub fn uses_output(&self) -> bool {
        match self {
            Self::Command(process) => process.command.iter().any(|arg| arg.contains("${output}")),
            Self::Expression(_) => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ProviderKey {
    pub widget_id: String,
    pub output: Option<String>,
}

#[derive(Clone)]
pub enum WidgetContent {
    Markup(Vec<MarkupSpan>),
    Image(ImageSpec),
    Provider {
        key: ProviderKey,
        state: Rc<RefCell<ProviderState>>,
    },
}

pub enum ProviderState {
    Command(ProcessState),
    Expression(ExpressionState),
}

pub struct ProcessState {
    pub definition: ProcessDefinition,
    pub value: String,
    pub rich: Option<Vec<MarkupSpan>>,
    pub child: Option<Child>,
    pub source: Option<RegistrationToken>,
    pub pending: Vec<u8>,
    pub discarding_oversized_line: bool,
    pub next_start: Option<Instant>,
    pub started_at: Option<Instant>,
    pub refresh_requested: bool,
    pub pending_event: Option<String>,
}

pub struct ExpressionState {
    pub definition: ExpressionDefinition,
    pub value: String,
    pub rich: Option<Vec<MarkupSpan>>,
    current_context: Value,
    state: BTreeMap<String, Value>,
    pending_events: Vec<(String, String)>,
    next_eval: Option<Instant>,
}

impl ProviderDefinition {
    pub fn instantiate(&self) -> ProviderState {
        match self {
            Self::Command(process) => ProviderState::Command(ProcessState {
                definition: process.clone(),
                value: String::new(),
                rich: None,
                child: None,
                source: None,
                pending: Vec::new(),
                discarding_oversized_line: false,
                next_start: (process.on_activate || process.every.is_some())
                    .then_some(Instant::now()),
                started_at: None,
                refresh_requested: false,
                pending_event: None,
            }),
            Self::Expression(expression) => ProviderState::Expression(ExpressionState {
                definition: expression.clone(),
                value: String::new(),
                rich: None,
                current_context: Value::Null,
                state: BTreeMap::new(),
                pending_events: Vec::new(),
                next_eval: (expression.on_activate || expression.every.is_some())
                    .then_some(Instant::now()),
            }),
        }
    }

    pub fn triggers(&self) -> &[Trigger] {
        match self {
            Self::Command(process) => &process.triggers,
            Self::Expression(expression) => &expression.triggers,
        }
    }
}

impl ProviderState {
    pub fn text(&self) -> &str {
        match self {
            Self::Command(process) => &process.value,
            Self::Expression(expression) => &expression.value,
        }
    }

    pub fn spans(&self) -> Vec<MarkupSpan> {
        match self {
            Self::Command(process) => process.rich.clone().unwrap_or_else(|| {
                vec![MarkupSpan {
                    part: MarkupPart::Text(process.value.clone()),
                    style: None,
                    max_width: None,
                    min_width: None,
                    align: None,
                    scope_style: None,
                    root_scope_id: None,
                    root_scope_style: None,
                    is_sub_scope: false,
                    target: None,
                }]
            }),
            Self::Expression(expression) => expression.rich.clone().unwrap_or_else(|| {
                vec![MarkupSpan {
                    part: MarkupPart::Text(expression.value.clone()),
                    style: None,
                    max_width: None,
                    min_width: None,
                    align: None,
                    scope_style: None,
                    root_scope_id: None,
                    root_scope_style: None,
                    is_sub_scope: false,
                    target: None,
                }]
            }),
        }
    }

    pub fn is_initialized(&self) -> bool {
        !self.text().is_empty()
            || match self {
                Self::Command(process) => process.rich.is_some(),
                Self::Expression(expression) => expression.rich.is_some(),
            }
    }

    pub fn triggers(&self) -> &[Trigger] {
        match self {
            Self::Command(process) => &process.definition.triggers,
            Self::Expression(expression) => &expression.definition.triggers,
        }
    }

    pub fn source_interval(&self) -> Option<Duration> {
        match self {
            Self::Command(process) => process.definition.source_interval,
            Self::Expression(expression) => expression.definition.source_interval,
        }
    }

    pub fn refresh(&mut self) -> bool {
        match self {
            Self::Command(_) => false,
            Self::Expression(expression) => expression.refresh(),
        }
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        match self {
            Self::Command(process) => {
                if process.child.is_some() {
                    let mut deadline = Instant::now().checked_add(PROCESS_POLL_INTERVAL);
                    if let (Some(timeout), Some(started)) =
                        (process.definition.timeout, process.started_at)
                    {
                        deadline = min_deadline(deadline, started.checked_add(timeout));
                    }
                    deadline
                } else {
                    process.next_start
                }
            }
            Self::Expression(expression) => expression.next_eval,
        }
    }

    pub fn schedule_event(&mut self, trigger_id: String, context: String, deadline: Instant) {
        match self {
            Self::Command(process) => {
                process.pending_event = Some(context);
                if process.child.is_some() {
                    process.refresh_requested = true;
                    process.next_start = Some(deadline);
                } else {
                    process.next_start = match process.next_start {
                        Some(start) if start <= deadline => Some(start),
                        _ => Some(deadline),
                    };
                }
            }
            Self::Expression(expression) => {
                if let Some(index) = expression
                    .pending_events
                    .iter()
                    .position(|(pending_trigger_id, _)| *pending_trigger_id == trigger_id)
                {
                    expression.pending_events.remove(index);
                }
                expression.pending_events.push((trigger_id, context));
                expression.next_eval = Some(match expression.next_eval {
                    Some(current) => current.min(deadline),
                    None => deadline,
                });
            }
        }
    }

    pub fn refresh_now(&mut self) -> bool {
        match self {
            Self::Command(process) => {
                process.pending_event = None;
                process.next_start = Some(Instant::now());
                if process.child.is_some() {
                    process.refresh_requested = true;
                }
                false
            }
            Self::Expression(expression) => expression.refresh_now(),
        }
    }

    pub fn expression_debounce(&self) -> Duration {
        match self {
            Self::Command(process) => process.definition.debounce,
            Self::Expression(expression) => expression.definition.debounce,
        }
    }
}

fn min_deadline(first: Option<Instant>, second: Option<Instant>) -> Option<Instant> {
    match (first, second) {
        (Some(first), Some(second)) => Some(first.min(second)),
        (first, second) => first.or(second),
    }
}

impl ExpressionState {
    pub fn refresh_now(&mut self) -> bool {
        self.next_eval = Some(Instant::now());
        self.refresh()
    }

    fn refresh(&mut self) -> bool {
        let now = Instant::now();
        if self.next_eval.is_none_or(|deadline| deadline > now) {
            return false;
        }

        for (trigger_id, context) in std::mem::take(&mut self.pending_events) {
            match serde_json::from_str::<Value>(&context) {
                Ok(context) => {
                    self.current_context = context.clone();
                    self.state.insert(trigger_id, context);
                }
                Err(error) => {
                    eprintln!(
                        "cellbar: expression provider {:?} received invalid context: {error}",
                        self.definition.id
                    );
                }
            }
        }

        let next = match expression::evaluate_parts(
            &self.definition.ast,
            &self.current_context,
            &self.state,
            &self.definition.settings,
            &self.definition.base,
        ) {
            Ok(value) => Some(value),
            Err(error) => {
                eprintln!(
                    "cellbar: expression provider {:?} failed: {error}",
                    self.definition.id
                );
                None
            }
        };
        self.next_eval = self
            .definition
            .every
            .and_then(|interval| now.checked_add(interval));

        let Some(next) = next else {
            return false;
        };
        let (text, rich) = match next.as_slice() {
            [Part::Text(text)] => match parse_markup(text, &self.definition.base) {
                Ok(spans) => (text.clone(), Some(spans)),
                Err(_) => (text.clone(), None),
            },
            _ => {
                let spans = next
                    .into_iter()
                    .map(|p| match p {
                        Part::Text(t) => MarkupSpan {
                            part: MarkupPart::Text(t),
                            style: None,
                            max_width: None,
                            min_width: None,
                            align: None,
                            scope_style: None,
                            root_scope_id: None,
                            root_scope_style: None,
                            is_sub_scope: false,
                            target: None,
                        },
                        Part::Image(img) => MarkupSpan {
                            part: MarkupPart::Image(img),
                            style: None,
                            max_width: None,
                            min_width: None,
                            align: None,
                            scope_style: None,
                            root_scope_id: None,
                            root_scope_style: None,
                            is_sub_scope: false,
                            target: None,
                        },
                    })
                    .collect::<Vec<_>>();
                (String::new(), Some(spans))
            }
        };
        if self.value == text && self.rich == rich {
            false
        } else {
            self.value = text;
            self.rich = rich;
            true
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WidgetKey {
    pub bar_id: u64,
    pub widget_index: usize,
}

impl WidgetDefinition {
    pub fn from_config(config: &Config) -> Vec<Self> {
        if let Some(first_id) = config.show.first()
            && let Some(bar) = config.bar_definitions.get(first_id)
        {
            return Self::from_bar_and_config(bar, config);
        }
        if let Some(bar) = config.bar_definitions.values().next() {
            return Self::from_bar_and_config(bar, config);
        }
        Vec::new()
    }

    pub fn from_bar_and_config(
        bar: &crate::config::BarConfig,
        config: &Config,
    ) -> Vec<Self> {
        let regions = [
            (Region::Left, &bar.left),
            (Region::Center, &bar.center),
            (Region::Right, &bar.right),
        ];
        let mut definitions = Vec::new();
        let config_dir = if config.config_dir.as_os_str().is_empty() {
            Path::new(".")
        } else {
            config.config_dir.as_path()
        };
        for (region, ids) in regions {
            for (index, id) in ids.iter().enumerate() {
                let Some(display) = config.displays.get(id) else {
                    definitions.push(Self {
                        id: format!("{region:?}-literal-{index}"),
                        region,
                        min_width: None,
                        align: DisplayAlign::Left,
                        max_width: None,
                        content: WidgetDefinitionContent::Markup(
                            parse_markup(id, config_dir).expect("validated layout literal markup"),
                        ),
                        actions: Vec::new(),
                    });
                    continue;
                };
                let mut merged_actions: BTreeMap<(Option<String>, InteractionEvent), ActionSpec> =
                    BTreeMap::new();
                let resolved_settings = display
                    .component
                    .as_ref()
                    .and_then(|provider_name| config.providers.get(provider_name))
                    .and_then(|manifest| manifest.settings_for(&display.settings).ok());
                if let Some(provider_name) = &display.component
                    && let Some(manifest) = config.providers.get(provider_name)
                {
                    for (event_name, spec) in &manifest.actions {
                        let (evt_key, target_pattern) =
                            if let Some((evt, pat)) = event_name.split_once(':') {
                                (evt, Some(pat.to_owned()))
                            } else {
                                (event_name.as_str(), None)
                            };
                        let event = match evt_key {
                            "click" | "left_click" => {
                                InteractionEvent::Click(crate::interaction::MouseButton::Left)
                            }
                            "right_click" => {
                                InteractionEvent::Click(crate::interaction::MouseButton::Right)
                            }
                            "middle_click" => {
                                InteractionEvent::Click(crate::interaction::MouseButton::Middle)
                            }
                            "scroll_up" => {
                                InteractionEvent::Scroll(crate::interaction::MouseAxis::ScrollUp)
                            }
                            "scroll_down" => {
                                InteractionEvent::Scroll(crate::interaction::MouseAxis::ScrollDown)
                            }
                            "scroll_left" => {
                                InteractionEvent::Scroll(crate::interaction::MouseAxis::ScrollLeft)
                            }
                            "scroll_right" => {
                                InteractionEvent::Scroll(crate::interaction::MouseAxis::ScrollRight)
                            }
                            _ => continue,
                        };
                        merged_actions.insert((target_pattern, event), spec.clone());
                    }
                }
                for (p, e, s) in display.resolved_actions() {
                    merged_actions.insert((p, e), s.clone());
                }
                let actions: Vec<(Option<String>, InteractionEvent, ActionSpec)> = merged_actions
                    .into_iter()
                    .map(|((p, e), mut s)| {
                        if let Some(settings) = &resolved_settings {
                            s.substitute_settings(settings);
                        }
                        (p, e, s)
                    })
                    .collect();
                let content = if let Some(text) = &display.text {
                    WidgetDefinitionContent::Markup(
                        parse_markup(text, config_dir).expect("validated display text markup"),
                    )
                } else if let Some(src) = &display.image {
                    let mut image = ImageSpec {
                        src: src.clone(),
                        width: display.width,
                        fit: display.fit,
                        shape: display.shape,
                        align: display.image_align,
                        fallback: display.fallback.clone(),
                    };
                    image.resolve(config_dir).expect("validated static image");
                    WidgetDefinitionContent::Image(image)
                } else {
                    let comp_name = display.component.as_deref().unwrap();
                    let base = {
                        let comp_path = config_dir.join("components").join(comp_name);
                        let parent_comp = config_dir.parent().map(|p| p.join("components").join(comp_name));

                        if comp_path.exists() {
                            comp_path
                        } else if let Some(p) = parent_comp && p.exists() {
                            p
                        } else {
                            comp_path
                        }
                    };
                    let provider = display
                        .component
                        .as_deref()
                        .expect("validated display component");
                    let manifest = config
                        .providers
                        .get(provider)
                        .expect("validated provider reference");
                    let settings = manifest
                        .settings_for(&display.settings)
                        .expect("validated provider settings");
                    let settings = serde_json::to_value(toml::Value::Table(settings))
                        .expect("provider settings are serializable");
                    if manifest.program.is_some() {
                        let command = manifest
                            .command_for(&display.settings, true)
                            .expect("validated provider command");
                        WidgetDefinitionContent::Provider(ProviderDefinition::Command(
                            ProcessDefinition {
                                id: id.clone(),
                                command,
                                base,
                                timeout: manifest.timeout.map(|duration| duration.0),
                                source_interval: provider_interval(
                                    &display.settings,
                                    &manifest.settings,
                                ),
                                on_activate: manifest.on_activate,
                                every: manifest.every,
                                debounce: manifest.debounce,
                                triggers: manifest.triggers.clone(),
                            },
                        ))
                    } else {
                        let expression = manifest
                            .expression
                            .as_deref()
                            .expect("validated provider expression");
                        WidgetDefinitionContent::Provider(ProviderDefinition::Expression(
                            ExpressionDefinition {
                                id: id.clone(),
                                ast: expression::compile(expression)
                                    .expect("validated provider expression"),
                                settings,
                                base,
                                source_interval: provider_interval(
                                    &display.settings,
                                    &manifest.settings,
                                ),
                                on_activate: manifest.on_activate,
                                every: manifest.every,
                                debounce: manifest.debounce,
                                triggers: manifest.triggers.clone(),
                            },
                        ))
                    }
                };
                definitions.push(Self {
                    id: id.clone(),
                    region,
                    min_width: display.min_width,
                    align: display.align,
                    max_width: display.max_width,
                    content,
                    actions,
                });
            }
        }
        definitions
    }
}

fn provider_interval(
    display_settings: &toml::Table,
    provider_settings: &BTreeMap<String, toml::Value>,
) -> Option<Duration> {
    display_settings
        .get("interval")
        .or_else(|| {
            provider_settings
                .get("interval")
        })
        .and_then(toml::Value::as_str)
        .and_then(|value| humantime::parse_duration(value).ok())
        .filter(|duration| !duration.is_zero())
}

impl WidgetState {
    pub fn instantiate(definitions: &[WidgetDefinition]) -> Vec<Self> {
        Self::instantiate_for_output(definitions, None)
    }

    pub fn instantiate_for_output(
        definitions: &[WidgetDefinition],
        output: Option<&str>,
    ) -> Vec<Self> {
        definitions
            .iter()
            .map(|definition| {
                let content = match &definition.content {
                    WidgetDefinitionContent::Markup(spans) => WidgetContent::Markup(spans.clone()),
                    WidgetDefinitionContent::Image(image) => WidgetContent::Image(image.clone()),
                    WidgetDefinitionContent::Provider(provider) => {
                        let uses_output = provider.uses_output();
                        let key = ProviderKey {
                            widget_id: definition.id.clone(),
                            output: if uses_output {
                                output.map(|s| s.to_owned())
                            } else {
                                None
                            },
                        };
                        WidgetContent::Provider {
                            key,
                            state: Rc::new(RefCell::new(provider.instantiate())),
                        }
                    }
                };
                Self {
                    id: definition.id.clone(),
                    region: definition.region,
                    min_width: definition.min_width,
                    align: definition.align,
                    max_width: definition.max_width,
                    content,
                    pushed_markup: None,
                }
            })
            .collect()
    }

    pub fn spans(&self) -> Vec<MarkupSpan> {
        if let Some(pushed) = &self.pushed_markup {
            return pushed.clone();
        }
        match &self.content {
            WidgetContent::Markup(spans) => spans.clone(),
            WidgetContent::Image(image) => vec![MarkupSpan {
                part: MarkupPart::Image(image.clone()),
                style: None,
                max_width: None,
                min_width: None,
                align: None,
                scope_style: None,
                root_scope_id: None,
                root_scope_style: None,
                is_sub_scope: false,
                target: None,
            }],
            WidgetContent::Provider { state, .. } => state.borrow().spans(),
        }
    }

    pub fn text(&self) -> String {
        let text = if let Some(pushed) = &self.pushed_markup {
            spans_text(pushed)
        } else {
            match &self.content {
                WidgetContent::Markup(spans) => spans_text(spans),
                WidgetContent::Image(_) => String::new(),
                WidgetContent::Provider { state, .. } => state.borrow().text().to_owned(),
            }
        };
        let text = match self.max_width {
            Some(max_width) => truncate_to_width(&text, max_width).into_owned(),
            None => text,
        };
        match self.min_width {
            Some(min_width) => pad_to_width(&text, min_width, self.align),
            None => text,
        }
    }

    pub fn refresh_provider(&mut self) -> bool {
        let WidgetContent::Provider { state, .. } = &mut self.content else {
            return false;
        };
        let changed = state.borrow_mut().refresh();
        if changed {
            self.pushed_markup = None;
        }
        changed
    }
}

pub fn decode_process_spans(line: &[u8], base: &Path) -> Result<Vec<MarkupSpan>, String> {
    let text = std::str::from_utf8(line).map_err(|e| e.to_string())?;
    parse_markup(text.trim_end_matches(['\r', '\n']), base)
}

/// Concatenates the plain text of markup spans, ignoring image segments.
pub fn spans_text(spans: &[MarkupSpan]) -> String {
    spans
        .iter()
        .filter_map(|span| match &span.part {
            MarkupPart::Text(text) => Some(text.as_str()),
            MarkupPart::Image(_) => None,
        })
        .collect()
}

pub const TRUNCATION_ELLIPSIS: &str = "…";

pub fn pad_to_width(text: &str, min_width: usize, align: DisplayAlign) -> String {
    let current_width = text_cell_width(text);
    if current_width >= min_width {
        return text.to_owned();
    }

    let remaining = min_width - current_width;
    let (leading, trailing) = match align {
        DisplayAlign::Left => (0, remaining),
        DisplayAlign::Center => (remaining / 2, remaining - remaining / 2),
        DisplayAlign::Right => (remaining, 0),
    };
    let mut output = String::with_capacity(text.len() + remaining);
    output.extend(std::iter::repeat_n(' ', leading));
    output.push_str(text);
    output.extend(std::iter::repeat_n(' ', trailing));
    output
}

pub fn truncate_to_width(text: &str, max_width: usize) -> Cow<'_, str> {
    if max_width == 0 || text.is_empty() {
        return Cow::Borrowed("");
    }

    let total_width = text_cell_width(text);
    if total_width <= max_width {
        return Cow::Borrowed(text);
    }

    let ellipsis_width = TRUNCATION_ELLIPSIS.width();
    if max_width <= ellipsis_width {
        return Cow::Borrowed(TRUNCATION_ELLIPSIS);
    }

    let target_width = max_width.saturating_sub(ellipsis_width);
    let mut current_width: usize = 0;
    let mut output = String::with_capacity(text.len());

    for grapheme in text.graphemes(true) {
        let grapheme_width = crate::cell_frame::grapheme_cell_width(grapheme);
        if current_width.saturating_add(grapheme_width) > target_width {
            break;
        }
        output.push_str(grapheme);
        current_width += grapheme_width;
    }

    output.push_str(TRUNCATION_ELLIPSIS);
    Cow::Owned(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::images::{ImageFit, ImageShape};

    fn text_markup(text: &str) -> WidgetDefinitionContent {
        WidgetDefinitionContent::Markup(vec![MarkupSpan {
            part: MarkupPart::Text(text.to_owned()),
            style: None,
            max_width: None,
            min_width: None,
            align: None,
            scope_style: None,
            root_scope_id: None,
            root_scope_style: None,
            is_sub_scope: false,
            target: None,
        }])
    }

    #[test]
    fn decodes_image_segments_and_markup() {
        let spans = decode_process_spans(
            b"![cover.png](3 circle contain)",
            Path::new("/tmp/provider"),
        )
        .unwrap();
        assert_eq!(spans.len(), 1);
        match &spans[0].part {
            MarkupPart::Image(img) => {
                assert_eq!(img.src, Path::new("/tmp/provider/cover.png"));
                assert_eq!(img.width, 3);
                assert_eq!(img.shape, ImageShape::Circle);
                assert_eq!(img.fit, ImageFit::Contain);
            }
            _ => panic!("expected image"),
        }
    }

    #[test]
    fn decodes_command_provider_output() {
        let spans = decode_process_spans(b"[CPU 12%]", Path::new(".")).unwrap();
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].part, MarkupPart::Text("CPU 12%".into()));
    }

    #[test]
    fn preserves_markup_style_and_target_metadata() {
        let spans = decode_process_spans(b"#btn{ [Click Me](@accent) }", Path::new(".")).unwrap();
        assert_eq!(spans[0].target, Some("btn".into()));
        assert_eq!(spans[0].style, Some("accent".into()));
        assert_eq!(spans[0].part, MarkupPart::Text("Click Me".into()));
    }

    #[test]
    fn truncates_ascii_and_cjk_to_terminal_cell_width() {
        assert_eq!(truncate_to_width("hello", 10), "hello");
        assert_eq!(truncate_to_width("hello world", 8), "hello w…");
        assert_eq!(truncate_to_width("hello world", 1), "…");
        assert_eq!(truncate_to_width("hello world", 0), "");
        assert_eq!(truncate_to_width("", 10), "");
        assert_eq!(truncate_to_width("一二三", 6), "一二三");
        assert_eq!(truncate_to_width("一二三", 5), "一二…");
        assert_eq!(truncate_to_width("一二三", 4), "一…");

        let full = " マサラダ - ㋰責任集合体";
        let truncated = truncate_to_width(full, 16);
        assert!(truncated.ends_with('…'));
        assert!(truncated.width() <= 16);
    }

    #[test]
    fn expression_provider_keeps_pending_context_for_each_trigger() {
        let definition = ExpressionDefinition {
            id: "combined".to_owned(),
            ast: expression::compile(
                "context.data.value.to_string() + \" \" + state.cpu.data.value.to_string() + \"/\" + state.memory.data.value.to_string()",
            )
            .expect("valid expression"),
            settings: serde_json::Value::Null,
            base: PathBuf::from("."),
            source_interval: None,
            on_activate: false,
            every: None,
            debounce: Duration::from_secs(1),
            triggers: Vec::new(),
        };
        let mut provider = ProviderState::Expression(ExpressionState {
            definition,
            value: String::new(),
            rich: None,
            current_context: Value::Null,
            state: BTreeMap::new(),
            pending_events: Vec::new(),
            next_eval: None,
        });
        let deadline = Instant::now();
        provider.schedule_event(
            "cpu".to_owned(),
            r#"{"data":{"value":"42"}}"#.to_owned(),
            deadline,
        );
        provider.schedule_event(
            "memory".to_owned(),
            r#"{"data":{"value":"8G"}}"#.to_owned(),
            deadline,
        );
        provider.schedule_event(
            "cpu".to_owned(),
            r#"{"data":{"value":"43"}}"#.to_owned(),
            deadline,
        );

        assert!(provider.refresh());
        assert_eq!(provider.text(), "43 43/8G");
    }

    #[test]
    fn pads_to_terminal_cell_width() {
        assert_eq!(pad_to_width("CPU", 5, DisplayAlign::Left), "CPU  ");
        assert_eq!(pad_to_width("CPU", 5, DisplayAlign::Center), " CPU ");
        assert_eq!(pad_to_width("CPU", 5, DisplayAlign::Right), "  CPU");
        assert_eq!(pad_to_width("一", 4, DisplayAlign::Right), "  一");
        assert_eq!(pad_to_width("hello", 4, DisplayAlign::Center), "hello");
    }

    #[test]
    fn widget_state_applies_width_constraints() {
        let definition = WidgetDefinition {
            id: "title".to_owned(),
            region: Region::Left,
            min_width: Some(10),
            align: DisplayAlign::Left,
            max_width: Some(10),
            content: text_markup("Very Long Text Content"),
            actions: Vec::new(),
        };
        let states = WidgetState::instantiate(&[definition]);
        assert_eq!(states[0].text(), "Very Long…");

        let padded = WidgetDefinition {
            id: "title2".to_owned(),
            region: Region::Left,
            min_width: Some(10),
            align: DisplayAlign::Right,
            max_width: None,
            content: text_markup("CPU"),
            actions: Vec::new(),
        };
        let states2 = WidgetState::instantiate(&[padded]);
        assert_eq!(states2[0].text(), "       CPU");

        let unconstrained = WidgetDefinition {
            id: "title3".to_owned(),
            region: Region::Left,
            min_width: None,
            align: DisplayAlign::Left,
            max_width: None,
            content: text_markup("Very Long Text Content"),
            actions: Vec::new(),
        };
        let states2 = WidgetState::instantiate(&[unconstrained]);
        assert_eq!(states2[0].text(), "Very Long Text Content");
    }

    #[test]
    fn instantiates_inline_literals_as_text_widgets() {
        let toml = r##"
[bar.main]
theme = "theme.toml"
left = ["[hello]", "[ | ]", "[  ]"]
"##;
        let config = crate::config::Config::parse(toml).expect("valid config with literals");
        let definitions = WidgetDefinition::from_config(&config);
        assert_eq!(definitions.len(), 3);
        assert!(
            matches!(&definitions[0].content, WidgetDefinitionContent::Markup(spans) if spans_text(spans) == "hello")
        );
        assert!(
            matches!(&definitions[1].content, WidgetDefinitionContent::Markup(spans) if spans_text(spans) == " | ")
        );
        assert!(
            matches!(&definitions[2].content, WidgetDefinitionContent::Markup(spans) if spans_text(spans) == "  ")
        );

        let widgets = WidgetState::instantiate(&definitions);
        assert_eq!(widgets[0].text(), "hello");
        assert_eq!(widgets[1].text(), " | ");
        assert_eq!(widgets[2].text(), "  ");
    }

    #[test]
    fn provider_manifest_actions_inherited_and_overridden() {
        let toml = r##"
[bar.main]
theme = "theme.toml"
left = ["media"]
"##;
        let mut config: crate::config::Config = toml::from_str(toml).expect("valid config");
        let mut provider_actions = std::collections::BTreeMap::new();
        provider_actions.insert(
            "click:prev".into(),
            ActionSpec::Command("default_prev".into()),
        );
        provider_actions.insert(
            "click:next".into(),
            ActionSpec::Command("default_next".into()),
        );
        provider_actions.insert(
            "scroll_up".into(),
            ActionSpec::Command("default_vol_up".into()),
        );

        config.providers.insert(
            "media".into(),
            crate::config::ProviderConfig {
                program: Some("run.sh".into()),
                expression: None,
                args: vec![],
                timeout: None,
                triggers: vec![],
                on_activate: true,
                every: None,
                debounce: std::time::Duration::from_millis(50),
                settings: std::collections::BTreeMap::new(),
                actions: provider_actions,
            },
        );

        let definitions = WidgetDefinition::from_config(&config);
        assert_eq!(definitions.len(), 1);
        let media = &definitions[0];
        assert_eq!(media.actions.len(), 3);
    }

    #[test]
    fn provider_actions_substitute_settings() {
        let toml = r##"
[bar.main]
theme = "theme.toml"
left = ["volume"]
"##;
        let mut config: crate::config::Config = toml::from_str(toml).expect("valid config");
        let mut provider_actions = std::collections::BTreeMap::new();
        provider_actions.insert(
            "scroll_up".into(),
            ActionSpec::Full(crate::config::ActionConfig {
                command: "wpctl".into(),
                args: vec!["set-volume".into(), "${setting.device}".into(), "5%+".into()],
                refresh: None,
                debounce: None,
            }),
        );
        let mut settings = std::collections::BTreeMap::new();
        settings.insert(
            "device".into(),
            toml::Value::String("alsa_output.pci".into()),
        );

        config.providers.insert(
            "volume".into(),
            crate::config::ProviderConfig {
                program: Some("run.sh".into()),
                expression: None,
                args: vec![],
                timeout: None,
                triggers: vec![],
                on_activate: true,
                every: None,
                debounce: std::time::Duration::from_millis(50),
                settings,
                actions: provider_actions,
            },
        );

        let definitions = WidgetDefinition::from_config(&config);
        assert_eq!(definitions.len(), 1);
        let vol = &definitions[0];
        let scroll_up = &vol.actions[0].2;
        assert_eq!(
            scroll_up,
            &ActionSpec::Full(crate::config::ActionConfig {
                command: "wpctl".into(),
                args: vec!["set-volume".into(), "alsa_output.pci".into(), "5%+".into()],
                refresh: None,
                debounce: None,
            })
        );
    }
}
