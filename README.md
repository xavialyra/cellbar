# Cellbar

[![CI](https://github.com/xavialyra/cellbar/actions/workflows/ci.yml/badge.svg)](https://github.com/xavialyra/cellbar/actions/workflows/ci.yml)
[![Wayland Layer Shell](https://img.shields.io/badge/Wayland-layer--shell-blue.svg)](https://wayland.app/protocols/wlr-layer-shell-unstable-v1)
[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)](LICENSE)

A lightweight, terminal-inspired Wayland status bar engineered for performance, precision, and simplicity.

Cellbar renders status information onto an explicit character cell grid with subpixel text antialiasing, direct kernel event integration, and a sandboxed in-process expression engine — keeping resource usage low (~3.3 MB release binary) without spawning periodic shell forks.

---

<p align="center">
  <img src="assets/screenshot-1.png" alt="Cellbar Flat Preset" width="100%" />
</p>
<p align="center">
  <img src="assets/screenshot-2.png" alt="Cellbar Capsules Preset" width="100%" />
</p>

---

## Highlights

- **Ultra-Low Resource Footprint**: Tiny ~3.3 MB standalone binary with ~8 MB resident memory (~1.3 MB unique private USS), zero heavy GUI toolkits (no GTK, Qt, or Electron), and battery-friendly event-driven sleep.
- **Zero-Fork Native Event Sources**: Direct in-process listeners for Linux kernel and desktop events (battery, backlight, Netlink network state, PipeWire audio, Wayland workspaces) with zero background shell polling.
- **Deep Extensibility & Managed Pipelines**: Populate content via sandboxed expressions, custom scripts, or direct IPC. Seamlessly bind to system event triggers or spawn long-running streaming child processes with automatic lifecycle management.
- **Terminal-Precision Grid & Modern Aesthetics**: Monospace cell grid with strict Unicode/CJK widths, subpixel text antialiasing, inline cell graphics, and support for both classic flat bars and floating capsule pill islands.
- **Rich Interactions & Seamless Live Control**: Multi-button pointer bindings (left/right/middle click, 4-way wheel scroll) with built-in hardware debouncing, scriptable IPC controls (`push`, `toggle`, `reload`), and instant config reloads without window focus loss or screen tearing.

---

## Quickstart

### 1. Installation

#### Option A: Direct Download (Recommended)

Download the pre-compiled binary directly to your local bin path:

```sh
curl -fsSL https://github.com/xavialyra/cellbar/releases/latest/download/cellbar-linux-x86_64 -o ~/.local/bin/cellbar
chmod +x ~/.local/bin/cellbar
```

#### Option B: Build from Source

Requirements: Rust toolchain (1.80+), C compiler (`gcc`), `pkg-config`, `libsystemd`, and `libpipewire-0.3`.

```sh
# Clone repository
git clone https://github.com/xavialyra/cellbar.git
cd cellbar

# Build release binary (using Cargo or mise)
cargo build --release
# Or: mise run build

# Install to ~/.local/bin
install -Dm755 target/release/cellbar ~/.local/bin/cellbar
```

### 2. Initialize and Launch

Generate a ready-to-run starter configuration with clock, CPU, memory, and active window components:

```sh
# Initialize starter config in ~/.config/cellbar
cellbar init

# Validate configuration
cellbar check

# Run Cellbar
cellbar
```

For more examples and theme ideas, see the presets (`flat` and `capsules`) in the [`examples/`](examples) directory. See the [Quickstart Guide](docs/quickstart.md) for full setup instructions and system dependencies.

---

## Resource Footprint & Efficiency

Cellbar is built as an ultra-lean, native Wayland layer-shell client. It connects directly to the Wayland protocol and Linux kernel event streams (Netlink sockets, Uevent, and in-process sysfs sampling), eliminating the need for periodic background shell forks (`free`, `cat`, `date`, `awk`).

### Empirical Benchmark (Flat Preset Baseline)

Measured under a live Wayland session (`mango` / wlroots, layer-shell) on Arch Linux (Kernel `7.2.6-arch2-1`, Intel Core i5-14600KF, 2540×32 display):

| Benchmark Metric | Measured Result | Significance |
| :--- | :--- | :--- |
| **Executable Binary Size** | **3.27 MB** | Self-contained; links only `libc`, `libm`, `libsystemd` |
| **Peak Heap Allocation (DHAT)** | **837 KB** | Maximum live heap during execution stays below 1 MB |
| **Physical Resident Memory (RSS)** | **~8.0 MB** | Total pages mapped in RAM (mostly shared font/OS caches) |
| **Proportional Set Size (PSS)** | **~4.4 MB** | Actual memory share accounted to Cellbar |
| **Unique Private Memory (USS)** | **~1.3 MB** | Dedicated unshared private dirty memory |
| **Steady-State CPU Usage** | **~0.3% – 1.0%** (Single Core) | Majority of sampling seconds register 0.00% CPU |
| **Involuntary Context Switches** | **~1.0 / sec** | Sleeps in `epoll`; wakes strictly on aligned events |
| **Background Shell Forks** | **0** | No recurring `free`, `cat`, `date`, or `awk` processes |

Reproduce this benchmark on your machine at any time:

```sh
mise run benchmark
# Or run the script directly:
python3 scripts/benchmark.py examples/flat/config.toml
```

Heap allocations and process RSS are continuously measured and guarded with integrated profiling tools (see [Development](docs/development.md#memory-analysis)).

---

## Documentation

- [Quickstart Guide](docs/quickstart.md) — build instructions, initial setup, and first run.
- [Configuration Reference](docs/configuration.md) — bars, layouts, theme palettes, typography, and CLI commands.
- [Expressions](docs/expressions.md) — in-process expression syntax, safe navigation, and built-in helper functions.
- [Providers and Sources](docs/providers.md) — system events, custom commands, IPC subscriptions, and actions.
- [Context Reference](docs/context.md) — event payloads for Wayland, system monitors, Netlink, and PipeWire.
- [Inline Images](docs/images.md) — rendering PNG, JPEG, and WebP graphics inside character cells.
- [Architecture](docs/architecture.md) — cell grid model, event pipeline, and rendering internals.
- [Development](docs/development.md) — testing, formatting, and memory profiling.

---

## Philosophy

Cellbar is a status bar, not a terminal emulator. It does not implement a PTY, CSS styling engines, or bloated web runtimes. By combining declarative TOML configuration with a tiny in-process expression engine, Cellbar delivers instant response times and minimal battery impact on Wayland desktops.
