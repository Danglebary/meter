# meter

A Rust binary with two subcommands on one core: `meter statusline` renders Claude Code's status line from the JSON Claude Code sends on stdin, and `meter report` measures Claude Code's token usage and cost over its transcripts, for one session or a range. `README.md` says what it is for; `meter --help`, generated from `src/report.rs`, is the grammar.

The engineering style guide at `~/.claude/CLAUDE.md` governs. This file states only what is specific to this repository.

## Decisions

meter records its decisions in the pirr ledger, in a stream of its own in the machine's shared store, which `.pirr/config.json` names. `pirr` runs from the `PATH`. Before acting on a design claim or raising a row, read what the ledger holds: `pirr guide reading/existing`.

## Running things

`just check` is the gate and must stay green: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` under the lint table in `Cargo.toml`, and `cargo test`. The recipes run inside the flake's shell, entering `nix develop` themselves unless `METER_SHELL` shows they already stand in it. `just meter <arguments>` runs the binary from the tree.

The toolchain is pinned once, in `rust-toolchain.toml`, which both the flake and CI's rustup read.

CI also builds the flake's package on Linux and macOS. It compiles from the file set `flake.nix` lists, so a file the manifest names outside `src/` joins that set.

## Shape

`src/lib.rs` is the functional core and does no IO: the argument grammar (`cli`), every message (`report`), and the exit codes (`exit_code`). `src/main.rs` is the shell: it reads the arguments, prints, and maps outcomes to exit codes. A new decision goes in the library with a unit test; `main.rs` holds wiring, which `tests/cli.rs` covers against the built binary.

The lint table denies `unwrap`, `expect`, `panic!`, indexing, slicing, string slicing, `as` casts, and unchecked arithmetic. A programmer error crashes through `assert!` or `unreachable!` with a message; arithmetic goes through `checked_*` with a `let ... else { unreachable!(..) }`, and slicing through `.get(..)`.

## Tests

A test is a function named `when_<operation>_then_<result>` in a module named `given_<input>`, and each test file opens with its goal and method as a `//!` comment. Each case asserts on a returned value, or on the exit status and streams of the built binary.
