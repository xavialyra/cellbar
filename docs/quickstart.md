# Quickstart

[README](../README.md) · [Configuration](configuration.md)

Run these commands from the project root. Launch Cellbar inside a Wayland
session whose compositor supports layer-shell.

## Installation

### Option A: Direct Download (Recommended)

Download the pre-compiled binary directly to your local bin path:

```sh
curl -fsSL https://github.com/xavialyra/cellbar/releases/latest/download/cellbar-linux-x86_64 -o ~/.local/bin/cellbar
chmod +x ~/.local/bin/cellbar
```

### Option B: Build from Source

Install mise (or Rust/Cargo), a C compiler (`gcc`), `pkg-config`, `libsystemd`, and PipeWire development libraries using your system's package manager:

```sh
git clone https://github.com/xavialyra/cellbar.git
cd cellbar
cargo build --release
# Or using mise: mise run build

install -Dm755 target/release/cellbar ~/.local/bin/cellbar
```

## Initialize and run

Initialize a ready-to-run configuration, validate it, and launch:

```sh
cellbar init
cellbar check
cellbar
```

`init` writes a starter configuration to `$XDG_CONFIG_HOME/cellbar` (or
`$HOME/.config/cellbar` when `XDG_CONFIG_HOME` is unset). It refuses to
overwrite an existing directory.

The embedded starter configuration uses the minimal template:
- Layout, theme, and in-process components (`clock`, `cpu`, `memory`, `window`).
- Zero external scripts, zero icon font requirements, zero shell forks.
- Ready to customize immediately.

## Presets and examples

Ready-made configuration presets (`flat` and `capsules`) with complete layout, theme, and component manifests are available in the [`examples/`](../examples) directory of the repository.

## Validate and reload

After editing your configuration, validate it before restarting:

```sh
cellbar check
```

While Cellbar is running in the background, reload it seamlessly without losing
window focus or tearing:

```sh
cellbar reload
```

An invalid reload leaves the active configuration in place. Inspect bar status
and active displays with:

```sh
cellbar status
cellbar list
```

Other CLI commands are described in [Control](configuration.md#control).

## Next steps

- Explore [Configuration](configuration.md) for layout syntax, theme palettes, and font fallbacks.
- Learn about [Expressions](expressions.md) for conditional logic, safe navigation, and formatting.
- See [Providers and sources](providers.md) for custom commands, IPC triggers, and system events.
- Check [Inline images](images.md) to render graphics directly inside character grid cells.
