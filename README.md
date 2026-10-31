# meter

meter renders Claude Code's status line and measures Claude Code's token usage and cost, from one core. `meter statusline` reads the JSON Claude Code sends a status line on stdin and prints the line: the context in tokens, whether the prompt cache is warm or cold, and the account's usage or spend against its limit. `meter report` measures usage and cost over Claude Code's transcripts, for one session or a range, the same way every time.

Neither subcommand is built yet; the binary answers `--help` and `--version`.

## Development

`just check` is the gate: the format check, clippy with every warning denied, and the tests. The toolchain is the one `rust-toolchain.toml` pins; the flake's shell provides it, and the recipes enter that shell themselves.
