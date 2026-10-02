use std::{
    io::{ErrorKind, Read},
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, ChildStdout, Command, Stdio},
    time::Instant,
};

use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
use smithay_client_toolkit::reexports::calloop::{
    Interest, Mode, PostAction,
    generic::Generic,
};

use crate::{
    config::substitute_context_placeholder,
    interaction::CallAction,
    runtime::{
        DEFAULT_TICK_DELAY, Runtime,
        model::{ProcessDefinition, ProviderKey, ProviderState},
        sources::SOURCE_BACKOFF_MAX,
    },
};

pub const MAX_PROCESS_MESSAGE_BYTES: usize = 64 * 1024;
pub const MAX_PROCESS_BYTES_PER_EVENT: usize = 64 * 1024;

impl Runtime {
    pub(super) fn start_due_processes(&mut self) {
        let now = Instant::now();
        let visible_keys = self.visible_provider_keys();
        let mut due = Vec::new();
        for (key, state) in &self.providers {
            if !visible_keys.contains(key) {
                continue;
            }
            let state_ref = state.borrow();
            let ProviderState::Command(process) = &*state_ref else {
                continue;
            };
            if process.child.is_none() && process.next_start.is_some_and(|start| start <= now) {
                due.push(key.clone());
            }
        }
        for key in due {
            self.spawn_process(&key);
        }
    }

    pub(super) fn spawn_process(&mut self, key: &ProviderKey) {
        let Some(state) = self.providers.get(key) else {
            return;
        };
        let (definition, pending_event) = {
            let state_ref = state.borrow();
            let ProviderState::Command(process) = &*state_ref else {
                return;
            };
            (process.definition.clone(), process.pending_event.clone())
        };
        let event = pending_event.as_deref().unwrap_or("null");
        let output_name = key.output.as_deref().unwrap_or("");
        let expected_parent = i32::try_from(std::process::id()).unwrap_or(i32::MAX);
        let command_args: Vec<_> = definition
            .command
            .iter()
            .map(|argument| {
                let with_context = substitute_context_placeholder(argument, event);
                with_context.replace("${output}", output_name)
            })
            .collect();

        let mut command = Command::new(&command_args[0]);
        if definition.base.is_dir() {
            command.current_dir(&definition.base);
        }
        command
            .args(&command_args[1..])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .process_group(0);
        unsafe {
            command.pre_exec(move || {
                use rustix::process::{Pid, Signal, getppid, set_parent_process_death_signal};
                set_parent_process_death_signal(Some(Signal::KILL))?;
                if getppid().map(Pid::as_raw_pid) != Some(expected_parent) {
                    return Err(std::io::Error::other(
                        "provider parent exited before startup",
                    ));
                }
                Ok(())
            });
        }

        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                eprintln!(
                    "cellbar: cannot start widget {:?} (program: {:?}, cwd: {:?}): {error}",
                    definition.id,
                    command_args.first(),
                    definition.base,
                );
                self.schedule_process_failure(key);
                return;
            }
        };
        let Some(stdout) = child.stdout.take() else {
            terminate_process_group(&mut child);
            let _ = child.wait();
            self.schedule_process_failure(key);
            return;
        };
        if let Err(error) = set_nonblocking(&stdout) {
            eprintln!(
                "cellbar: cannot configure stdout for widget {:?}: {error}",
                definition.id
            );
            terminate_process_group(&mut child);
            let _ = child.wait();
            self.schedule_process_failure(key);
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
                            runtime.handle_process_eof(&callback_key);
                            return Ok(PostAction::Remove);
                        }
                        Ok(count) => {
                            processed += count;
                            runtime.handle_process_bytes(&callback_key, &bytes[..count]);
                            if processed >= MAX_PROCESS_BYTES_PER_EVENT {
                                return Ok(PostAction::Continue);
                            }
                        }
                        Err(error) if error.kind() == ErrorKind::WouldBlock => {
                            return Ok(PostAction::Continue);
                        }
                        Err(error) => {
                            eprintln!("cellbar: widget stdout read failed: {error}");
                            runtime.handle_process_eof(&callback_key);
                            return Ok(PostAction::Remove);
                        }
                    }
                }
            },
        ) {
            Ok(token) => token,
            Err(error) => {
                eprintln!(
                    "cellbar: cannot monitor widget {:?}: {error}",
                    definition.id
                );
                terminate_process_group(&mut child);
                let _ = child.wait();
                self.schedule_process_failure(key);
                return;
            }
        };

        if let Some(state) = self.providers.get(key) {
            let mut state_mut = state.borrow_mut();
            if let ProviderState::Command(process) = &mut *state_mut {
                process.child = Some(child);
                process.source = Some(token);
                process.started_at = Some(Instant::now());
                process.next_start = None;
                process.pending_event = None;
                process.refresh_requested = false;
                process.pending.clear();
                process.discarding_oversized_line = false;
            } else {
                self.loop_handle.remove(token);
                terminate_process_group(&mut child);
                let _ = child.wait();
            }
        } else {
            self.loop_handle.remove(token);
            terminate_process_group(&mut child);
            let _ = child.wait();
        }
    }

    pub(super) fn schedule_process_failure(&mut self, key: &ProviderKey) {
        let Some(state) = self.providers.get(key) else {
            return;
        };
        let mut state_mut = state.borrow_mut();
        let ProviderState::Command(process) = &mut *state_mut else {
            return;
        };
        let delay = process
            .definition
            .every
            .unwrap_or(DEFAULT_TICK_DELAY)
            .min(SOURCE_BACKOFF_MAX);
        process.next_start = Some(Instant::now() + delay);
    }

    pub(super) fn handle_process_bytes(&mut self, key: &ProviderKey, bytes: &[u8]) {
        let mut lines = Vec::new();
        let definition = {
            let Some(state) = self.providers.get(key) else {
                return;
            };
            let mut state_mut = state.borrow_mut();
            let ProviderState::Command(process) = &mut *state_mut else {
                return;
            };
            for byte in bytes {
                if process.discarding_oversized_line {
                    if *byte == b'\n' {
                        process.discarding_oversized_line = false;
                    }
                    continue;
                }
                if *byte == b'\n' {
                    let mut line = std::mem::take(&mut process.pending);
                    if line.last() == Some(&b'\r') {
                        line.pop();
                    }
                    lines.push(line);
                } else if process.pending.len() == MAX_PROCESS_MESSAGE_BYTES {
                    process.pending.clear();
                    process.discarding_oversized_line = true;
                    eprintln!(
                        "cellbar: discarded oversized output from widget {:?}",
                        process.definition.id
                    );
                } else {
                    process.pending.push(*byte);
                }
            }
            process.definition.clone()
        };
        self.apply_process_lines(key, &definition, lines);
    }

    pub(super) fn handle_process_eof(&mut self, key: &ProviderKey) {
        let (definition, final_line) = {
            let Some(state) = self.providers.get(key) else {
                return;
            };
            let mut state_mut = state.borrow_mut();
            let ProviderState::Command(process) = &mut *state_mut else {
                return;
            };
            process.source = None;
            let final_line = (!process.discarding_oversized_line && !process.pending.is_empty())
                .then(|| std::mem::take(&mut process.pending));
            process.discarding_oversized_line = false;
            (process.definition.clone(), final_line)
        };
        if let Some(line) = final_line {
            self.apply_process_lines(key, &definition, vec![line]);
        }
        self.reap_processes();
        self.start_due_processes();
        #[cfg(target_os = "linux")]
        unsafe {
            libc::malloc_trim(0);
        }
    }

    pub(super) fn apply_process_lines(
        &mut self,
        key: &ProviderKey,
        definition: &ProcessDefinition,
        lines: Vec<Vec<u8>>,
    ) {
        let mut latest = None;
        for line in lines {
            match crate::runtime::model::decode_process_spans(&line, &definition.base) {
                Ok(value) => latest = Some(value),
                Err(error) => eprintln!(
                    "cellbar: ignored output from widget {:?}: {error}",
                    definition.id
                ),
            }
        }
        let Some(latest) = latest else {
            return;
        };
        let (changed, image_paths) = {
            let Some(state) = self.providers.get(key) else {
                return;
            };
            let mut state_mut = state.borrow_mut();
            let ProviderState::Command(process) = &mut *state_mut else {
                return;
            };
            let text = latest
                .iter()
                .filter_map(|span| match &span.part {
                    crate::markup::MarkupPart::Text(text) => Some(text.as_str()),
                    crate::markup::MarkupPart::Image(_) => None,
                })
                .collect::<String>();
            let rich = Some(latest);
            let old_sources: Vec<PathBuf> = process
                .rich
                .as_deref()
                .unwrap_or_default()
                .iter()
                .filter_map(|span| match &span.part {
                    crate::markup::MarkupPart::Image(image) => Some(image.src.clone()),
                    crate::markup::MarkupPart::Text(_) => None,
                })
                .collect();
            let new_sources: Vec<PathBuf> = rich
                .as_deref()
                .unwrap_or_default()
                .iter()
                .filter_map(|span| match &span.part {
                    crate::markup::MarkupPart::Image(image) => Some(image.src.clone()),
                    crate::markup::MarkupPart::Text(_) => None,
                })
                .collect();
            let image_paths = if old_sources == new_sources {
                Vec::new()
            } else {
                new_sources
            };
            let changed = process.value != text || process.rich != rich;
            if changed {
                process.value = text;
                process.rich = rich;
            }
            (changed, image_paths)
        };
        if let Some(store) = self.renderer.images.as_mut() {
            for path in image_paths {
                store.refresh(&path);
            }
        }
        let _ = changed;
        self.mark_bars_with_provider_dirty(key);
    }

    pub(super) fn reap_processes(&mut self) {
        let now = Instant::now();
        let mut exited = Vec::new();
        for (key, state) in &self.providers {
            let mut state_mut = state.borrow_mut();
            let ProviderState::Command(process) = &mut *state_mut else {
                continue;
            };
            let Some(child) = process.child.as_mut() else {
                continue;
            };
            let timed_out = process
                .definition
                .timeout
                .zip(process.started_at)
                .is_some_and(|(timeout, started)| now.duration_since(started) >= timeout);
            if timed_out {
                terminate_process_group(child);
                exited.push((key.clone(), child.id()));
                continue;
            }
            if process.source.is_some() {
                continue;
            }
            match child.try_wait() {
                Ok(Some(_)) => exited.push((key.clone(), child.id())),
                Ok(None) => {}
                Err(error) => eprintln!(
                    "cellbar: cannot inspect widget {:?}: {error}",
                    process.definition.id
                ),
            }
        }
        for (key, _pid) in exited {
            let (token, next_start) = {
                let Some(state) = self.providers.get(&key) else {
                    continue;
                };
                let mut state_mut = state.borrow_mut();
                let ProviderState::Command(process) = &mut *state_mut else {
                    continue;
                };
                let token = process.source.take();
                process.child = None;
                process.started_at = None;
                let next_start = if std::mem::take(&mut process.refresh_requested) {
                    process
                        .next_start
                        .filter(|start| *start > now)
                        .or(Some(now))
                } else {
                    process
                        .definition
                        .every
                        .and_then(|interval| now.checked_add(interval))
                };
                process.next_start = next_start;
                (token, next_start)
            };
            if let Some(token) = token {
                self.loop_handle.remove(token);
            }
            if let Some(start) = next_start
                && start > now
            {
                self.schedule_event_debounce(start);
            }
        }
    }

    pub(super) fn stop_all_processes(&mut self) {
        let mut tokens = Vec::new();
        for state in self.providers.values() {
            let mut state_mut = state.borrow_mut();
            let ProviderState::Command(process) = &mut *state_mut else {
                continue;
            };
            if let Some(token) = process.source.take() {
                tokens.push(token);
            }
            if let Some(mut child) = process.child.take() {
                terminate_process_group(&mut child);
                let _ = child.wait();
            }
            process.started_at = None;
        }
        for token in tokens {
            self.loop_handle.remove(token);
        }
    }

    pub(super) fn execute_action(&mut self, action: &CallAction) {
        match action {
            CallAction::Command {
                program,
                args,
                refresh_target,
            } => {
                let mut cmd = std::process::Command::new(program);
                if let Some(parent) = Path::new(program).parent()
                    && parent.is_dir()
                {
                    cmd.current_dir(parent);
                }
                cmd.args(args);
                cmd.stdin(std::process::Stdio::null());
                cmd.stdout(std::process::Stdio::null());
                cmd.stderr(std::process::Stdio::inherit());
                cmd.process_group(0);
                match cmd.spawn() {
                    Ok(mut child) => {
                        let program = program.clone();
                        std::thread::spawn(move || {
                            if let Ok(status) = child.wait()
                                && !status.success()
                            {
                                eprintln!("cellbar: action {program} exited with {status}");
                            }
                        });
                    }
                    Err(error) => {
                        eprintln!("cellbar: failed to execute action {program} {args:?}: {error}");
                    }
                }
                if let Some(target) = refresh_target {
                    self.refresh_widget(target);
                }
            }
            CallAction::RefreshWidget(target) => {
                self.refresh_widget(target);
            }
        }
    }
}

pub(super) fn set_nonblocking(stdout: &ChildStdout) -> std::io::Result<()> {
    let flags = fcntl_getfl(stdout)?;
    Ok(fcntl_setfl(stdout, flags | OFlags::NONBLOCK)?)
}

pub(super) fn terminate_process_group(child: &mut Child) {
    use rustix::process::{Pid, Signal, kill_process_group};

    let pid = i32::try_from(child.id()).ok().and_then(Pid::from_raw);
    if pid.is_none_or(|pid| kill_process_group(pid, Signal::KILL).is_err()) {
        let _ = child.kill();
    }
}
