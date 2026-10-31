//! The shell: reads the arguments, prints what the core decides, and ends with the exit
//! code the core names.

use std::process::ExitCode;

use meter::cli::{self, Command};
use meter::{exit_code, report};

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let parsed = cli::parse_arguments(&arguments);
    let code = match parsed {
        Ok(Command::Help) => {
            print!("{}", report::HELP);
            exit_code::SUCCESS
        }
        Ok(Command::Version) => {
            println!("meter {}", env!("CARGO_PKG_VERSION"));
            exit_code::SUCCESS
        }
        Err(fault) => {
            eprint!("{}", report::usage_fault(&fault));
            exit_code::USAGE
        }
    };
    ExitCode::from(code)
}
