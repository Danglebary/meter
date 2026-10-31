# meter

meter renders Claude Code's status line and measures Claude Code's token usage and cost, from one core. `meter statusline` reads the JSON Claude Code sends a status line on stdin and prints the line: the context in tokens, whether the prompt cache is warm or cold, and the account's usage or spend against its limit. `meter report` measures usage and cost over Claude Code's transcripts, for one session or a range, the same way every time.

Neither subcommand is built yet; the binary answers `--help` and `--version`.

## Development

`just check` is the gate: the format check, clippy with every warning denied, and the tests. The toolchain is the one `rust-toolchain.toml` pins; the flake's shell provides it, and the recipes enter that shell themselves.

## License

Licensed under either of the [Apache License, Version 2.0](LICENSE-APACHE) or the [MIT license](LICENSE-MIT), at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
