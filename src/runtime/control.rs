use std::{
    fs,
    io::ErrorKind,
    os::unix::{
        fs::{FileTypeExt, PermissionsExt},
        net::UnixDatagram,
    },
    path::Path,
};

use crate::{
    events,
    runtime::{Runtime, RuntimeError},
};

impl Runtime {
    pub(super) fn handle_control_command(&mut self, command: &[u8]) -> String {
        let Ok(command) = std::str::from_utf8(command) else {
            return control_error("command is not valid UTF-8");
        };
        let trimmed = command.trim();
        if let Some(rest) = trimmed.strip_prefix("push") {
            let rest = rest.trim_start();
            if let Some((widget_id, markup)) = rest.split_once(char::is_whitespace) {
                let widget_id = widget_id.trim();
                let markup = markup.trim();
                return match self.push_widget_markup(widget_id, markup) {
                    Ok(instances) => format!(
                        "{}\n",
                        serde_json::json!({"ok": true, "instances": instances})
                    ),
                    Err(error) => control_error(&error),
                };
            } else {
                return control_error("usage: push <widget-id> <markup>");
            }
        }
        if let Some(rest) = trimmed.strip_prefix("clear") {
            let widget_id = rest.trim();
            if widget_id.is_empty() {
                return control_error("usage: clear <widget-id>");
            }
            return match self.clear_widget_markup(widget_id) {
                Ok(instances) => format!(
                    "{}\n",
                    serde_json::json!({"ok": true, "instances": instances})
                ),
                Err(error) => control_error(&error),
            };
        }
        let parts: Vec<_> = trimmed.split_whitespace().collect();
        match parts.as_slice() {
            ["status"] => {
                let outputs: Vec<_> = self
                    .bars
                    .iter()
                    .map(|bar| {
                        serde_json::json!({
                            "name": bar.output_name,
                            "id": bar.name,
                            "position": bar.position.as_str(),
                            "layer": bar.layer_level.as_str(),
                            "hidden": bar.hidden,
                            "scale": bar.scale,
                            "configured": bar.configured,
                            "widgets": bar.widgets.iter().map(|w| serde_json::json!({
                                "id": w.id,
                                "text": w.text(),
                            })).collect::<Vec<_>>(),
                        })
                    })
                    .collect();
                format!(
                    "{}\n",
                    serde_json::json!({
                        "ok": true,
                        "pid": std::process::id(),
                        "config": self.config_path,
                        "outputs": outputs,
                    })
                )
            }
            ["list"] => {
                let bars: Vec<_> = self
                    .bars
                    .iter()
                    .map(|bar| {
                        serde_json::json!({
                            "id": bar.name.as_deref().unwrap_or("-"),
                            "output": bar.output_name,
                            "position": bar.position.as_str(),
                            "layer": bar.layer_level.as_str(),
                            "hidden": bar.hidden,
                        })
                    })
                    .collect();
                format!("{}\n", serde_json::json!({"ok": true, "bars": bars}))
            }
            ["reload"] => {
                self.reload_requested = true;
                control_ok("reload scheduled")
            }
            ["refresh", widget_id] => {
                let instances = self.refresh_widget(widget_id);
                if instances == 0 {
                    control_error(&format!("unknown widget {widget_id:?}"))
                } else {
                    format!(
                        "{}\n",
                        serde_json::json!({"ok": true, "instances": instances})
                    )
                }
            }
            ["emit", event_name, rest @ ..] => {
                let context = if rest.is_empty() {
                    "null".to_owned()
                } else {
                    rest.join(" ")
                };
                let matching: Vec<_> = self
                    .subscriptions
                    .iter()
                    .filter(|(_, entry)| entry.manifest_id == *event_name)
                    .map(|(id, _)| *id)
                    .collect();
                if matching.is_empty() {
                    control_error(&format!("no active subscriptions for event {event_name:?}"))
                } else {
                    let count = matching.len();
                    for id in matching {
                        self.handle_event(events::Event {
                            subscription_id: id,
                            context: context.clone(),
                        });
                    }
                    format!("{}\n", serde_json::json!({"ok": true, "emitted": count}))
                }
            }
            ["hide"] => {
                self.set_bars_hidden(None, true);
                control_ok("all bars hidden")
            }
            ["hide", bar_id] => {
                if !self.bar_exists(bar_id) {
                    control_error(&format!("unknown bar {bar_id:?}"))
                } else {
                    self.set_bars_hidden(Some(bar_id), true);
                    control_ok(&format!("bar {bar_id:?} hidden"))
                }
            }
            ["show"] => {
                self.set_bars_hidden(None, false);
                control_ok("all bars shown")
            }
            ["show", bar_id] => {
                if !self.bar_exists(bar_id) {
                    control_error(&format!("unknown bar {bar_id:?}"))
                } else {
                    self.set_bars_hidden(Some(bar_id), false);
                    control_ok(&format!("bar {bar_id:?} shown"))
                }
            }
            ["toggle"] => {
                let any_visible = self.bars.iter().any(|b| !b.hidden);
                self.set_bars_hidden(None, any_visible);
                if any_visible {
                    control_ok("bars hidden")
                } else {
                    control_ok("bars shown")
                }
            }
            ["toggle", bar_id] => {
                let currently_hidden = self
                    .bars
                    .iter()
                    .find(|b| b.name.as_deref() == Some(*bar_id))
                    .map(|b| b.hidden);
                let Some(hidden) = currently_hidden else {
                    return control_error(&format!("unknown bar {bar_id:?}"));
                };
                self.set_bars_hidden(Some(bar_id), !hidden);
                if hidden {
                    control_ok(&format!("bar {bar_id:?} shown"))
                } else {
                    control_ok(&format!("bar {bar_id:?} hidden"))
                }
            }
            ["is-visible", bar_id] => {
                if !self.bar_exists(bar_id) {
                    control_error(&format!("unknown bar {bar_id:?}"))
                } else {
                    let visible = if self.bars.iter().any(|b| b.name.as_deref() == Some(*bar_id)) {
                        self.bars
                            .iter()
                            .any(|b| b.name.as_deref() == Some(*bar_id) && !b.hidden)
                    } else {
                        self.bar_specs
                            .iter()
                            .any(|s| s.name.as_deref() == Some(*bar_id) && !s.initially_hidden)
                    };
                    format!("{}\n", serde_json::json!({"ok": true, "match": visible}))
                }
            }
            ["is-hidden", bar_id] => {
                if !self.bar_exists(bar_id) {
                    control_error(&format!("unknown bar {bar_id:?}"))
                } else {
                    let hidden = if self.bars.iter().any(|b| b.name.as_deref() == Some(*bar_id)) {
                        self.bars
                            .iter()
                            .filter(|b| b.name.as_deref() == Some(*bar_id))
                            .all(|b| b.hidden)
                    } else {
                        self.bar_specs
                            .iter()
                            .filter(|s| s.name.as_deref() == Some(*bar_id))
                            .all(|s| s.initially_hidden)
                    };
                    format!("{}\n", serde_json::json!({"ok": true, "match": hidden}))
                }
            }
            _ => control_error(
                "usage: status | list | reload | hide [bar-id] | show [bar-id] | toggle [bar-id] | is-visible <bar-id> | is-hidden <bar-id> | refresh <widget-id> | push <widget-id> <markup> | clear <widget-id> | emit <event-id> [data]",
            ),
        }
    }
}

pub(super) fn bind_control_socket(path: &Path) -> Result<UnixDatagram, RuntimeError> {
    let directory = path
        .parent()
        .ok_or_else(|| RuntimeError::ControlSocket("socket path has no parent".into()))?;
    fs::create_dir_all(directory)
        .map_err(|error| RuntimeError::ControlSocket(error.to_string()))?;
    fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
        .map_err(|error| RuntimeError::ControlSocket(error.to_string()))?;

    if let Ok(metadata) = fs::symlink_metadata(path) {
        if !metadata.file_type().is_socket() {
            return Err(RuntimeError::ControlSocket(format!(
                "refusing to replace non-socket {}",
                path.display()
            )));
        }
        let probe = UnixDatagram::unbound()
            .map_err(|error| RuntimeError::ControlSocket(error.to_string()))?;
        match probe.send_to(b"status", path) {
            Ok(_) => {
                return Err(RuntimeError::ControlSocket(format!(
                    "another Cellbar instance is using {}",
                    path.display()
                )));
            }
            Err(error) if error.kind() == ErrorKind::ConnectionRefused => {
                fs::remove_file(path)
                    .map_err(|error| RuntimeError::ControlSocket(error.to_string()))?;
            }
            Err(error) => return Err(RuntimeError::ControlSocket(error.to_string())),
        }
    }

    let socket =
        UnixDatagram::bind(path).map_err(|error| RuntimeError::ControlSocket(error.to_string()))?;
    socket
        .set_nonblocking(true)
        .map_err(|error| RuntimeError::ControlSocket(error.to_string()))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|error| RuntimeError::ControlSocket(error.to_string()))?;
    Ok(socket)
}

pub(super) fn control_ok(message: &str) -> String {
    format!("{}\n", serde_json::json!({"ok": true, "message": message}))
}

pub(super) fn control_error(message: &str) -> String {
    format!("{}\n", serde_json::json!({"ok": false, "error": message}))
}
