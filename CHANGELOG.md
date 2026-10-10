# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.1] - 2026-10-07

### Added
- **Multi-compositor workspace support**: Added support for Niri, Hyprland, and Sway compositors.

### Fixed
- **Wayland frame callback deadlock & instant redraw**: Removed blocked redraw guard in `mark_bar_dirty` to prevent client-compositor frame callback deadlocks, restoring immediate (<20ms) bar updates on workspace and window changes.
- **Wayland workspace state event dispatching**: Dispatch workspace change events immediately on `ExtWorkspaceHandleV1::State` updates rather than relying solely on compositor `Done` events.
- **Wayland bar recreation & event subscription restoration**: Automatically recreate layer shell bars and re-subscribe all event dispatchers (Netlink, D-Bus, PipeWire, Wayland) when compositors reset layer shell surfaces.
- **Process zombie leak prevention**: Properly reaped terminated and timed-out child processes with `waitpid` to prevent defunct process accumulation.
- **Media and Network script robustness**: Pre-hashed cover art URLs to reuse cache without blocking on curl, and used overflow-safe awk math for network speed.
- **Instant event dispatch**: Fixed delayed execution for event-driven command providers by dispatching ready processes immediately on event receipt.
- **Event debounce & tick scheduling**: Defaulted event debounce to 0ms for instantaneous reaction and unified delayed timers into the central tick queue, preventing dropped events.
- **Wayland input region lifecycle**: Ensured custom input regions are kept alive across surface commits.
- **MPRIS storm throttle**: Added 250ms debounce to MPRIS property updates to prevent child process floods during media playback.
- **Resource leak**: Destroy closed foreign toplevel handles according to Wayland protocol specification.
- **Process lifecycle isolation**: Removed global sweep dispatch on child EOF in favor of targeted provider resumption.

## [0.1.0] - 2025-10-03

### Added
- **Wayland Layer Shell integration**: Native, direct Wayland client support using `smithay-client-toolkit` and `wlr-layer-shell-unstable-v1`.
- **Zero-fork event monitoring**: In-process native listeners for Linux kernel Netlink, sysfs battery/backlight, PipeWire audio, and Wayland workspaces.
- **Terminal-precision cell grid rasterization**: Monospace cell grid engine with Swash subpixel antialiasing, direct glyph blending, and inline cell image rendering.
- **Sandboxed expression engine**: Fast expression evaluation for dynamic status formats and thresholds.
- **Inter-process communication (IPC)**: UNIX domain socket IPC interface supporting `push`, `toggle`, `reload`, and real-time control.
- **Preset configurations**: `flat` and `capsules` (island/pill) designs included in `examples/`.
- **Command line interface**: `cellbar init`, `cellbar check`, `cellbar push`, and `cellbar reload`.
- **CI/CD pipeline**: Automated GitHub Actions workflows for continuous integration testing and release artifact packaging.
