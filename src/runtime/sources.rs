use std::{
    collections::{HashMap, HashSet},
    io::{ErrorKind, Read},
    os::unix::process::CommandExt,
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use smithay_client_toolkit::reexports::calloop::{
    Interest, Mode, PostAction, RegistrationToken,
    generic::Generic,
    timer::{TimeoutAction, Timer},
};

use crate::{
    config::{EventBus, EventSourceKind, ExternalSourceConfig, SourceScope},
    events::{self, dbus, netlink, pipewire, system, uevent},
    runtime::{
        MAX_TICK_DELAY, Runtime, RuntimeError,
        model::{ProviderKey, ProviderState, WidgetContent},
        process::{
            MAX_PROCESS_BYTES_PER_EVENT, MAX_PROCESS_MESSAGE_BYTES, set_nonblocking,
            terminate_process_group,
        },
    },
};

pub const SOURCE_BACKOFF_MAX: Duration = Duration::from_secs(30);

pub(super) struct SubscriptionEntry {
    pub(super) key: ProviderKey,
    pub(super) manifest_id: String,
    pub(super) kind: EventSourceKind,
    pub(super) event: String,
    pub(super) subsystem: Option<String>,
    pub(super) action: Option<String>,
    pub(super) source: Option<String>,
    pub(super) source_interval: Option<Duration>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct SourceKey {
    pub(super) id: String,
    pub(super) output: Option<String>,
}

pub(super) struct SourceState {
    pub(super) definition: ExternalSourceConfig,
    pub(super) child: Option<Child>,
    pub(super) source: Option<RegistrationToken>,
    pub(super) pending: Vec<u8>,
    pub(super) discarding_oversized_line: bool,
    pub(super) next_start: Option<Instant>,
    pub(super) restart_failures: u32,
}

pub(super) struct BuiltinSourceState {
    pub(super) interval: Duration,
    pub(super) next_sample: Instant,
    pub(super) previous_cpu: Option<system::CpuSnapshot>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SourceMessage {
    #[serde(default = "source_protocol_version")]
    pub(super) version: u8,
    pub(super) event: String,
    #[serde(default)]
    pub(super) data: serde_json::Value,
}

pub(super) fn source_protocol_version() -> u8 {
    1
}

impl Runtime {
    pub(super) fn handle_event(&mut self, event: events::Event) {
        let Some(entry) = self.subscriptions.get(&event.subscription_id) else {
            return;
        };
        let key = entry.key.clone();
        let trigger_id = entry.manifest_id.clone();
        let context = event.context;
        let Some(state) = self.providers.get(&key) else {
            return;
        };
        let debounce = {
            let state_ref = state.borrow();
            if !state_ref.is_initialized() {
                Duration::ZERO
            } else {
                state_ref.expression_debounce()
            }
        };
        let deadline = Instant::now() + debounce;
        state
            .borrow_mut()
            .schedule_event(trigger_id, context, deadline);
        if debounce.is_zero() {
            if state.borrow_mut().refresh() {
                self.mark_bars_with_provider_dirty(&key);
            }
        } else {
            self.schedule_event_debounce(deadline);
        }
    }

    pub(super) fn schedule_event_debounce(&mut self, deadline: Instant) {
        let now = Instant::now();
        if let Some(current_deadline) = self.debounce_deadline
            && current_deadline <= deadline
        {
            return;
        }

        if let Some(token) = self.debounce_timer.take() {
            self.loop_handle.remove(token);
        }

        let delay = deadline.checked_duration_since(now).unwrap_or_default();
        match self
            .loop_handle
            .insert_source(Timer::from_duration(delay), |_, _, runtime| {
                runtime.debounce_timer = None;
                runtime.debounce_deadline = None;
                let next = runtime.tick();
                // The debounce timer is one-shot. Hand the recomputed wake-up
                // time back to the main tick timer: two events landing
                // microseconds apart share this timer, so the later deadline is
                // not due when it fires and would otherwise be lost until the
                // next unrelated event.
                runtime.rearm_tick_to(next);
                TimeoutAction::Drop
            }) {
            Ok(token) => {
                self.debounce_timer = Some(token);
                self.debounce_deadline = Some(deadline);
            }
            Err(error) => {
                eprintln!("cellbar: cannot schedule event debounce timer: {error}");
            }
        }
    }

    pub(super) fn sync_event_subscriptions(&mut self) -> Result<(), RuntimeError> {
        self.clear_event_subscriptions();
        let has_trigger = |pred: fn(&crate::config::Trigger) -> bool| {
            self.bar_specs.iter().any(|b| {
                b.widget_definitions.iter().any(|d| {
                    d.content
                        .provider()
                        .is_some_and(|p| p.triggers().iter().any(pred))
                })
            })
        };

        let needs_dbus = has_trigger(|t| t.kind == EventSourceKind::Dbus);
        let needs_pipewire = has_trigger(|t| t.kind == EventSourceKind::Pipewire);
        let needs_netlink = has_trigger(|t| t.kind == EventSourceKind::Netlink);
        let needs_uevent = self
            .builtin_sources
            .keys()
            .any(|s| s == system::BATTERY_SOURCE || s == system::BACKLIGHT_SOURCE)
            || has_trigger(|t| t.kind == EventSourceKind::Uevent);
        let needs_toplevel =
            has_trigger(|t| t.kind == EventSourceKind::Wayland && t.event == "toplevel");
        let needs_workspace =
            has_trigger(|t| t.kind == EventSourceKind::Wayland && t.event == "workspace");

        if needs_dbus && self.event_dispatcher.is_none() {
            let (sender, source) = events::channel();
            let dispatcher = dbus::start(sender);
            let token = self
                .loop_handle
                .insert_source(source, |event, _, runtime| {
                    if let calloop::channel::Event::Msg(event) = event {
                        runtime.handle_event(event);
                    }
                })
                .map_err(|error| RuntimeError::EventLoop(error.to_string()))?;
            self.event_dispatcher = Some(dispatcher);
            self.event_source = Some(token);
        }
        if !needs_dbus {
            self.stop_event_dispatcher();
        }

        if needs_pipewire && self.pipewire_dispatcher.is_none() {
            let (sender, source) = events::channel();
            let dispatcher = pipewire::start(sender);
            let token = self
                .loop_handle
                .insert_source(source, |event, _, runtime| {
                    if let calloop::channel::Event::Msg(event) = event {
                        runtime.dispatch_pipewire_event(event);
                    }
                })
                .map_err(|error| RuntimeError::EventLoop(error.to_string()))?;
            self.pipewire_dispatcher = Some(dispatcher);
            self.pipewire_event_source = Some(token);
        }
        if !needs_pipewire {
            self.stop_pipewire_dispatcher();
        }

        if needs_netlink && self.netlink_source.is_none() {
            self.start_netlink_listener()?;
        }
        if !needs_netlink {
            self.stop_netlink_listener();
        }

        if needs_uevent && self.uevent_source.is_none() {
            self.start_uevent_listener()?;
        }
        if !needs_uevent {
            self.stop_uevent_listener();
        }

        if needs_toplevel && self.toplevel_tracker.is_none() {
            self.start_toplevel_listener();
        }
        if !needs_toplevel {
            self.stop_toplevel_listener();
        }

        if needs_workspace && self.workspace_tracker.is_none() {
            self.start_workspace_listener();
        }
        if !needs_workspace {
            self.stop_workspace_listener();
        }

        for index in 0..self.bars.len() {
            self.register_bar_subscriptions(index);
        }
        self.sync_sources();
        self.rearm_tick();
        Ok(())
    }

    pub(super) fn sync_sources(&mut self) {
        let mut desired = HashSet::new();
        for entry in self.subscriptions.values() {
            if entry.kind != EventSourceKind::Source {
                continue;
            }
            let Some(id) = entry.source.clone() else {
                continue;
            };
            if system::is_builtin_source(&id) {
                continue;
            }
            let Some(definition) = self.source_definitions.get(&id) else {
                continue;
            };
            desired.insert(SourceKey {
                id,
                output: (definition.scope == SourceScope::Output)
                    .then(|| entry.key.output.clone())
                    .flatten(),
            });
        }

        let obsolete: Vec<_> = self
            .sources
            .keys()
            .filter(|key| !desired.contains(*key))
            .cloned()
            .collect();
        for key in obsolete {
            self.stop_source(&key);
        }
        for key in desired {
            if self.sources.contains_key(&key) {
                continue;
            }
            let Some(definition) = self.source_definitions.get(&key.id).cloned() else {
                continue;
            };
            self.sources.insert(
                key,
                SourceState {
                    definition,
                    child: None,
                    source: None,
                    pending: Vec::new(),
                    discarding_oversized_line: false,
                    next_start: Some(Instant::now()),
                    restart_failures: 0,
                },
            );
        }
        self.sync_builtin_sources();
        self.refresh_builtin_sources();
        self.start_due_sources();
    }

    pub(super) fn sync_builtin_sources(&mut self) {
        let mut desired = HashMap::new();
        for entry in self.subscriptions.values() {
            if entry.kind != EventSourceKind::Source {
                continue;
            }
            let Some(source) = entry.source.as_deref() else {
                continue;
            };
            let Some(default_interval) = system::default_interval(source) else {
                continue;
            };
            let interval = entry.source_interval.unwrap_or(default_interval);
            desired
                .entry(source.to_owned())
                .and_modify(|current: &mut Duration| *current = (*current).min(interval))
                .or_insert(interval);
        }

        self.builtin_sources
            .retain(|source, _| desired.contains_key(source));
        for (source, interval) in desired {
            if let Some(state) = self.builtin_sources.get_mut(&source) {
                state.interval = interval;
            } else {
                self.builtin_sources.insert(
                    source,
                    BuiltinSourceState {
                        interval,
                        next_sample: Instant::now(),
                        previous_cpu: None,
                    },
                );
            }
        }
    }

    pub(super) fn refresh_builtin_sources(&mut self) {
        let now = Instant::now();
        let visible_sources = self.visible_source_names();
        let due: Vec<_> = self
            .builtin_sources
            .iter()
            .filter_map(|(source, state)| {
                (visible_sources.contains(source) && state.next_sample <= now)
                    .then_some(source.clone())
            })
            .collect();

        for source in due {
            let sampled = {
                let Some(state) = self.builtin_sources.get_mut(&source) else {
                    continue;
                };
                let delay = system::next_delay(&source, state.interval);
                state.next_sample = now.checked_add(delay).unwrap_or(now + MAX_TICK_DELAY);
                system::sample(&source, &mut state.previous_cpu)
            };
            match sampled {
                Ok((event, data)) => self.dispatch_builtin_source_event(&source, event, data),
                Err(error) => eprintln!("cellbar: builtin source {source:?} failed: {error}"),
            }
        }
    }

    pub(super) fn dispatch_builtin_source_event(
        &mut self,
        source: &str,
        event: &str,
        data: serde_json::Value,
    ) {
        let targets: Vec<_> = self
            .subscriptions
            .iter()
            .filter(|(_, entry)| {
                entry.kind == EventSourceKind::Source
                    && entry.source.as_deref() == Some(source)
                    && entry.event == event
            })
            .map(|(id, _)| *id)
            .collect();
        let context = serde_json::json!({
            "source": source,
            "event": event,
            "data": data,
        })
        .to_string();
        for subscription_id in targets {
            self.handle_event(events::Event {
                subscription_id,
                context: context.clone(),
            });
        }
    }

    pub(super) fn start_due_sources(&mut self) {
        let now = Instant::now();
        let visible_sources = self.visible_source_names();
        let keys: Vec<_> = self
            .sources
            .iter()
            .filter_map(|(key, source)| {
                (visible_sources.contains(&key.id)
                    && source.child.is_none()
                    && source.next_start.is_some_and(|start| start <= now))
                .then_some(key.clone())
            })
            .collect();
        for key in keys {
            self.spawn_source(key);
        }
    }

    pub(super) fn spawn_source(&mut self, key: SourceKey) {
        let Some(state) = self.sources.get(&key) else {
            return;
        };
        let definition = state.definition.clone();
        let output_name = key.output.as_deref().unwrap_or("");
        let expected_parent = i32::try_from(std::process::id()).unwrap_or(i32::MAX);
        let mut command = Command::new(&definition.program);
        if let Some(parent) = Path::new(&definition.program).parent()
            && parent.is_dir()
        {
            command.current_dir(parent);
        }
        let source_args: Vec<_> = definition
            .args
            .iter()
            .map(|arg| arg.replace("${output}", output_name))
            .collect();
        command
            .args(&source_args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .process_group(0);
        unsafe {
            command.pre_exec(move || {
                use rustix::process::{Pid, Signal, getppid, set_parent_process_death_signal};
                set_parent_process_death_signal(Some(Signal::KILL))?;
                if getppid().map(Pid::as_raw_pid) != Some(expected_parent) {
                    return Err(std::io::Error::other("source parent exited before startup"));
                }
                Ok(())
            });
        }
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                eprintln!("cellbar: cannot start source {:?}: {error}", key.id);
                self.schedule_source_restart(&key);
                return;
            }
        };
        let Some(stdout) = child.stdout.take() else {
            terminate_process_group(&mut child);
            let _ = child.wait();
            self.schedule_source_restart(&key);
            return;
        };
        if let Err(error) = set_nonblocking(&stdout) {
            eprintln!(
                "cellbar: cannot configure source {:?} stdout: {error}",
                key.id
            );
            terminate_process_group(&mut child);
            let _ = child.wait();
            self.schedule_source_restart(&key);
            return;
        }
        let callback_key = key.clone();
        let token = match self.loop_handle.insert_source(
            Generic::new(stdout, Interest::READ, Mode::Level),
            move |readiness, stdout, runtime| {
                if !readiness.readable {
                    return Ok(PostAction::Continue);
                }
                let mut bytes = [0_u8; 8192];
                let mut processed = 0;
                loop {
                    match unsafe { stdout.get_mut() }.read(&mut bytes) {
                        Ok(0) => {
                            runtime.handle_source_eof(&callback_key);
                            return Ok(PostAction::Remove);
                        }
                        Ok(count) => {
                            processed += count;
                            runtime.handle_source_bytes(&callback_key, &bytes[..count]);
                            if processed >= MAX_PROCESS_BYTES_PER_EVENT {
                                return Ok(PostAction::Continue);
                            }
                        }
                        Err(error) if error.kind() == ErrorKind::WouldBlock => {
                            return Ok(PostAction::Continue);
                        }
                        Err(error) => {
                            eprintln!("cellbar: source stdout read failed: {error}");
                            runtime.handle_source_eof(&callback_key);
                            return Ok(PostAction::Remove);
                        }
                    }
                }
            },
        ) {
            Ok(token) => token,
            Err(error) => {
                eprintln!("cellbar: cannot monitor source {:?}: {error}", key.id);
                terminate_process_group(&mut child);
                let _ = child.wait();
                self.schedule_source_restart(&key);
                return;
            }
        };
        if let Some(state) = self.sources.get_mut(&key) {
            state.child = Some(child);
            state.source = Some(token);
            state.next_start = None;
            state.pending.clear();
            state.discarding_oversized_line = false;
        }
    }

    pub(super) fn schedule_source_restart(&mut self, key: &SourceKey) {
        let Some(source) = self.sources.get_mut(key) else {
            return;
        };
        source.restart_failures = source.restart_failures.saturating_add(1);
        let factor = 1_u32
            .checked_shl(source.restart_failures.min(16))
            .unwrap_or(u32::MAX);
        let delay = source
            .definition
            .restart_after
            .0
            .checked_mul(factor)
            .unwrap_or(SOURCE_BACKOFF_MAX)
            .min(SOURCE_BACKOFF_MAX);
        source.next_start = Some(Instant::now() + delay);
    }

    pub(super) fn handle_source_bytes(&mut self, key: &SourceKey, bytes: &[u8]) {
        let mut lines = Vec::new();
        let Some(source) = self.sources.get_mut(key) else {
            return;
        };
        for byte in bytes {
            if source.discarding_oversized_line {
                if *byte == b'\n' {
                    source.discarding_oversized_line = false;
                }
                continue;
            }
            if *byte == b'\n' {
                let mut line = std::mem::take(&mut source.pending);
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                lines.push(line);
            } else if source.pending.len() == MAX_PROCESS_MESSAGE_BYTES {
                source.pending.clear();
                source.discarding_oversized_line = true;
                eprintln!(
                    "cellbar: discarded oversized output from source {:?}",
                    key.id
                );
            } else {
                source.pending.push(*byte);
            }
        }
        for line in lines {
            self.dispatch_source_line(key, &line);
        }
    }

    pub(super) fn handle_source_eof(&mut self, key: &SourceKey) {
        let final_line = self.sources.get_mut(key).and_then(|source| {
            source.source = None;
            (!source.discarding_oversized_line && !source.pending.is_empty())
                .then(|| std::mem::take(&mut source.pending))
        });
        if let Some(line) = final_line {
            self.dispatch_source_line(key, &line);
        }
        self.reap_sources();
    }

    pub(super) fn dispatch_source_line(&mut self, key: &SourceKey, line: &[u8]) {
        let message: SourceMessage = match serde_json::from_slice::<SourceMessage>(line) {
            Ok(message) if message.version == 1 => message,
            Ok(message) => {
                eprintln!(
                    "cellbar: source {:?} emitted unsupported version {}",
                    key.id, message.version
                );
                return;
            }
            Err(error) => {
                eprintln!(
                    "cellbar: source {:?} emitted invalid JSONL: {error}",
                    key.id
                );
                return;
            }
        };
        let declared = self
            .sources
            .get(key)
            .is_some_and(|source| source.definition.emits.contains(&message.event));
        if !declared {
            eprintln!(
                "cellbar: source {:?} emitted undeclared event {:?}",
                key.id, message.event
            );
            return;
        }
        let context = serde_json::json!({
            "source": key.id,
            "event": message.event,
            "data": message.data,
        })
        .to_string();
        let targets: Vec<_> = self
            .subscriptions
            .iter()
            .filter(|(_, entry)| {
                entry.kind == EventSourceKind::Source
                    && entry.source.as_deref() == Some(key.id.as_str())
                    && entry.event == message.event
                    && (key.output.is_none() || key.output == entry.key.output)
            })
            .map(|(id, _)| *id)
            .collect();
        for subscription_id in targets {
            self.handle_event(events::Event {
                subscription_id,
                context: context.clone(),
            });
        }
    }

    pub(super) fn reap_sources(&mut self) {
        let mut exited = Vec::new();
        for (key, source) in &mut self.sources {
            let Some(child) = source.child.as_mut() else {
                continue;
            };
            match child.try_wait() {
                Ok(Some(_)) => exited.push((key.clone(), child.id())),
                Ok(None) => {}
                Err(error) => eprintln!("cellbar: cannot inspect source {:?}: {error}", key.id),
            }
        }
        for (key, _pid) in exited {
            let token = match self.sources.get_mut(&key) {
                Some(source) => {
                    source.child = None;
                    source.source.take()
                }
                None => continue,
            };
            if let Some(token) = token {
                self.loop_handle.remove(token);
            }
            self.schedule_source_restart(&key);
        }
    }

    pub(super) fn stop_source(&mut self, key: &SourceKey) {
        let Some(mut source) = self.sources.remove(key) else {
            return;
        };
        if let Some(token) = source.source.take() {
            self.loop_handle.remove(token);
        }
        if let Some(mut child) = source.child.take() {
            terminate_process_group(&mut child);
            let _ = child.wait();
        }
    }

    pub(super) fn stop_all_sources(&mut self) {
        let keys: Vec<_> = self.sources.keys().cloned().collect();
        for key in keys {
            self.stop_source(&key);
        }
        self.builtin_sources.clear();
    }

    pub(super) fn start_netlink_listener(&mut self) -> Result<(), RuntimeError> {
        if self.netlink_source.is_some() {
            return Ok(());
        }
        let socket = netlink::open_route_socket().map_err(|error| {
            RuntimeError::EventLoop(format!("cannot open netlink route socket: {error}"))
        })?;
        let token = self
            .loop_handle
            .insert_source(
                Generic::new(socket, Interest::READ, Mode::Level),
                |readiness, socket, runtime| {
                    if !readiness.readable {
                        return Ok(PostAction::Continue);
                    }
                    runtime.handle_netlink_read(socket);
                    Ok(PostAction::Continue)
                },
            )
            .map_err(|error| RuntimeError::EventLoop(error.to_string()))?;
        self.netlink_source = Some(token);
        Ok(())
    }

    pub(super) fn stop_netlink_listener(&mut self) {
        if let Some(token) = self.netlink_source.take() {
            self.loop_handle.remove(token);
        }
    }

    pub(super) fn start_uevent_listener(&mut self) -> Result<(), RuntimeError> {
        if self.uevent_source.is_some() {
            return Ok(());
        }
        let socket = uevent::open_socket().map_err(|error| {
            RuntimeError::EventLoop(format!("cannot open kernel uevent socket: {error}"))
        })?;
        let token = self
            .loop_handle
            .insert_source(
                Generic::new(socket, Interest::READ, Mode::Level),
                |readiness, socket, runtime| {
                    if !readiness.readable {
                        return Ok(PostAction::Continue);
                    }
                    runtime.handle_uevent_read(socket);
                    Ok(PostAction::Continue)
                },
            )
            .map_err(|error| RuntimeError::EventLoop(error.to_string()))?;
        self.uevent_source = Some(token);
        Ok(())
    }

    pub(super) fn stop_uevent_listener(&mut self) {
        if let Some(token) = self.uevent_source.take() {
            self.loop_handle.remove(token);
        }
    }

    pub(super) fn handle_netlink_read(&mut self, socket: &std::os::fd::OwnedFd) {
        use rustix::io::read;
        let mut buffer = [0_u8; 8192];
        loop {
            match read(socket, &mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    let parsed_events = netlink::parse_netlink_messages(&buffer[..count]);
                    if parsed_events.is_empty() {
                        let fallback = netlink::NetlinkEvent {
                            kind: "network",
                            action: "change",
                            interface_index: None,
                            interface_name: None,
                        };
                        self.dispatch_netlink_event(&fallback);
                    } else {
                        for ev in &parsed_events {
                            self.dispatch_netlink_event(ev);
                        }
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    eprintln!("cellbar: netlink read error: {error}");
                    break;
                }
            }
        }
    }

    pub(super) fn handle_uevent_read(&mut self, socket: &std::os::fd::OwnedFd) {
        use rustix::io::read;
        let mut buffer = [0_u8; 8192];
        loop {
            match read(socket, &mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    if let Some(event) = uevent::parse_message(&buffer[..count]) {
                        self.dispatch_uevent(&event);
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    eprintln!("cellbar: kernel uevent read error: {error}");
                    break;
                }
            }
        }
    }

    pub(super) fn dispatch_uevent(&mut self, event: &uevent::Uevent) {
        if event.subsystem.as_deref() == Some("power_supply") {
            if let Some(state) = self.builtin_sources.get_mut(system::BATTERY_SOURCE) {
                state.next_sample = std::time::Instant::now();
            }
            self.refresh_builtin_sources();
        } else if event.subsystem.as_deref() == Some("backlight") {
            if let Some(state) = self.builtin_sources.get_mut(system::BACKLIGHT_SOURCE) {
                state.next_sample = std::time::Instant::now();
            }
            self.refresh_builtin_sources();
        }

        let targets: Vec<_> = self
            .subscriptions
            .iter()
            .filter(|(_, entry)| {
                entry.kind == EventSourceKind::Uevent
                    && entry
                        .subsystem
                        .as_deref()
                        .is_none_or(|subsystem| event.subsystem.as_deref() == Some(subsystem))
                    && entry
                        .action
                        .as_deref()
                        .is_none_or(|action| event.action == action)
            })
            .map(|(sub_id, entry)| (*sub_id, entry.manifest_id.clone()))
            .collect();
        for (sub_id, subscription) in targets {
            self.handle_event(events::Event {
                subscription_id: sub_id,
                context: uevent::format_event_context(&subscription, event),
            });
        }
    }

    pub(super) fn dispatch_pipewire_event(&mut self, event: pipewire::Event) {
        let targets: Vec<_> = self
            .subscriptions
            .iter()
            .filter(|(_, entry)| {
                entry.kind == EventSourceKind::Pipewire
                    && pipewire::subscription_matches(&entry.event, event)
            })
            .map(|(sub_id, entry)| (*sub_id, entry.manifest_id.clone()))
            .collect();
        for (sub_id, subscription) in targets {
            self.handle_event(events::Event {
                subscription_id: sub_id,
                context: pipewire::format_event_context(&subscription, event),
            });
        }
    }

    pub(super) fn dispatch_netlink_event(&mut self, event: &netlink::NetlinkEvent) {
        let targets: Vec<_> = self
            .subscriptions
            .iter()
            .filter(|(_, entry)| entry.kind == EventSourceKind::Netlink)
            .map(|(sub_id, entry)| (*sub_id, entry.manifest_id.clone()))
            .collect();
        for (sub_id, manifest_id) in targets {
            let context = netlink::format_netlink_context(&manifest_id, event);
            self.handle_event(events::Event {
                subscription_id: sub_id,
                context,
            });
        }
    }

    pub(super) fn clear_event_subscriptions(&mut self) {
        if let Some(dispatcher) = &self.event_dispatcher {
            for (id, entry) in &self.subscriptions {
                if entry.kind == EventSourceKind::Dbus {
                    dispatcher.unregister(*id);
                }
            }
        }
        self.subscriptions.clear();
    }

    pub(super) fn stop_event_dispatcher(&mut self) {
        if let Some(token) = self.event_source.take() {
            self.loop_handle.remove(token);
        }
        self.event_dispatcher.take();
    }

    pub(super) fn stop_pipewire_dispatcher(&mut self) {
        if let Some(token) = self.pipewire_event_source.take() {
            self.loop_handle.remove(token);
        }
        self.pipewire_dispatcher.take();
    }

    pub(super) fn register_bar_subscriptions(&mut self, bar_index: usize) {
        let Some(bar) = self.bars.get(bar_index) else {
            return;
        };
        let keys_and_states: Vec<_> = bar
            .widgets
            .iter()
            .filter_map(|widget| {
                let WidgetContent::Provider { key, state } = &widget.content else {
                    return None;
                };
                Some((key.clone(), state.clone()))
            })
            .collect();
        for (key, state) in keys_and_states {
            if self.subscriptions.values().any(|entry| entry.key == key) {
                continue;
            }
            let provider = state.borrow();
            let source_interval = provider.source_interval();
            for subscription in provider.triggers() {
                let id = self.next_subscription_id;
                self.next_subscription_id = self.next_subscription_id.saturating_add(1);
                match subscription.kind {
                    EventSourceKind::Dbus => {
                        let bus = subscription.bus.unwrap_or(EventBus::Session);
                        let rule = subscription.match_rule.clone().unwrap_or_default();
                        if let Some(dispatcher) = &self.event_dispatcher {
                            dispatcher.register(id, bus, rule, subscription.id.clone());
                        }
                    }
                    EventSourceKind::Netlink
                    | EventSourceKind::Uevent
                    | EventSourceKind::Pipewire
                    | EventSourceKind::Wayland
                    | EventSourceKind::Source => {}
                }
                self.subscriptions.insert(
                    id,
                    SubscriptionEntry {
                        key: key.clone(),
                        manifest_id: subscription.id.clone(),
                        kind: subscription.kind,
                        event: subscription.event.clone(),
                        subsystem: subscription.subsystem.clone(),
                        action: subscription.action.clone(),
                        source: subscription.source.clone(),
                        source_interval,
                    },
                );
            }
        }
    }

    pub(super) fn unregister_unused_providers(&mut self) {
        let mut active_keys = HashSet::new();
        for bar in &self.bars {
            for widget in &bar.widgets {
                if let WidgetContent::Provider { key, .. } = &widget.content {
                    active_keys.insert(key.clone());
                }
            }
        }
        let unused_keys: Vec<_> = self
            .providers
            .keys()
            .filter(|k| !active_keys.contains(*k))
            .cloned()
            .collect();
        for key in unused_keys {
            if let Some(state) = self.providers.remove(&key) {
                let mut state_mut = state.borrow_mut();
                if let ProviderState::Command(process) = &mut *state_mut {
                    if let Some(token) = process.source.take() {
                        self.loop_handle.remove(token);
                    }
                    if let Some(mut child) = process.child.take() {
                        terminate_process_group(&mut child);
                        let _ = child.wait();
                    }
                }
            }
            let ids: Vec<_> = self
                .subscriptions
                .iter()
                .filter_map(|(id, entry)| (entry.key == key).then_some(*id))
                .collect();
            for id in ids {
                if let Some(entry) = self.subscriptions.remove(&id)
                    && entry.kind == EventSourceKind::Dbus
                    && let Some(dispatcher) = &self.event_dispatcher
                {
                    dispatcher.unregister(id);
                }
            }
        }
        if !self
            .subscriptions
            .values()
            .any(|entry| entry.kind == EventSourceKind::Netlink)
        {
            self.stop_netlink_listener();
        }
        if !self
            .subscriptions
            .values()
            .any(|entry| entry.kind == EventSourceKind::Dbus)
        {
            self.stop_event_dispatcher();
        }
        if !self
            .subscriptions
            .values()
            .any(|entry| entry.kind == EventSourceKind::Uevent)
        {
            self.stop_uevent_listener();
        }
        if !self
            .subscriptions
            .values()
            .any(|entry| entry.kind == EventSourceKind::Pipewire)
        {
            self.stop_pipewire_dispatcher();
        }
        if !self
            .subscriptions
            .values()
            .any(|entry| entry.kind == EventSourceKind::Wayland && entry.event == "toplevel")
        {
            self.stop_toplevel_listener();
        }
        if !self
            .subscriptions
            .values()
            .any(|entry| entry.kind == EventSourceKind::Wayland && entry.event == "workspace")
        {
            self.stop_workspace_listener();
        }
    }
}
