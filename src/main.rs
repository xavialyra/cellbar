use std::{
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::ExitCode,
};

use cellbar::{
    config::{Config, default_path, normalize_path},
    control, runtime,
};

#[cfg(feature = "dhat-heap")]
#[global_allocator]
static DHAT_ALLOCATOR: dhat::Alloc = dhat::Alloc;

fn main() -> ExitCode {
    #[cfg(target_os = "linux")]
    unsafe {
        libc::mallopt(libc::M_ARENA_MAX, 1);
        libc::mallopt(libc::M_TRIM_THRESHOLD, 64 * 1024);
        libc::mallopt(libc::M_MMAP_THRESHOLD, 64 * 1024);
    }

    match run(env::args_os().skip(1).collect()) {
        Ok(code) => code,
        Err(message) => {
            eprintln!("cellbar: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(arguments: Vec<OsString>) -> Result<ExitCode, String> {
    match arguments.as_slice() {
        [command] if command == "--help" || command == "-h" => {
            print_usage();
            Ok(ExitCode::SUCCESS)
        }
        [command] if command == "check" => check(None).map(|()| ExitCode::SUCCESS),
        [command, path] if command == "check" => check(Some(PathBuf::from(path))).map(|()| ExitCode::SUCCESS),
        [command] if command == "init" => init(None).map(|()| ExitCode::SUCCESS),
        [command, path] if command == "init" => init(Some(PathBuf::from(path))).map(|()| ExitCode::SUCCESS),
        [command] if command == "list" => {
            let value = control::request("list")?;
            let table = control::format_bar_list(&value);
            if !table.is_empty() {
                println!("{table}");
            }
            Ok(ExitCode::SUCCESS)
        }
        [command, target_id]
            if (command == "is-visible" || command == "is-hidden") && !target_id.is_empty() =>
        {
            let cmd = command.to_string_lossy();
            let bar_id = target_id
                .to_str()
                .ok_or_else(|| "bar id must be valid UTF-8".to_owned())?;
            let value = control::request(&format!("{cmd} {bar_id}"))?;
            let is_match = value.get("match").and_then(serde_json::Value::as_bool).unwrap_or(false);
            if is_match {
                println!("true");
                Ok(ExitCode::SUCCESS)
            } else {
                println!("false");
                Ok(ExitCode::from(1))
            }
        }
        [command]
            if command == "status"
                || command == "reload"
                || command == "hide"
                || command == "show"
                || command == "toggle" =>
        {
            control_command(command.to_string_lossy().as_ref()).map(|()| ExitCode::SUCCESS)
        }
        [command, target_id]
            if (command == "refresh"
                || command == "clear"
                || command == "hide"
                || command == "show"
                || command == "toggle")
                && !target_id.is_empty() =>
        {
            let cmd = command.to_string_lossy();
            let target_id = target_id
                .to_str()
                .ok_or_else(|| "target id must be valid UTF-8".to_owned())?;
            control_command(&format!("{cmd} {target_id}")).map(|()| ExitCode::SUCCESS)
        }
        [command, widget_id, markup @ ..]
            if command == "push" && !widget_id.is_empty() && !markup.is_empty() =>
        {
            let widget_id = widget_id
                .to_str()
                .ok_or_else(|| "widget id must be valid UTF-8".to_owned())?;
            let markup_str = markup
                .iter()
                .map(|arg| arg.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ");
            control_command(&format!("push {widget_id} {markup_str}")).map(|()| ExitCode::SUCCESS)
        }
        [command, event_id, payload @ ..] if command == "emit" && !event_id.is_empty() => {
            let event_id = event_id
                .to_str()
                .ok_or_else(|| "event id must be valid UTF-8".to_owned())?;
            let payload = payload
                .iter()
                .map(|arg| arg.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ");
            let full_command = if payload.is_empty() {
                format!("emit {event_id}")
            } else {
                format!("emit {event_id} {payload}")
            };
            control_command(&full_command).map(|()| ExitCode::SUCCESS)
        }
        [flag, path] if flag == "-c" || flag == "--config" => start(Some(PathBuf::from(path))).map(|()| ExitCode::SUCCESS),
        [path] if !path.to_string_lossy().starts_with('-') => start(Some(PathBuf::from(path))).map(|()| ExitCode::SUCCESS),
        [] => start(None).map(|()| ExitCode::SUCCESS),
        _ => Err("usage: cellbar [-c PATH | PATH | check [PATH] | init [PATH] | status | list | reload | hide [bar-id] | show [bar-id] | toggle [bar-id] | is-visible <bar-id> | is-hidden <bar-id> | refresh <widget-id> | push <widget-id> <markup> | clear <widget-id> | emit <event-id> [data]]".into()),
    }
}

fn start(path: Option<PathBuf>) -> Result<(), String> {
    #[cfg(feature = "dhat-heap")]
    let _heap_profiler = start_heap_profiler();
    #[cfg(not(feature = "dhat-heap"))]
    start_heap_profiler();
    let path = match path {
        Some(path) => path,
        None => default_path().map_err(|error| error.to_string())?,
    };
    let path = normalize_path(&path);
    let config = Config::load(&path).map_err(|error| error.display_with_path(&path))?;
    runtime::run(config, path).map_err(|error| error.to_string())
}

#[cfg(feature = "dhat-heap")]
fn start_heap_profiler() -> dhat::Profiler {
    let file_name = OsString::from("target/memory/dhat-heap.json");
    dhat::Profiler::builder().file_name(file_name).build()
}

#[cfg(not(feature = "dhat-heap"))]
fn start_heap_profiler() {}

fn control_command(command: &str) -> Result<(), String> {
    let value = control::request(command)?;
    if command == "status" {
        println!(
            "{}",
            serde_json::to_string_pretty(&value).map_err(|error| error.to_string())?
        );
    } else if let Some(message) = value.get("message").and_then(serde_json::Value::as_str) {
        println!("{message}");
    } else if let Some(instances) = value.get("instances").and_then(serde_json::Value::as_u64) {
        println!("refreshed {instances} instance(s)");
    } else if let Some(emitted) = value.get("emitted").and_then(serde_json::Value::as_u64) {
        println!("emitted to {emitted} subscription(s)");
    }
    Ok(())
}

fn check(path: Option<PathBuf>) -> Result<(), String> {
    let path = match path {
        Some(path) => path,
        None => default_path().map_err(|error| error.to_string())?,
    };
    let path = normalize_path(&path);
    let config = Config::load(&path).map_err(|error| error.display_with_path(&path))?;
    print_check_diagnostics(&config, &path);
    println!("{}: valid", path.display());
    Ok(())
}

fn init(path: Option<PathBuf>) -> Result<(), String> {
    let path = match path {
        Some(path) => path,
        None => default_path()
            .map_err(|error| error.to_string())?
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| "cannot determine the XDG Cellbar configuration directory".to_owned())?,
    };
    if path.exists() {
        return Err(format!(
            "refusing to initialize existing path {}",
            path.display()
        ));
    }
    fs::create_dir_all(path.join("components/clock"))
        .and_then(|_| fs::create_dir_all(path.join("components/memory")))
        .and_then(|_| fs::create_dir_all(path.join("components/cpu")))
        .and_then(|_| fs::create_dir_all(path.join("components/window")))
        .map_err(|error| format!("cannot create {}: {error}", path.display()))?;
    write_init_file(&path.join("config.toml"), INIT_CONFIG)?;
    write_init_file(&path.join("theme.toml"), INIT_THEME)?;
    write_init_file(&path.join("components/clock/manifest.toml"), INIT_CLOCK)?;
    write_init_file(&path.join("components/memory/manifest.toml"), INIT_MEMORY)?;
    write_init_file(&path.join("components/cpu/manifest.toml"), INIT_CPU)?;
    write_init_file(&path.join("components/window/manifest.toml"), INIT_WINDOW)?;
    println!("initialized {}", path.display());
    Ok(())
}

fn write_init_file(path: &Path, contents: &str) -> Result<(), String> {
    fs::write(path, contents).map_err(|error| format!("cannot write {}: {error}", path.display()))
}

fn print_check_diagnostics(config: &Config, config_path: &Path) {
    for (id, provider) in &config.providers {
        if let Some(program) = &provider.program {
            let available = if program.contains('/') {
                Path::new(program).exists()
            } else {
                env::var_os("PATH")
                    .map(|path| env::split_paths(&path).any(|dir| dir.join(program).is_file()))
                    .unwrap_or(false)
            };
            if !available {
                eprintln!("warning: component {id}: program not found: {program}");
            }
        }
    }
    if config_path.parent().is_none() {
        eprintln!("warning: configuration has no parent directory");
    }
}

const INIT_CONFIG: &str = include_str!("../examples/minimal/config.toml");
const INIT_THEME: &str = include_str!("../examples/minimal/theme.toml");
const INIT_CLOCK: &str = include_str!("../examples/minimal/components/clock/manifest.toml");
const INIT_MEMORY: &str = include_str!("../examples/minimal/components/memory/manifest.toml");
const INIT_CPU: &str = include_str!("../examples/minimal/components/cpu/manifest.toml");
const INIT_WINDOW: &str = include_str!("../examples/minimal/components/window/manifest.toml");

fn print_usage() {
    println!(
        "Cellbar Wayland topbar\n\nUsage:\n  cellbar [-c PATH | PATH]\n  cellbar check [PATH]\n  cellbar init [PATH]\n  cellbar status\n  cellbar list\n  cellbar reload\n  cellbar hide [bar-id]\n  cellbar show [bar-id]\n  cellbar toggle [bar-id]\n  cellbar is-visible <bar-id>\n  cellbar is-hidden <bar-id>\n  cellbar refresh <widget-id>\n  cellbar push <widget-id> <markup>\n  cellbar clear <widget-id>\n  cellbar emit <event-id> [data]\n  cellbar --help"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_copies_minimal_example_and_refuses_overwrite() {
        let root = env::temp_dir().join(format!("cellbar-init-test-{}", std::process::id()));
        if root.exists() {
            fs::remove_dir_all(&root).unwrap();
        }
        run(vec!["init".into(), root.as_os_str().to_owned()]).expect("init command");
        assert_eq!(
            fs::read_to_string(root.join("config.toml")).unwrap(),
            INIT_CONFIG
        );
        assert_eq!(
            fs::read_to_string(root.join("theme.toml")).unwrap(),
            INIT_THEME
        );
        let config = Config::load(root.join("config.toml")).expect("generated config validates");
        assert_eq!(config.components.len(), 4);
        assert!(run(vec!["init".into(), root.as_os_str().to_owned()]).is_err());
        assert_eq!(
            fs::read_to_string(root.join("config.toml")).unwrap(),
            INIT_CONFIG
        );
        fs::remove_dir_all(root).unwrap();
    }
}
