# meter

meter renders Claude Code's status line and measures Claude Code's token usage and cost, from one core. `meter statusline` reads the JSON Claude Code sends a status line on stdin and prints the line: the context in tokens, whether the prompt cache is warm or cold, and the account's usage or spend against its limit. `meter report` measures usage and cost over Claude Code's transcripts, for one session or a range, the same way every time.

`meter statusline` is built, and `meter report` measures cost, context sizes, sessions, and cache rebuilds; its sections on what fills the context, subagents, and a context cap are not built yet. Point Claude Code's status line at `meter statusline` in `settings.json`:

```json
"statusLine": { "type": "command", "command": "meter statusline", "refreshInterval": 60 }
```

It prints three lines: the directory, branch, and pull request; the model, effort, context in tokens, and whether the prompt cache is warm, with the minutes left, or cold; and the 7-day usage on a subscription limited by usage, or the session's, day's, and period's spend behind a gateway with a spend limit. A field Claude Code leaves out or sends malformed blanks its own segment and nothing else. Behind a spend limit, the day's first reading is kept in `$XDG_STATE_HOME/meter/spend-day.json`, `~/.local/state` by default.

`meter report` reads every transcript under `~/.claude/projects` and prints how many sessions, subagents, and calls it measured, their cost at list price by bucket, by thread and model, by model and effort, and by cache-write TTL, the main thread's calls by context size, how much context each session started with and how many calls it ran, and which calls rebuilt the prompt cache, by the gap they followed. `--since` and `--until` take a date or a timestamp and bound the records read, `--session` and `--project` narrow the transcripts, and `--format json` prints the same report for an agent; `meter --help` lists the flags.

## Development

`just check` is the gate: the format check, clippy with every warning denied, and the tests. The toolchain is the one `rust-toolchain.toml` pins; the flake's shell provides it, and the recipes enter that shell themselves.

A release is cut by raising `version` in `Cargo.toml` and merging to `main`; the release workflow builds and publishes it.

## License

Licensed under either of the [Apache License, Version 2.0](LICENSE-APACHE) or the [MIT license](LICENSE-MIT), at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
