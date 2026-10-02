use std::{
    env,
    ffi::OsString,
    fs,
    os::unix::{fs::PermissionsExt, net::UnixDatagram},
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde_json::Value;
use thiserror::Error;

pub const SOCKET_NAME: &str = "control.sock";
pub const MAX_COMMAND_BYTES: usize = 4096;

pub fn runtime_directory() -> Result<PathBuf, ControlPathError> {
    if let Some(directory) = nonempty_env("XDG_RUNTIME_DIR") {
        return Ok(PathBuf::from(directory).join("cellbar"));
    }
    Err(ControlPathError)
}

pub fn socket_path() -> Result<PathBuf, ControlPathError> {
    Ok(runtime_directory()?.join(SOCKET_NAME))
}

pub fn request(command: &str) -> Result<Value, String> {
    let runtime_directory = runtime_directory().map_err(|error| error.to_string())?;
    let server_path = socket_path().map_err(|error| error.to_string())?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    let client_path =
        runtime_directory.join(format!(".cellbar-{}-{nonce}.sock", std::process::id()));
    let _cleanup = SocketCleanup(client_path.clone());

    let socket = UnixDatagram::bind(&client_path)
        .map_err(|error| format!("cannot create client socket: {error}"))?;
    fs::set_permissions(&client_path, fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("cannot secure client socket: {error}"))?;
    socket
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|error| error.to_string())?;
    socket
        .send_to(command.as_bytes(), &server_path)
        .map_err(|error| format!("cannot contact {}: {error}", server_path.display()))?;

    let mut response = vec![0; MAX_COMMAND_BYTES];
    let count = socket
        .recv(&mut response)
        .map_err(|error| format!("no response from Cellbar: {error}"))?;
    let value: Value = serde_json::from_slice(&response[..count])
        .map_err(|error| format!("invalid response from Cellbar: {error}"))?;
    if value.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(value
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("control command failed")
            .to_owned());
    }
    Ok(value)
}

pub fn format_bar_list(value: &Value) -> String {
    let Some(bars) = value.get("bars").and_then(Value::as_array) else {
        return String::new();
    };
    if bars.is_empty() {
        return "no active bars".to_owned();
    }
    let mut rows = Vec::with_capacity(bars.len() + 1);
    rows.push((
        "BAR".to_owned(),
        "OUTPUT".to_owned(),
        "POSITION".to_owned(),
        "LAYER".to_owned(),
        "STATUS".to_owned(),
    ));
    for bar in bars {
        let id = bar.get("id").and_then(Value::as_str).unwrap_or("-");
        let output = bar.get("output").and_then(Value::as_str).unwrap_or("-");
        let position = bar.get("position").and_then(Value::as_str).unwrap_or("-");
        let layer = bar.get("layer").and_then(Value::as_str).unwrap_or("-");
        let hidden = bar.get("hidden").and_then(Value::as_bool).unwrap_or(false);
        let status = if hidden { "hidden" } else { "visible" };
        rows.push((
            id.to_owned(),
            output.to_owned(),
            position.to_owned(),
            layer.to_owned(),
            status.to_owned(),
        ));
    }

    let w_id = rows.iter().map(|r| r.0.len()).max().unwrap_or(3);
    let w_out = rows.iter().map(|r| r.1.len()).max().unwrap_or(6);
    let w_pos = rows.iter().map(|r| r.2.len()).max().unwrap_or(8);
    let w_lay = rows.iter().map(|r| r.3.len()).max().unwrap_or(5);

    let mut out = String::new();
    for (idx, (id, output, position, layer, status)) in rows.into_iter().enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        out.push_str(&format!(
            "{id:<w_id$}  {output:<w_out$}  {position:<w_pos$}  {layer:<w_lay$}  {status}"
        ));
    }
    out
}

fn nonempty_env(name: &str) -> Option<OsString> {
    env::var_os(name).filter(|value| !value.is_empty())
}

#[derive(Debug, Clone, Copy, Error)]
#[error("cannot determine control socket path: set XDG_RUNTIME_DIR")]
pub struct ControlPathError;

struct SocketCleanup(PathBuf);

impl Drop for SocketCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_name_is_stable() {
        assert_eq!(
            PathBuf::from("/tmp/cellbar").join(SOCKET_NAME),
            PathBuf::from("/tmp/cellbar/control.sock")
        );
    }

    #[test]
    fn formats_bar_list_table() {
        let json = serde_json::json!({
            "bars": [
                {
                    "id": "top",
                    "output": "DP-1",
                    "position": "top",
                    "layer": "top",
                    "hidden": false
                },
                {
                    "id": "drawer",
                    "output": "DP-1",
                    "position": "bottom",
                    "layer": "overlay",
                    "hidden": true
                }
            ]
        });
        let table = format_bar_list(&json);
        assert!(table.contains("BAR     OUTPUT  POSITION  LAYER    STATUS"));
        assert!(table.contains("top     DP-1    top       top      visible"));
        assert!(table.contains("drawer  DP-1    bottom    overlay  hidden"));
    }
}
