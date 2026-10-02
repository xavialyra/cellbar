# Development

[README](../README.md) · [Quickstart](quickstart.md)

The project uses mise to pin the Rust toolchain and run standard checks:

```sh
mise install
mise run check
```

Validate the bundled configurations directly:

```sh
mise run config-check
```

## Memory analysis

The two memory-analysis tasks are managed through mise. Results are written
under `target/memory`; the normal `check` task does not require profiling tools.

The primary heap profiler is `dhat`, compiled and managed as a Cargo
optional dependency. It does not require Valgrind, Heaptrack, sudo, or any
system package:

```sh
mise install
CELLBAR_MEMORY_DURATION=60s mise run memory-dhat
```

The task automatically sends Cellbar `SIGTERM` after the configured duration,
allowing DHAT to flush `target/memory/dhat-heap.json`. Pressing `Ctrl-C` also
works. Open the JSON with the DHAT web viewer to inspect allocation call stacks
and live heap growth. The profiled binary is slower and is built with release
debug information.

For RSS/PSS and memory used by descendant processes such as reapers and
providers:

```sh
CELLBAR_PID=$(pgrep -xo cellbar) mise run memory-rss
```

The RSS task writes `target/memory/rss.tsv` until Cellbar exits. Override the
sample interval or output path with `CELLBAR_MEMORY_INTERVAL` and
`CELLBAR_MEMORY_SNAPSHOT_FILE`.

DHAT covers Rust heap allocations in the main Cellbar process. `memory-rss`
complements it with operating-system memory accounting for the complete process
tree.

Build a release binary with `mise run build`. See [Quickstart](quickstart.md)
for running it with a minimal configuration.
