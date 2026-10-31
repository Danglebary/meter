set positional-arguments

# The prefix that puts a command inside the flake's shell: nothing where the shell's
# marker `METER_SHELL` is set, so a command already inside it runs in place, and
# `nix develop` everywhere else, since a plain shell resolves the machine's toolchain
# rather than the project's.
in_shell := if env("METER_SHELL", "") == "1" { "" } else { "nix develop --command" }

# The `meter` binary built from the tree, its arguments passed through whole.
meter *ARGS:
    @{{in_shell}} cargo run --quiet -- "$@"

# The gate.
check: check-rust

check-rust:
    {{in_shell}} cargo fmt --all --check
    {{in_shell}} cargo clippy --all-targets -- -D warnings
    {{in_shell}} cargo test

# Installs the binary onto the machine's path with cargo, from this checkout.
install:
    cargo install --locked --path .
