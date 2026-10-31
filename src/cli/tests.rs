//! The command line's grammar, each row an argument list and the command or fault it
//! parses to.

use super::*;

fn parsed(words: &[&str]) -> Result<Command, UsageFault> {
    let arguments: Vec<String> = words.iter().map(|word| (*word).to_owned()).collect();
    parse_arguments(&arguments)
}

mod given_a_flag_alone {
    use super::*;

    #[test]
    fn when_parsed_then_each_spelling_names_its_command() {
        let rows = [
            ("--help", Command::Help),
            ("-h", Command::Help),
            ("--version", Command::Version),
            ("-V", Command::Version),
        ];

        for (flag, command) in rows {
            assert_eq!(parsed(&[flag]), Ok(command), "{flag}");
        }
    }
}

mod given_the_statusline_command {
    use super::*;

    #[test]
    fn when_parsed_alone_then_it_names_the_status_line() {
        assert_eq!(parsed(&["statusline"]), Ok(Command::Statusline));
    }

    #[test]
    fn when_parsed_with_a_surplus_word_then_the_word_is_refused_by_name() {
        assert_eq!(
            parsed(&["statusline", "--verbose"]),
            Err(UsageFault::UnknownArgument("--verbose".to_owned()))
        );
    }
}

mod given_no_argument {
    use super::*;

    #[test]
    fn when_parsed_then_it_is_refused_for_naming_no_command() {
        assert_eq!(parsed(&[]), Err(UsageFault::MissingCommand));
    }
}

mod given_an_unknown_argument {
    use super::*;

    #[test]
    fn when_parsed_first_then_it_is_refused_by_name() {
        assert_eq!(
            parsed(&["--frobnicate"]),
            Err(UsageFault::UnknownArgument("--frobnicate".to_owned()))
        );
    }

    #[test]
    fn when_parsed_after_a_flag_then_the_surplus_word_is_refused_by_name() {
        assert_eq!(
            parsed(&["--version", "extra"]),
            Err(UsageFault::UnknownArgument("extra".to_owned()))
        );
    }
}
