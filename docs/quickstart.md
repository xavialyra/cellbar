# Quickstart

[README](../README.md) · [Configuration](configuration.md)

Run these commands from the project root. Launch Cellbar inside a Wayland
session whose compositor supports layer-shell.

## Build

Install mise, a C build toolchain, pkg-config, and the PipeWire development
libraries using your system's package manager. Font discovery uses `fc-list`
from Fontconfig; install at least one text font.

```sh
mise install
mise run build
```

The executable is `target/release/cellbar`.

## Initialize and run

Initialize a ready-to-run configuration, validate it, and launch:

```sh
./target/release/cellbar init
./target/release/cellbar check
./target/release/cellbar
```

`init` writes a starter configuration to `$XDG_CONFIG_HOME/cellbar` (or
`$HOME/.config/cellbar` when `XDG_CONFIG_HOME` is unset). It refuses to
overwrite an existing directory.

The embedded starter configuration uses the minimal template:
- Layout, theme, and in-process components (`clock`, `cpu`, `memory`, `window`).
- Zero external scripts, zero icon font requirements, zero shell forks.
- Ready to customize immediately.

## Try examples without installing

You can test any configuration directly by specifying its path with `-c`:

```sh
# Minimal starter setup
./target/release/cellbar -c examples/minimal/config.toml

# Classic flat statusbar
./target/release/cellbar -c examples/flat/config.toml

# Modern capsule pill islands
./target/release/cellbar -c examples/capsules/config.toml
```

## Validate and reload

After editing your configuration, validate it before restarting:

```sh
./target/release/cellbar check
```

While Cellbar is running in the background, reload it seamlessly without losing
window focus or tearing:

```sh
./target/release/cellbar reload
```

An invalid reload leaves the active configuration in place. Inspect bar status
and active displays with:

```sh
./target/release/cellbar status
./target/release/cellbar list
```

Other CLI commands are described in [Control](configuration.md#control).

## Next steps

- Explore [Configuration](configuration.md) for layout syntax, theme palettes, and font fallbacks.
- Learn about [Expressions](expressions.md) for conditional logic, safe navigation, and formatting.
- See [Providers and sources](providers.md) for custom commands, IPC triggers, and system events.
- Check [Inline images](images.md) to render graphics directly inside character grid cells.
