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

mod given_the_report_command {
    use super::*;

    fn words(texts: &[&str]) -> Vec<String> {
        texts.iter().map(|text| (*text).to_owned()).collect()
    }

    #[test]
    fn when_parsed_alone_then_it_measures_every_transcript_as_text() {
        assert_eq!(
            parsed(&["report"]),
            Ok(Command::Report(ReportArguments::default()))
        );
    }

    #[test]
    fn when_parsed_with_every_flag_then_each_value_is_kept() {
        let arguments = [
            "report",
            "--since",
            "2026-10-01",
            "--until",
            "2026-10-30T22:35:23.003Z",
            "--session",
            "abc-1",
            "--session",
            "def-2",
            "--project",
            "/home/admin/projects/meter",
            "--cap",
            "200000",
            "--format",
            "json",
        ];

        assert_eq!(
            parsed(&arguments),
            Ok(Command::Report(ReportArguments {
                bounds: Bounds {
                    since: Some(Bound::Date(jiff::civil::date(2026, 10, 1))),
                    until: Some(Bound::Instant("2026-10-30T22:35:23.003Z".parse().unwrap())),
                },
                sessions: words(&["abc-1", "def-2"]),
                projects: words(&["/home/admin/projects/meter"]),
                cap: Some(200_000),
                format: Format::Json,
            }))
        );
    }

    #[test]
    fn when_a_flag_taken_once_is_repeated_then_it_is_refused_by_name() {
        let rows = [
            (
                ["--since", "2026-10-01", "--since", "2026-10-02"],
                Flag::Since,
            ),
            (
                ["--until", "2026-10-01", "--until", "2026-10-02"],
                Flag::Until,
            ),
            (["--format", "text", "--format", "json"], Flag::Format),
            (["--cap", "200000", "--cap", "300000"], Flag::Cap),
        ];

        for (flags, flag) in rows {
            let arguments: Vec<&str> = std::iter::once("report").chain(flags).collect();

            assert_eq!(
                parsed(&arguments),
                Err(UsageFault::RepeatedFlag(flag)),
                "{flags:?}"
            );
        }
    }

    #[test]
    fn when_a_flag_lacks_its_value_then_it_is_refused_naming_the_flag() {
        assert_eq!(
            parsed(&["report", "--session"]),
            Err(UsageFault::MissingValue(Flag::Session))
        );
    }

    #[test]
    fn when_a_value_fails_its_flags_check_then_it_is_refused_with_the_value() {
        let rows = [
            (Flag::Since, "yesterday"),
            (Flag::Until, "2026-10-30T22:35:23"),
            (Flag::Session, "../escape"),
            (Flag::Project, ""),
            (Flag::Format, "yaml"),
            (Flag::Cap, "0"),
            (Flag::Cap, "200k"),
            (Flag::Cap, "+200000"),
            (Flag::Cap, ""),
        ];

        for (flag, value) in rows {
            assert_eq!(
                parsed(&["report", flag.spelling(), value]),
                Err(UsageFault::InvalidValue(flag, value.to_owned())),
                "{flag:?} {value}"
            );
        }
    }

    #[test]
    fn when_given_an_unknown_flag_then_it_is_refused_by_name() {
        assert_eq!(
            parsed(&["report", "--limit", "100"]),
            Err(UsageFault::UnknownArgument("--limit".to_owned()))
        );
    }
}
