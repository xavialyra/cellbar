# Contributing to Cellbar

Thank you for your interest in contributing to Cellbar! Cellbar aims to provide a reliable, ultra-lightweight, and zero-fork Wayland status bar.

## Code of Conduct

Please be respectful, constructive, and considerate in all interactions across issues, pull requests, and discussions.

## Development Workflow

### Prerequisites

- **Rust toolchain** (Rust 1.80+ recommended)
- **C compiler** (`gcc` or `clang`) and `pkg-config`
- **Development libraries**:
  - Arch Linux: `sudo pacman -S pipewire systemd-libs fontconfig`
  - Debian / Ubuntu: `sudo apt install libpipewire-0.3-dev libsystemd-dev fontconfig`
  - Fedora: `sudo dnf install pipewire-devel systemd-devel fontconfig-devel`

### Building and Testing

```sh
# Clone your fork
git clone https://github.com/your-username/cellbar.git
cd cellbar

# Check compilation
cargo check

# Run tests
cargo test

# Run with custom configuration
cargo run -- --config examples/flat/config.toml
```

### Code Style & Guidelines

1. **Format & Lint**: Ensure `cargo fmt --check` and `cargo clippy` pass without warnings.
2. **Keep Overhead Minimal**:
   - Cellbar's core mission is zero periodic shell forks and minimal RAM footprint (<10 MB RSS).
   - Prefer in-process kernel interfaces (`/sys`, `/proc`, Netlink, standard sockets) over spawning external binaries.
   - Avoid pulling heavy dependencies or GUI frameworks.
3. **Commit Messages**: Use Conventional Commits format (e.g. `feat: ...`, `fix: ...`, `docs: ...`, `refactor: ...`).

## Reporting Issues

If you encounter unexpected crashes, rendering artifacts, or bugs:
- Check existing issues before opening a new one.
- Use the **Bug Report** template and include your compositor, GPU driver, and minimal `config.toml`.

## Feature Proposals

Open a feature request issue or discussion first so we can discuss the design, protocol requirements, and feasibility before submitting a large PR.
