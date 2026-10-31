//! The binary's wiring, run as a process: each case starts the built `meter` with
//! arguments and reads the exit status and the streams it wrote.

#[cfg(test)]
mod support {
    use std::process::{Command, Output};

    pub fn run(arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_meter"))
            .args(arguments)
            .output()
            .unwrap()
    }
}

mod given_version {
    use super::support::run;

    #[test]
    fn when_run_then_it_prints_the_crate_version_and_succeeds() {
        let output = run(&["--version"]);

        assert_eq!(
            output.status.code(),
            Some(i32::from(meter::exit_code::SUCCESS))
        );
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            format!("meter {}\n", env!("CARGO_PKG_VERSION"))
        );
    }
}

mod given_an_unknown_argument {
    use super::support::run;

    #[test]
    fn when_run_then_it_exits_with_the_usage_code_naming_the_argument() {
        let output = run(&["--frobnicate"]);

        assert_eq!(
            output.status.code(),
            Some(i32::from(meter::exit_code::USAGE))
        );
        assert_eq!(
            String::from_utf8(output.stderr).unwrap(),
            "meter: unknown argument `--frobnicate`; `meter --help` lists the commands\n"
        );
    }
}
