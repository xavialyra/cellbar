mod bar;
mod control;
pub mod model;
mod process;
mod sources;
mod wayland;

#[cfg(test)]
mod tests;

use bar::Bar;
use control::bind_control_socket;
use model::*;
use sources::{BuiltinSourceState, SourceKey, SourceState, SubscriptionEntry};

use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    io::ErrorKind,
    path::PathBuf,
    rc::Rc,
    time::{Duration, Instant},
};

use cursor_icon::CursorIcon;
use smithay_client_toolkit::{
    compositor::CompositorState,
    output::OutputState,
    reexports::{
        calloop::{
            EventLoop, Interest, LoopHandle, Mode, PostAction, RegistrationToken,
            channel::{self, Event as ChannelEvent},
            generic::Generic,
            signals::{Signal, Signals},
            timer::{TimeoutAction, Timer},
        },
        calloop_wayland_source::WaylandSource,
    },
    registry::RegistryState,
    seat::{
        SeatState,
        pointer::{ThemeSpec, ThemedPointer},
    },
    shell::{WaylandSurface, wlr_layer::LayerShell},
    shm::Shm,
};
use thiserror::Error;
use wayland_client::{
    Connection, QueueHandle,
    globals::{GlobalList, registry_queue_init},
};

use crate::events::wayland::{ToplevelTracker, WorkspaceTracker};
use crate::events::{dbus, pipewire};
use crate::{
    config::{BarLayer, BarMargins, BarPosition, Config, ExternalSourceConfig, Theme},
    control as cellbar_control,
    images::ImageStore,
    interaction::InteractionRegistry,
    render::{FontStyleRequirements, TextRenderer},
};

pub(crate) const MAX_TICK_DELAY: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub struct BarSpec {
    pub name: Option<String>,
    pub height: Option<u32>,
    pub outputs: Vec<String>,
    pub position: BarPosition,
    pub layer: BarLayer,
    pub margin: BarMargins,
    pub exclusive: bool,
    pub theme: Theme,
    pub widget_definitions: Vec<WidgetDefinition>,
    pub initially_hidden: bool,
    pub auto_hide: Option<Duration>,
}

impl BarSpec {
    pub fn bar_height(&self) -> u32 {
        let min_height = self.theme.cell_height();
        match self.height {
            Some(configured) => configured.max(min_height),
            None => self.theme.bar_height(),
        }
    }
}

pub(crate) fn build_bar_specs(config: &Config) -> Vec<BarSpec> {
    let mut ordered_ids = Vec::new();
    let mut seen = HashSet::new();
    for id in &config.show {
        if config.bar_definitions.contains_key(id) && seen.insert(id.clone()) {
            ordered_ids.push(id.clone());
        }
    }
    for id in config.bar_definitions.keys() {
        if seen.insert(id.clone()) {
            ordered_ids.push(id.clone());
        }
    }

    ordered_ids
        .into_iter()
        .filter_map(|bar_id| {
            let bar_cfg = config.bar_definitions.get(&bar_id)?;
            let widget_definitions = WidgetDefinition::from_bar_and_config(bar_cfg, config);
            let theme = bar_cfg
                .theme
                .clone()
                .unwrap_or_else(|| config.theme.clone());
            let initially_hidden = !config.show.iter().any(|id| id == &bar_id);
            Some(BarSpec {
                name: Some(bar_id),
                height: bar_cfg.height,
                outputs: bar_cfg.outputs.clone(),
                position: bar_cfg.position,
                layer: BarLayer::Top,
                margin: bar_cfg.margin,
                exclusive: true,
                theme,
                widget_definitions,
                initially_hidden,
                auto_hide: None,
            })
        })
        .collect()
}

pub fn run(config: Config, config_path: PathBuf) -> Result<(), RuntimeError> {
    let connection = Connection::connect_to_env()
        .map_err(|error| RuntimeError::WaylandConnection(error.to_string()))?;
    let (globals, event_queue) = registry_queue_init(&connection)
        .map_err(|error| RuntimeError::WaylandRegistry(error.to_string()))?;
    let queue_handle = event_queue.handle();
    let mut event_loop: EventLoop<Runtime> =
        EventLoop::try_new().map_err(|error| RuntimeError::EventLoop(error.to_string()))?;
    let loop_handle = event_loop.handle();
    WaylandSource::new(connection.clone(), event_queue)
        .insert(loop_handle.clone())
        .map_err(|error| RuntimeError::EventLoop(error.to_string()))?;

    let control_socket_path = cellbar_control::socket_path()
        .map_err(|error| RuntimeError::ControlSocket(error.to_string()))?;
    let control_socket = bind_control_socket(&control_socket_path)?;
    loop_handle
        .insert_source(
            Generic::new(control_socket, Interest::READ, Mode::Level),
            |readiness, socket, runtime| {
                if !readiness.readable {
                    return Ok(PostAction::Continue);
                }
                let mut command = [0_u8; cellbar_control::MAX_COMMAND_BYTES];
                loop {
                    match socket.recv_from(&mut command) {
                        Ok((count, sender)) => {
                            let response = runtime.handle_control_command(&command[..count]);
                            if let Some(path) = sender.as_pathname()
                                && let Err(error) = socket.send_to(response.as_bytes(), path)
                            {
                                eprintln!("cellbar: cannot reply to control client: {error}");
                            }
                        }
                        Err(error) if error.kind() == ErrorKind::WouldBlock => {
                            return Ok(PostAction::Continue);
                        }
                        Err(error) => return Err(error),
                    }
                }
            },
        )
        .map_err(|error| RuntimeError::ControlSocket(error.to_string()))?;
    loop_handle
        .insert_source(
            Signals::new(&[Signal::SIGHUP, Signal::SIGINT, Signal::SIGTERM])
                .map_err(|error| RuntimeError::EventLoop(error.to_string()))?,
            |event, _, runtime| match event.signal() {
                Signal::SIGHUP => runtime.reload_requested = true,
                signal => {
                    eprintln!("cellbar: received {signal:?}, shutting down");
                    runtime.exit = true;
                }
            },
        )
        .map_err(|error| RuntimeError::EventLoop(error.to_string()))?;
    let tick_timer = loop_handle
        .insert_source(
            Timer::from_duration(Duration::from_millis(1)),
            |_, _, runtime| TimeoutAction::ToDuration(runtime.tick()),
        )
        .map_err(|error| RuntimeError::EventLoop(error.to_string()))?;

    let (image_sender, image_receiver) = channel::sync_channel(8);
    loop_handle
        .insert_source(image_receiver, |event, _, runtime| {
            if let ChannelEvent::Msg(result) = event
                && let Some(store) = runtime.renderer.images.as_mut()
            {
                let before = store.generation();
                store.complete(result);
                if store.generation() != before {
                    runtime.redraw_all();
                }
            }
        })
        .map_err(|error| RuntimeError::EventLoop(error.to_string()))?;
    let bar_specs = build_bar_specs(&config);

    let registry_state = RegistryState::new(&globals);
    let output_state = OutputState::new(&globals, &queue_handle);
    let compositor = CompositorState::bind(&globals, &queue_handle)
        .map_err(|error| RuntimeError::MissingProtocol("wl_compositor", error.to_string()))?;
    let layer_shell = LayerShell::bind(&globals, &queue_handle)
        .map_err(|error| RuntimeError::MissingProtocol("zwlr_layer_shell_v1", error.to_string()))?;
    let shm = Shm::bind(&globals, &queue_handle)
        .map_err(|error| RuntimeError::MissingProtocol("wl_shm", error.to_string()))?;
    let mut seat_state = SeatState::new(&globals, &queue_handle);
    let mut pointer = None;
    for seat in seat_state.seats() {
        if let Some(data) = seat_state.info(&seat)
            && data.has_pointer
        {
            let cursor_surface = compositor.create_surface(&queue_handle);
            match seat_state.get_pointer_with_theme::<Runtime, ()>(
                &queue_handle,
                &seat,
                shm.wl_shm(),
                cursor_surface,
                ThemeSpec::System,
            ) {
                Ok(p) => {
                    pointer = Some(p);
                    break;
                }
                Err(error) => {
                    eprintln!("cellbar: failed to get themed pointer: {error}");
                }
            }
        }
    }

    let mut runtime = Runtime {
        globals,
        registry_state,
        output_state,
        compositor,
        layer_shell,
        shm,
        seat_state,
        pointer,
        cursor_icon: CursorIcon::Default,
        queue_handle: queue_handle.clone(),
        bars: Vec::new(),
        bar_specs,
        bar_height: config.theme.bar_height(),
        bar_margin: config.bar.margin,
        exclusive: true,
        theme: config.theme.clone(),
        renderer: TextRenderer::new(
            &config.theme.font.families,
            config.theme.font.size,
            config.theme.font.cell_width_adjust,
            config.theme.font.line_height,
            {
                let (bold, italic) = config.font_style_requirements();
                FontStyleRequirements { bold, italic }
            },
        ),
        source_definitions: config.sources.clone(),
        sources: HashMap::new(),
        builtin_sources: HashMap::new(),
        config_path,
        control_socket_path,
        loop_handle,
        event_dispatcher: None,
        event_source: None,
        pipewire_dispatcher: None,
        pipewire_event_source: None,
        netlink_source: None,
        uevent_source: None,
        toplevel_tracker: None,
        workspace_tracker: None,
        providers: HashMap::new(),
        subscriptions: HashMap::new(),
        widget_index_by_id: HashMap::new(),
        interaction_registry: InteractionRegistry::new(),
        next_subscription_id: 1,
        next_bar_id: 1,
        reload_requested: false,
        exit: false,
        failure: None,
        tick_timer: Some(tick_timer),
    };
    runtime.renderer.images = Some(ImageStore::new(image_sender));
    runtime.sync_event_subscriptions()?;

    #[cfg(target_os = "linux")]
    unsafe {
        libc::malloc_trim(0);
    }

    loop {
        event_loop
            .dispatch(Duration::from_secs(60), &mut runtime)
            .map_err(|error| RuntimeError::Dispatch(error.to_string()))?;

        if let Some(message) = runtime.failure.take() {
            return Err(RuntimeError::Runtime(message));
        }
        if std::mem::take(&mut runtime.reload_requested) {
            runtime.reload();
        }
        if runtime.exit {
            return Ok(());
        }
    }
}

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("cannot connect to the Wayland compositor: {0}")]
    WaylandConnection(String),
    #[error("cannot initialize the Wayland registry: {0}")]
    WaylandRegistry(String),
    #[error("required Wayland protocol {0} is unavailable: {1}")]
    MissingProtocol(&'static str, String),
    #[error("cannot initialize the event loop: {0}")]
    EventLoop(String),
    #[error("cannot initialize the control socket: {0}")]
    ControlSocket(String),
    #[error("Wayland dispatch failed: {0}")]
    Dispatch(String),
    #[error("Wayland runtime failed: {0}")]
    Runtime(String),
}

struct Runtime {
    globals: GlobalList,
    registry_state: RegistryState,
    output_state: OutputState,
    compositor: CompositorState,
    layer_shell: LayerShell,
    shm: Shm,
    seat_state: SeatState,
    pointer: Option<ThemedPointer<()>>,
    cursor_icon: CursorIcon,
    queue_handle: QueueHandle<Runtime>,
    bars: Vec<Bar>,
    pub(super) bar_specs: Vec<BarSpec>,
    bar_height: u32,
    bar_margin: BarMargins,
    exclusive: bool,
    theme: Theme,
    renderer: TextRenderer,
    source_definitions: BTreeMap<String, ExternalSourceConfig>,
    sources: HashMap<SourceKey, SourceState>,
    builtin_sources: HashMap<String, BuiltinSourceState>,
    config_path: PathBuf,
    control_socket_path: PathBuf,
    loop_handle: LoopHandle<'static, Runtime>,
    event_dispatcher: Option<dbus::Dispatcher>,
    event_source: Option<RegistrationToken>,
    pipewire_dispatcher: Option<pipewire::Dispatcher>,
    pipewire_event_source: Option<RegistrationToken>,
    netlink_source: Option<RegistrationToken>,
    uevent_source: Option<RegistrationToken>,
    toplevel_tracker: Option<ToplevelTracker>,
    workspace_tracker: Option<WorkspaceTracker>,
    pub(super) providers: HashMap<ProviderKey, Rc<RefCell<ProviderState>>>,
    subscriptions: HashMap<u64, SubscriptionEntry>,
    widget_index_by_id: HashMap<String, Vec<WidgetKey>>,
    pub interaction_registry: InteractionRegistry,
    next_subscription_id: u64,
    next_bar_id: u64,
    reload_requested: bool,
    exit: bool,
    failure: Option<String>,
    tick_timer: Option<RegistrationToken>,
}

impl Runtime {
    pub(super) fn rearm_tick_to(&mut self, delay: Duration) {
        if let Some(token) = self.tick_timer.take() {
            self.loop_handle.remove(token);
        }
        match self
            .loop_handle
            .insert_source(Timer::from_duration(delay), |_, _, runtime| {
                TimeoutAction::ToDuration(runtime.tick())
            }) {
            Ok(token) => self.tick_timer = Some(token),
            Err(error) => eprintln!("cellbar: cannot schedule tick timer: {error}"),
        }
    }

    pub(super) fn rearm_tick(&mut self) {
        let delay = self.next_tick_delay();
        self.rearm_tick_to(delay);
    }

    pub(super) fn instantiate_bar_widgets(
        &mut self,
        definitions: &[WidgetDefinition],
        output_name: &str,
    ) -> Vec<WidgetState> {
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
                                Some(output_name.to_owned())
                            } else {
                                None
                            },
                        };
                        let state = self
                            .providers
                            .entry(key.clone())
                            .or_insert_with(|| Rc::new(RefCell::new(provider.instantiate())))
                            .clone();
                        WidgetContent::Provider { key, state }
                    }
                };
                WidgetState {
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

    pub(super) fn mark_bars_with_provider_dirty(&mut self, key: &ProviderKey) {
        let mut redraw = Vec::new();
        for (index, bar) in self.bars.iter_mut().enumerate() {
            let mut bar_matched = false;
            for widget in &mut bar.widgets {
                if let WidgetContent::Provider { key: p_key, .. } = &mut widget.content
                    && p_key == key
                {
                    widget.pushed_markup = None;
                    bar_matched = true;
                }
            }
            if bar_matched {
                if bar.configured {
                    redraw.push(index);
                } else {
                    bar.dirty = true;
                }
            }
        }
        for index in redraw {
            self.mark_bar_dirty(index);
        }
    }

    pub(super) fn rebuild_widget_indices(&mut self) {
        self.widget_index_by_id.clear();
        for bar in &self.bars {
            for (widget_index, widget) in bar.widgets.iter().enumerate() {
                self.widget_index_by_id
                    .entry(widget.id.clone())
                    .or_default()
                    .push(WidgetKey {
                        bar_id: bar.id,
                        widget_index,
                    });
            }
        }
    }

    pub(super) fn rebuild_interaction_registry(&mut self) {
        use crate::interaction::CallAction;
        self.interaction_registry.clear();
        let default_debounce = Duration::from_millis(150);
        for bar in &self.bars {
            for (widget_index, _widget) in bar.widgets.iter().enumerate() {
                let key = WidgetKey {
                    bar_id: bar.id,
                    widget_index,
                };
                if let Some(definition) = bar.widget_definitions.get(widget_index) {
                    for (pattern, event, spec) in &definition.actions {
                        let (mut command_args, refresh, debounce) = spec.to_command_args();
                        if !command_args.is_empty() {
                            let program = command_args.remove(0);
                            let action = CallAction::Command {
                                program,
                                args: command_args,
                                refresh_target: refresh,
                            };
                            self.interaction_registry.register(
                                key,
                                pattern.clone(),
                                *event,
                                action,
                                debounce.unwrap_or(default_debounce),
                                None,
                            );
                        }
                    }
                }
            }
        }
    }

    pub(super) fn refresh_widget(&mut self, widget_id: &str) -> usize {
        let Some(keys) = self.widget_index_by_id.get(widget_id).cloned() else {
            return 0;
        };
        for (key, state) in &self.providers {
            if key.widget_id == widget_id {
                let provider = state.borrow();
                for trigger in provider.triggers() {
                    if trigger.kind == crate::config::EventSourceKind::Source
                        && let Some(source_name) = &trigger.source
                        && let Some(state) = self.builtin_sources.get_mut(source_name)
                    {
                        state.next_sample = Instant::now();
                    }
                }
            }
        }
        self.refresh_builtin_sources();

        let mut changed_keys = Vec::new();
        for (key, state) in &self.providers {
            if key.widget_id == widget_id && state.borrow_mut().refresh_now() {
                changed_keys.push(key.clone());
            }
        }
        for key in changed_keys {
            self.mark_bars_with_provider_dirty(&key);
        }

        let mut redraw = HashSet::new();
        let matched = keys.len();
        for key in keys {
            if let Some(bar_index) = self.bars.iter().position(|bar| bar.id == key.bar_id) {
                let bar = &mut self.bars[bar_index];
                if let Some(widget) = bar.widgets.get_mut(key.widget_index)
                    && widget.pushed_markup.take().is_some()
                    && bar.configured
                {
                    redraw.insert(bar_index);
                }
            }
        }
        self.start_due_processes();
        for index in redraw {
            self.mark_bar_dirty(index);
            if self.failure.is_some() {
                break;
            }
        }
        matched
    }

    pub(super) fn refresh_providers(&mut self) -> Vec<usize> {
        let mut changed_keys = HashSet::new();
        for (key, state) in &self.providers {
            if state.borrow_mut().refresh() {
                changed_keys.insert(key.clone());
            }
        }
        if changed_keys.is_empty() {
            return Vec::new();
        }
        let mut changed = Vec::new();
        for (bar_index, bar) in self.bars.iter_mut().enumerate() {
            let mut bar_changed = false;
            for widget in &mut bar.widgets {
                if let WidgetContent::Provider { key, .. } = &mut widget.content
                    && changed_keys.contains(key)
                {
                    widget.pushed_markup = None;
                    bar_changed = true;
                }
            }
            if bar_changed {
                changed.push(bar_index);
            }
        }
        changed
    }

    pub(super) fn mark_bar_dirty(&mut self, index: usize) {
        let should_redraw = {
            let Some(bar) = self.bars.get_mut(index) else {
                return;
            };
            if bar.hidden {
                return;
            }
            bar.dirty = true;
            bar.configured && !bar.frame_pending
        };
        if should_redraw && let Err(error) = self.redraw_bar(index) {
            self.failure = Some(error);
        }
    }

    pub(super) fn redraw_all(&mut self) {
        for index in 0..self.bars.len() {
            if self.bars[index].configured {
                self.mark_bar_dirty(index);
                if self.failure.is_some() {
                    break;
                }
            }
        }
    }

    pub(super) fn redraw_bar(&mut self, index: usize) -> Result<(), String> {
        let queue_handle = self.queue_handle.clone();
        let bar = self
            .bars
            .get_mut(index)
            .ok_or_else(|| "bar disappeared before redraw".to_owned())?;
        if bar.hidden {
            return Ok(());
        }
        let theme = bar.theme.clone().unwrap_or_else(|| self.theme.clone());
        bar.render(&theme, &mut self.renderer, &queue_handle, &self.compositor)
    }

    pub(super) fn push_widget_markup(
        &mut self,
        widget_id: &str,
        markup: &str,
    ) -> Result<usize, String> {
        let base_dir = self
            .config_path
            .parent()
            .unwrap_or(std::path::Path::new("."));
        let spans = crate::markup::parse_markup(markup, base_dir)?;
        let Some(keys) = self.widget_index_by_id.get(widget_id).cloned() else {
            return Err(format!("unknown widget {widget_id:?}"));
        };
        let mut redraw = HashSet::new();
        let count = keys.len();
        for key in keys {
            if let Some(bar_index) = self.bars.iter().position(|bar| bar.id == key.bar_id) {
                let bar = &mut self.bars[bar_index];
                if let Some(widget) = bar.widgets.get_mut(key.widget_index) {
                    widget.pushed_markup = Some(spans.clone());
                    if let Some(slot) = bar.static_spans.get_mut(key.widget_index) {
                        *slot = None;
                    }
                    if bar.configured && !bar.hidden {
                        redraw.insert(bar_index);
                    }
                }
            }
        }
        for index in redraw {
            self.mark_bar_dirty(index);
        }
        Ok(count)
    }

    pub(super) fn clear_widget_markup(&mut self, widget_id: &str) -> Result<usize, String> {
        let Some(keys) = self.widget_index_by_id.get(widget_id).cloned() else {
            return Err(format!("unknown widget {widget_id:?}"));
        };
        let mut redraw = HashSet::new();
        let count = keys.len();
        for key in keys {
            if let Some(bar_index) = self.bars.iter().position(|bar| bar.id == key.bar_id) {
                let bar = &mut self.bars[bar_index];
                if let Some(widget) = bar.widgets.get_mut(key.widget_index) {
                    widget.pushed_markup = None;
                    if let Some(slot) = bar.static_spans.get_mut(key.widget_index) {
                        *slot = None;
                    }
                    if bar.configured && !bar.hidden {
                        redraw.insert(bar_index);
                    }
                }
            }
        }
        for index in redraw {
            self.mark_bar_dirty(index);
        }
        Ok(count)
    }

    pub(super) fn visible_provider_keys(&self) -> HashSet<ProviderKey> {
        let mut keys = HashSet::new();
        for bar in &self.bars {
            if !bar.hidden {
                for widget in &bar.widgets {
                    if let WidgetContent::Provider { key, .. } = &widget.content {
                        keys.insert(key.clone());
                    }
                }
            }
        }
        keys
    }

    pub(super) fn visible_source_names(&self) -> HashSet<String> {
        let visible_keys = self.visible_provider_keys();
        let mut sources = HashSet::new();
        for (key, state) in &self.providers {
            if visible_keys.contains(key) {
                let provider = state.borrow();
                for trigger in provider.triggers() {
                    if let Some(src) = &trigger.source {
                        sources.insert(src.clone());
                    }
                }
            }
        }
        sources
    }

    pub(super) fn bar_exists(&self, target_bar: &str) -> bool {
        self.bars
            .iter()
            .any(|bar| bar.name.as_deref() == Some(target_bar))
    }

    pub(super) fn set_bars_hidden(&mut self, target_bar: Option<&str>, hidden: bool) -> usize {
        let mut changed_count = 0;
        let now = Instant::now();
        for bar in &mut self.bars {
            if let Some(target) = target_bar
                && bar.name.as_deref() != Some(target)
            {
                continue;
            }
            if hidden {
                bar.auto_hide_deadline = None;
                bar.pointer_inside = false;
            } else if let Some(dur) = bar.auto_hide
                && !bar.pointer_inside
            {
                bar.auto_hide_deadline = Some(now + dur);
            }
            if bar.hidden != hidden {
                bar.hidden = hidden;
                changed_count += 1;
                bar.frame_pending = false;
                bar.last_frame = None;
                bar.last_geometry = None;
                if hidden {
                    bar.configured = false;
                    bar.layer.set_exclusive_zone(0);
                    bar.layer.wl_surface().attach(None, 0, 0);
                    bar.layer.commit();
                } else {
                    bar.configured = false;
                    bar.dirty = true;
                    let theme = bar.theme.as_ref().unwrap_or(&self.theme);
                    let min_height = theme.cell_height();
                    let bar_height = bar
                        .height
                        .map_or_else(|| theme.bar_height(), |h| h.max(min_height));
                    bar.logical_height = bar_height;
                    wayland::apply_bar_surface_config(
                        &bar.layer,
                        bar.position,
                        bar.logical_height,
                        bar.margin,
                        bar.exclusive,
                    );
                    bar.layer.commit();
                }
            }
        }
        if !hidden {
            if changed_count > 0 {
                let visible_keys = self.visible_provider_keys();
                let visible_sources = self.visible_source_names();
                for (key, state) in &self.providers {
                    if visible_keys.contains(key) {
                        state.borrow_mut().refresh_now();
                    }
                }
                for (key, state) in &mut self.sources {
                    if visible_sources.contains(&key.id) && state.child.is_none() {
                        state.next_start = Some(Instant::now());
                    }
                }
                for (id, state) in &mut self.builtin_sources {
                    if visible_sources.contains(id) {
                        state.next_sample = Instant::now();
                    }
                }
                self.refresh_builtin_sources();
                self.refresh_providers();
                self.start_due_processes();
                self.start_due_sources();
            }
            let next = self.next_tick_delay();
            self.rearm_tick_to(next);
        }
        changed_count
    }

    pub(super) fn reload(&mut self) {
        let candidate = match Config::load(&self.config_path) {
            Ok(config) => config,
            Err(error) => {
                eprintln!(
                    "cellbar: reload rejected for {}: {error}",
                    self.config_path.display()
                );
                return;
            }
        };

        self.apply_config(candidate);
        eprintln!("cellbar: reloaded {}", self.config_path.display());
    }

    pub(super) fn apply_config(&mut self, config: Config) {
        self.stop_all_sources();
        self.stop_all_processes();
        self.providers.clear();

        self.bar_specs = build_bar_specs(&config);
        self.bar_height = config.theme.bar_height();
        self.bar_margin = config.bar.margin;
        self.exclusive = true;
        self.theme = config.theme.clone();
        let images = self.renderer.images.take();
        self.renderer = TextRenderer::new(
            &config.theme.font.families,
            config.theme.font.size,
            config.theme.font.cell_width_adjust,
            config.theme.font.line_height,
            {
                let (bold, italic) = config.font_style_requirements();
                FontStyleRequirements { bold, italic }
            },
        );
        self.renderer.images = images;
        if let Some(store) = self.renderer.images.as_mut() {
            store.clear();
        }
        self.source_definitions = config.sources.clone();

        let mut idx = 0;
        while idx < self.bars.len() {
            let bar = &self.bars[idx];
            let still_valid = self.bar_specs.iter().any(|spec| {
                spec.name == bar.name
                    && spec.position == bar.position
                    && spec.layer == bar.layer_level
                    && (spec.outputs.is_empty()
                        || spec
                            .outputs
                            .iter()
                            .any(|o| o == &bar.output_name || o == "*"))
            });
            if !still_valid {
                let removed = self.bars.swap_remove(idx);
                eprintln!(
                    "cellbar: removed obsolete bar {:?} on {}",
                    removed.name, removed.output_name
                );
            } else {
                idx += 1;
            }
        }

        for bar_idx in 0..self.bars.len() {
            let (
                margin,
                exclusive,
                theme,
                logical_height,
                widget_defs,
                output_name,
                initially_hidden,
                auto_hide,
                height,
            ) = {
                let bar = &self.bars[bar_idx];
                let spec = self
                    .bar_specs
                    .iter()
                    .find(|s| s.name == bar.name && s.position == bar.position)
                    .expect("already filtered still_valid bars");
                (
                    spec.margin,
                    spec.exclusive,
                    Some(spec.theme.clone()),
                    spec.bar_height(),
                    spec.widget_definitions.clone(),
                    bar.output_name.clone(),
                    spec.initially_hidden,
                    spec.auto_hide,
                    spec.height,
                )
            };
            let widgets = self.instantiate_bar_widgets(&widget_defs, &output_name);
            let bar = &mut self.bars[bar_idx];
            bar.height = height;
            let was_hidden = bar.hidden;
            bar.margin = margin;
            bar.exclusive = exclusive;
            bar.theme = theme;
            bar.logical_height = logical_height;
            bar.widget_definitions = widget_defs;
            bar.widgets = widgets;
            bar.hidden = initially_hidden;
            bar.auto_hide = auto_hide;
            if initially_hidden {
                bar.auto_hide_deadline = None;
                bar.pointer_inside = false;
            } else if let Some(dur) = auto_hide
                && !bar.pointer_inside
            {
                bar.auto_hide_deadline = Some(Instant::now() + dur);
            }
            if initially_hidden {
                bar.configured = false;
                bar.layer.set_exclusive_zone(0);
                bar.layer.wl_surface().attach(None, 0, 0);
            } else {
                if was_hidden {
                    bar.configured = false;
                    bar.dirty = true;
                }
                wayland::apply_bar_surface_config(
                    &bar.layer,
                    bar.position,
                    bar.logical_height,
                    bar.margin,
                    bar.exclusive,
                );
            }
            bar.static_spans.clear();
            bar.widget_ranges.clear();
            bar.last_frame = None;
            bar.last_geometry = None;
            if !bar.configured || initially_hidden {
                bar.layer.commit();
            }
        }
        self.rebuild_widget_indices();
        self.rebuild_interaction_registry();
        if let Err(error) = self.sync_event_subscriptions() {
            self.failure = Some(error.to_string());
            return;
        }

        #[cfg(target_os = "linux")]
        unsafe {
            libc::malloc_trim(0);
        }
        if self.bars.iter().any(|b| !b.hidden) {
            self.redraw_all();
        }

        let queue_handle = self.queue_handle.clone();
        let outputs: Vec<_> = self.output_state.outputs().collect();
        for output in outputs {
            self.create_bar(&queue_handle, output);
        }
    }

    pub(super) fn check_auto_hide(&mut self) {
        let now = Instant::now();
        let mut bars_to_hide = Vec::new();
        for bar in &self.bars {
            if !bar.hidden
                && !bar.pointer_inside
                && let Some(deadline) = bar.auto_hide_deadline
                && now >= deadline
                && let Some(name) = &bar.name
            {
                bars_to_hide.push(name.clone());
            }
        }
        for name in bars_to_hide {
            self.set_bars_hidden(Some(&name), true);
        }
    }

    pub(super) fn tick(&mut self) -> Duration {
        self.check_auto_hide();
        for bar in &mut self.bars {
            if bar.configured && bar.frame_pending && bar.dirty {
                bar.frame_pending = false;
            }
        }
        self.refresh_builtin_sources();
        for index in self.refresh_providers() {
            self.mark_bar_dirty(index);
            if self.failure.is_some() {
                return DEFAULT_TICK_DELAY;
            }
        }
        self.reap_processes();
        self.start_due_processes();
        self.reap_sources();
        self.start_due_sources();
        #[cfg(target_os = "linux")]
        unsafe {
            libc::malloc_trim(0);
        }
        self.next_tick_delay()
    }

    pub(super) fn next_tick_delay(&self) -> Duration {
        let now = Instant::now();
        let visible_keys = self.visible_provider_keys();
        let visible_sources = self.visible_source_names();
        let widget_deadlines = self
            .providers
            .iter()
            .filter(|(key, _)| visible_keys.contains(*key))
            .filter_map(|(_, provider)| provider.borrow().next_deadline());
        let source_deadlines = self
            .sources
            .iter()
            .filter(|(key, _)| visible_sources.contains(&key.id))
            .filter_map(|(_, source)| {
                if source.child.is_none() {
                    source.next_start
                } else {
                    None
                }
            });
        let builtin_source_deadlines = self
            .builtin_sources
            .iter()
            .filter(|(id, _)| visible_sources.contains(*id))
            .map(|(_, source)| source.next_sample);
        let bar_auto_hide_deadlines = self
            .bars
            .iter()
            .filter(|bar| !bar.hidden && !bar.pointer_inside)
            .filter_map(|bar| bar.auto_hide_deadline);
        next_tick_delay_for(
            now,
            widget_deadlines
                .chain(source_deadlines)
                .chain(builtin_source_deadlines)
                .chain(bar_auto_hide_deadlines),
        )
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.stop_all_sources();
        self.stop_all_processes();
        self.bars.clear();
        self.unregister_unused_providers();
        self.stop_event_dispatcher();
        self.stop_netlink_listener();
        self.stop_toplevel_listener();
        self.stop_workspace_listener();
        let _ = fs::remove_file(&self.control_socket_path);
    }
}

pub(super) fn next_tick_delay_for(
    now: Instant,
    deadlines: impl IntoIterator<Item = Instant>,
) -> Duration {
    let mut next = MAX_TICK_DELAY;
    for deadline in deadlines {
        next = next.min(deadline.checked_duration_since(now).unwrap_or_default());
    }
    next.max(Duration::from_millis(1))
}
