//! The command line's grammar: the arguments in, the command to run or a usage fault out.

/// What one invocation does.
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Help,
    Statusline,
    Version,
}

/// Every way the arguments break the grammar.
#[derive(Debug, PartialEq, Eq)]
pub enum UsageFault {
    MissingCommand,
    UnknownArgument(String),
}

/// Parses the arguments after the program name.
///
/// # Errors
///
/// Returns the fault in the first argument the grammar refuses, or the absence of any.
pub fn parse_arguments(arguments: &[String]) -> Result<Command, UsageFault> {
    let words: Vec<&str> = arguments.iter().map(String::as_str).collect();
    match words.as_slice() {
        [] => Err(UsageFault::MissingCommand),
        ["--help" | "-h"] => Ok(Command::Help),
        ["--version" | "-V"] => Ok(Command::Version),
        ["statusline"] => Ok(Command::Statusline),
        [
            "--help" | "-h" | "--version" | "-V" | "statusline",
            surplus,
            ..,
        ] => Err(UsageFault::UnknownArgument((*surplus).to_owned())),
        [first, ..] => Err(UsageFault::UnknownArgument((*first).to_owned())),
    }
}

#[cfg(test)]
mod tests;
