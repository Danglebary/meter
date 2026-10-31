//! The status line's core, each case a JSON fixture and the facts the shell gathers, held to
//! the lines it renders. Fixtures start from one valid input and override the field a case
//! is about, so each case states the one property it varies; the clock is fixed at
//! 2026-10-28 15:00, a Wednesday, at UTC-7.

use super::*;
use jiff::Zoned;
use jiff::civil::{Date, date};
use jiff::tz::{TimeZone, offset};
use serde_json::{Value, json};

fn zone() -> TimeZone {
    TimeZone::fixed(offset(-7))
}

fn at(day: Date, hour: i8, minute: i8) -> Zoned {
    day.at(hour, minute, 0, 0).to_zoned(zone()).unwrap()
}

fn now() -> Zoned {
    at(date(2026, 10, 28), 15, 0)
}

fn epoch(zoned: &Zoned) -> i64 {
    zoned.timestamp().as_second()
}

fn seconds_from_now(seconds: i64) -> i64 {
    epoch(&now()).checked_add(seconds).unwrap()
}

/// A usage-limited session in the layout's warm example, its 7-day window resetting
/// 2026-11-03 at 1am.
fn base() -> Value {
    json!({
        "session_id": "abc123",
        "cwd": "/home/admin/projects/pirr",
        "workspace": { "current_dir": "/home/admin/projects/pirr", "project_dir": "/home/admin/projects/pirr" },
        "model": { "id": "claude-opus-5-5", "display_name": "Opus 5.5" },
        "effort": { "level": "xhigh" },
        "context_window": { "total_input_tokens": 143_000, "context_window_size": 1_000_000 },
        "prompt_cache": { "warm": true, "caching_observed": true, "ttl": "1h", "expires_at": seconds_from_now(42 * 60) },
        "rate_limits": { "seven_day": { "used_percentage": 20.0, "resets_at": epoch(&at(date(2026, 11, 3), 1, 0)) } },
        "cost": { "total_cost_usd": 21.45 },
        "pr": { "number": 51, "url": "https://github.com/Danglebary/pirr/pull/51" }
    })
}

/// The base with the value at each JSON pointer replaced, or removed where the value is
/// `None`.
fn with(overrides: &[(&str, Option<Value>)]) -> Value {
    let mut input = base();
    for (pointer, value) in overrides {
        let (parent, key) = pointer.rsplit_once('/').unwrap();
        let object = input.pointer_mut(parent).unwrap().as_object_mut().unwrap();
        match value {
            Some(value) => object.insert(key.to_owned(), value.clone()),
            None => object.remove(key),
        };
    }
    input
}

fn clean_branch() -> GitState {
    GitState {
        head: Head::Branch("main".to_owned()),
        staged: false,
        unstaged: true,
        untracked: false,
    }
}

struct Run {
    input: Value,
    git: Option<GitState>,
    pirr_checkout: bool,
    day_start_usd: Option<f64>,
}

impl Run {
    fn of(input: Value) -> Self {
        Self {
            input,
            git: Some(clean_branch()),
            pirr_checkout: false,
            day_start_usd: None,
        }
    }

    fn lines(&self) -> Vec<Line> {
        let now = now();
        let facts = Facts {
            now: &now,
            home: Some("/home/admin"),
            git: self.git.as_ref(),
            pirr_checkout: self.pirr_checkout,
            day_start_usd: self.day_start_usd,
        };
        render(&Input::read(&self.input), &facts)
    }

    fn texts(&self) -> Vec<String> {
        self.lines().iter().map(Line::text).collect()
    }
}

fn color_of(line: &Line, text: &str) -> Option<Color> {
    line.spans
        .iter()
        .find(|span| span.text == text)
        .map(|span| span.color)
}

mod given_a_usage_limited_session {
    use super::*;

    #[test]
    fn when_rendered_then_it_reads_as_the_layouts_warm_example() {
        assert_eq!(
            Run::of(base()).texts(),
            [
                "~/projects/pirr on main* · PR #51",
                "opus xhigh | 143k · warm · 42m",
                "7d 20% · resets 11/3 @ 1am",
            ]
        );
    }

    #[test]
    fn when_rendered_then_the_branch_is_blue_and_the_cache_green() {
        let lines = Run::of(base()).lines();

        assert_eq!(color_of(&lines[0], "main"), Some(Color::Blue));
        assert_eq!(color_of(&lines[1], "warm"), Some(Color::Green));
    }
}

mod given_the_context_size {
    use super::*;

    #[test]
    fn when_rendered_then_the_count_is_colored_by_its_band() {
        let rows = [
            (99_999, "100k", Color::Green),
            (100_000, "100k", Color::Yellow),
            (199_999, "200k", Color::Yellow),
            (200_000, "200k", Color::Red),
            (342_400, "342k", Color::Red),
        ];

        for (tokens, shown, color) in rows {
            let run = Run::of(with(&[(
                "/context_window/total_input_tokens",
                Some(json!(tokens)),
            )]));
            let lines = run.lines();

            assert_eq!(color_of(&lines[1], shown), Some(color), "{tokens}");
        }
    }

    #[test]
    fn when_no_response_has_arrived_then_the_count_is_left_out() {
        let run = Run::of(with(&[(
            "/context_window/total_input_tokens",
            Some(json!(0)),
        )]));

        assert_eq!(run.texts()[1], "opus xhigh | warm · 42m");
    }
}

mod given_the_cache_state {
    use super::*;

    #[test]
    fn when_the_cache_is_cold_then_line_two_reads_as_the_layouts_cold_example() {
        let run = Run::of(with(&[
            ("/context_window/total_input_tokens", Some(json!(342_000))),
            ("/prompt_cache/warm", Some(json!(false))),
        ]));
        let lines = run.lines();

        assert_eq!(lines[1].text(), "opus xhigh | 342k · cold · /clear");
        assert_eq!(color_of(&lines[1], "cold"), Some(Color::Red));
    }

    #[test]
    fn when_the_cache_is_cold_in_a_pirr_checkout_then_the_hint_adds_the_pickup() {
        let mut run = Run::of(with(&[("/prompt_cache/warm", Some(json!(false)))]));
        run.pirr_checkout = true;

        assert_eq!(
            run.texts()[1],
            "opus xhigh | 143k · cold · /clear → /pirr:pickup"
        );
    }

    #[test]
    fn when_a_warm_caches_expiry_has_passed_then_it_reads_cold() {
        let run = Run::of(with(&[(
            "/prompt_cache/expires_at",
            Some(json!(seconds_from_now(-1))),
        )]));

        assert_eq!(run.texts()[1], "opus xhigh | 143k · cold · /clear");
    }

    #[test]
    fn when_the_countdown_nears_expiry_then_it_turns_yellow_in_the_last_ten_minutes() {
        let rows = [
            (11 * 60, "11m", Color::Plain),
            (10 * 60, "10m", Color::Yellow),
            (8 * 60 + 1, "9m", Color::Yellow),
        ];

        for (seconds, shown, color) in rows {
            let run = Run::of(with(&[(
                "/prompt_cache/expires_at",
                Some(json!(seconds_from_now(seconds))),
            )]));
            let lines = run.lines();

            assert_eq!(color_of(&lines[1], shown), Some(color), "{seconds}");
        }
    }

    #[test]
    fn when_caching_is_not_observed_then_the_cache_state_is_left_out() {
        let run = Run::of(with(&[(
            "/prompt_cache/caching_observed",
            Some(json!(false)),
        )]));

        assert_eq!(run.texts()[1], "opus xhigh | 143k");
    }
}

mod given_the_seven_day_window {
    use super::*;

    /// The window opened 2026-10-27 at 1am, so 38 of its 168 hours, 22.6%, have passed.
    #[test]
    fn when_rendered_then_the_figure_is_colored_by_its_pace() {
        let rows = [
            (22.0, Color::Green),
            (32.0, Color::Yellow),
            (33.0, Color::Red),
            (91.0, Color::Red),
        ];

        for (used, color) in rows {
            let run = Run::of(with(&[(
                "/rate_limits/seven_day/used_percentage",
                Some(json!(used)),
            )]));
            let lines = run.lines();
            let shown = format!("{used:.0}%");

            assert_eq!(color_of(&lines[2], &shown), Some(color), "{used}");
        }
    }

    /// The window resets 12 hours from now, so 156 of its 168 hours, 92.9%, have passed.
    #[test]
    fn when_the_limit_is_nearly_spent_then_the_figure_is_red_whatever_its_pace() {
        let rows = [(89.0, Color::Green), (91.0, Color::Red)];

        for (used, color) in rows {
            let run = Run::of(with(&[
                ("/rate_limits/seven_day/used_percentage", Some(json!(used))),
                (
                    "/rate_limits/seven_day/resets_at",
                    Some(json!(seconds_from_now(12 * 3600))),
                ),
            ]));
            let lines = run.lines();
            let shown = format!("{used:.0}%");

            assert_eq!(color_of(&lines[2], &shown), Some(color), "{used}");
        }
    }

    #[test]
    fn when_the_reset_falls_in_the_afternoon_then_its_time_reads_in_twelve_hour_form() {
        let resets = epoch(&at(date(2026, 11, 4), 13, 30));
        let run = Run::of(with(&[(
            "/rate_limits/seven_day/resets_at",
            Some(json!(resets)),
        )]));

        assert_eq!(run.texts()[2], "7d 20% · resets 11/4 @ 1:30pm");
    }
}

mod given_an_account_with_no_usage_window {
    use super::*;

    #[test]
    fn when_the_session_has_spent_then_line_three_shows_its_cost_alone() {
        let run = Run::of(with(&[("/rate_limits", None)]));

        assert_eq!(run.texts()[2], "$21.45");
    }

    #[test]
    fn when_nothing_has_been_spent_then_line_three_is_left_out() {
        let run = Run::of(with(&[
            ("/rate_limits", None),
            ("/cost/total_cost_usd", Some(json!(0.0))),
        ]));

        assert_eq!(run.texts().len(), 2);
    }
}

mod given_a_spend_limited_account {
    use super::*;

    /// A monthly limit resetting 2026-11-01 at midnight: 3 business days remain today
    /// included, and the day began at $412.37 spent, so $4,587.63 of the limit spreads over
    /// them at $1,529.21 a day.
    fn spend() -> Value {
        with(&[(
            "/rate_limits",
            Some(json!({ "spend_limit": {
                "used_percentage": 9.679, "resets_at": epoch(&at(date(2026, 11, 1), 0, 0)),
                "used_usd": 483.95, "limit_usd": 5000.0, "period": "monthly"
            }})),
        )])
    }

    #[test]
    fn when_the_days_start_is_known_then_line_three_reads_as_the_layouts_cost_example() {
        let mut run = Run::of(spend());
        run.day_start_usd = Some(412.37);

        assert_eq!(
            run.texts()[2],
            "$21.45 | $71.58 / $1,529.21 | $483.95 / $5,000 | 2 days"
        );
    }

    #[test]
    fn when_the_days_spend_is_compared_with_the_pace_then_it_is_colored_by_their_ratio() {
        let rows = [
            (412.37, "$71.58", Color::Green),
            (0.0, "$483.95", Color::Green),
        ];

        for (day_start, shown, color) in rows {
            let mut run = Run::of(spend());
            run.day_start_usd = Some(day_start);
            let lines = run.lines();

            assert_eq!(color_of(&lines[2], shown), Some(color), "{day_start}");
        }
    }

    #[test]
    fn when_the_day_overspends_its_pace_then_its_spend_turns_red() {
        let mut input = spend();
        input["rate_limits"]["spend_limit"]["used_usd"] = json!(4900.0);
        let mut run = Run::of(input);
        run.day_start_usd = Some(1000.0);
        let lines = run.lines();

        assert_eq!(color_of(&lines[2], "$3,900"), Some(Color::Red));
    }

    #[test]
    fn when_the_days_start_is_unknown_then_the_days_figure_is_left_out() {
        assert_eq!(
            Run::of(spend()).texts()[2],
            "$21.45 | $483.95 / $5,000 | 2 days"
        );
    }

    #[test]
    fn when_the_dollar_fields_are_absent_then_the_line_shows_the_percentage() {
        let mut input = spend();
        let limit = input["rate_limits"]["spend_limit"].as_object_mut().unwrap();
        limit.remove("used_usd");
        limit.remove("limit_usd");

        assert_eq!(Run::of(input).texts()[2], "$21.45 | spend 10%");
    }
}

mod given_a_malformed_field {
    use super::*;

    #[test]
    fn when_rendered_then_only_its_own_segment_is_blank() {
        let rows = [
            (
                "/model/id",
                json!(42),
                "opus 5.5 xhigh | 143k · warm · 42m",
                1,
            ),
            (
                "/effort/level",
                json!(["xhigh"]),
                "opus | 143k · warm · 42m",
                1,
            ),
            (
                "/context_window/total_input_tokens",
                json!("143000"),
                "opus xhigh | warm · 42m",
                1,
            ),
            (
                "/prompt_cache/expires_at",
                json!("soon"),
                "opus xhigh | 143k",
                1,
            ),
            ("/pr/number", json!(-3), "~/projects/pirr on main*", 0),
        ];

        for (pointer, value, expected, line) in rows {
            let run = Run::of(with(&[(pointer, Some(value))]));

            assert_eq!(run.texts()[line], expected, "{pointer}");
        }
    }

    #[test]
    fn when_the_seven_day_window_is_malformed_then_line_three_is_left_out() {
        let run = Run::of(with(&[(
            "/rate_limits/seven_day/used_percentage",
            Some(json!("20%")),
        )]));

        assert_eq!(run.texts().len(), 2);
    }

    #[test]
    fn when_the_input_is_not_an_object_then_nothing_is_rendered() {
        let mut run = Run::of(json!([1, 2, 3]));
        run.git = None;

        assert_eq!(run.texts(), Vec::<String>::new());
    }
}

mod given_the_location {
    use super::*;

    #[test]
    fn when_rendered_then_the_git_state_and_pull_request_read_as_the_checkout_has_them() {
        let rows = [
            (
                Some(GitState {
                    head: Head::Detached("1a2b3c4".to_owned()),
                    staged: false,
                    unstaged: false,
                    untracked: false,
                }),
                base(),
                "~/projects/pirr on 1a2b3c4 · PR #51",
            ),
            (
                Some(GitState {
                    head: Head::Branch("main".to_owned()),
                    staged: true,
                    unstaged: true,
                    untracked: true,
                }),
                base(),
                "~/projects/pirr on main+*? · PR #51",
            ),
            (None, base(), "~/projects/pirr · PR #51"),
            (
                Some(clean_branch()),
                with(&[("/pr/kind", Some(json!("mr")))]),
                "~/projects/pirr on main* · MR !51",
            ),
            (
                Some(clean_branch()),
                with(&[("/workspace/current_dir", Some(json!("/srv/code")))]),
                "/srv/code on main* · PR #51",
            ),
            (
                Some(clean_branch()),
                with(&[("/workspace/current_dir", Some(json!("/home/admin")))]),
                "~ on main* · PR #51",
            ),
            (
                Some(clean_branch()),
                with(&[("/workspace/current_dir", Some(json!("/home/administrator")))]),
                "/home/administrator on main* · PR #51",
            ),
        ];

        for (git, input, expected) in rows {
            let mut run = Run::of(input);
            run.git = git;

            assert_eq!(run.texts()[0], expected);
        }
    }
}

mod given_porcelain_output {
    use super::*;

    #[test]
    fn when_parsed_then_the_head_and_markers_follow_its_lines() {
        let rows = [
            (
                "# branch.oid 1a2b3c4d5e\n# branch.head main\n",
                Head::Branch("main".to_owned()),
                [false, false, false],
            ),
            (
                "# branch.oid 1a2b3c4d5e\n# branch.head (detached)\n",
                Head::Detached("1a2b3c4".to_owned()),
                [false, false, false],
            ),
            (
                "# branch.oid (initial)\n# branch.head main\n? notes.md\n",
                Head::Branch("main".to_owned()),
                [false, false, true],
            ),
            (
                "# branch.head main\n1 M. N... 100644 100644 100644 a b src/lib.rs\n",
                Head::Branch("main".to_owned()),
                [true, false, false],
            ),
            (
                "# branch.head main\n1 .M N... 100644 100644 100644 a b src/lib.rs\n",
                Head::Branch("main".to_owned()),
                [false, true, false],
            ),
            (
                "# branch.head main\nu UU N... 1 2 3 4 a b c x.rs\n",
                Head::Branch("main".to_owned()),
                [true, true, false],
            ),
        ];

        for (text, head, [staged, unstaged, untracked]) in rows {
            assert_eq!(
                parse_porcelain(text),
                Ok(GitState {
                    head,
                    staged,
                    unstaged,
                    untracked
                }),
                "{text}"
            );
        }
    }

    #[test]
    fn when_no_head_line_is_present_then_it_is_refused() {
        assert_eq!(
            parse_porcelain("? notes.md\n"),
            Err(PorcelainFault::MissingHead)
        );
    }

    #[test]
    fn when_detached_with_no_commit_then_it_is_refused() {
        assert_eq!(
            parse_porcelain("# branch.head (detached)\n"),
            Err(PorcelainFault::MissingCommit)
        );
    }
}

mod given_a_date_range {
    use super::*;

    #[test]
    fn when_counted_then_weekdays_alone_are_business_days() {
        let rows = [
            (date(2026, 10, 28), date(2026, 10, 31), 3),
            (date(2026, 10, 31), date(2026, 11, 1), 0),
            (date(2026, 10, 26), date(2026, 11, 1), 5),
            (date(2026, 10, 29), date(2026, 10, 28), 0),
            (date(2026, 11, 1), date(2026, 11, 30), 21),
        ];

        for (from, through, count) in rows {
            assert_eq!(
                business_days(from, through),
                Ok(count),
                "{from}..={through}"
            );
        }
    }

    #[test]
    fn when_it_spans_more_days_than_its_bound_then_it_is_refused() {
        assert_eq!(
            business_days(date(2026, 1, 1), date(2028, 1, 1)),
            Err(RangeTooLong)
        );
    }
}

mod given_a_spend_reading {
    use super::*;

    fn reading(previous: Option<&Snapshot>, resets_at: i64, used_usd: f64) -> DayStart {
        let now = now();
        day_start(&DayStartInput {
            previous,
            now: &now,
            resets_at,
            used_usd,
        })
    }

    fn snapshot(day: Date, resets_at: i64, used_usd: f64) -> Snapshot {
        Snapshot {
            date: day,
            resets_at,
            used_usd,
        }
    }

    fn month_end() -> i64 {
        epoch(&at(date(2026, 11, 1), 0, 0))
    }

    #[test]
    fn when_none_was_kept_then_this_reading_starts_the_day() {
        let start = reading(None, month_end(), 100.0);

        assert_eq!(format!("{:.2}", start.used_usd), "100.00");
        assert_eq!(
            start.write,
            Some(snapshot(date(2026, 10, 28), month_end(), 100.0))
        );
    }

    #[test]
    fn when_todays_snapshot_stands_then_it_starts_the_day_and_nothing_is_written() {
        let kept = snapshot(date(2026, 10, 28), month_end(), 80.0);
        let start = reading(Some(&kept), month_end(), 100.0);

        assert_eq!(format!("{:.2}", start.used_usd), "80.00");
        assert_eq!(start.write, None);
    }

    #[test]
    fn when_the_snapshot_is_from_an_earlier_day_then_this_reading_starts_the_day() {
        let kept = snapshot(date(2026, 10, 27), month_end(), 80.0);
        let start = reading(Some(&kept), month_end(), 100.0);

        assert_eq!(format!("{:.2}", start.used_usd), "100.00");
        assert_eq!(
            start.write,
            Some(snapshot(date(2026, 10, 28), month_end(), 100.0))
        );
    }

    #[test]
    fn when_the_period_rolled_over_today_then_the_day_starts_from_nothing() {
        let rolled = epoch(&at(date(2026, 10, 28), 0, 0));
        let kept = snapshot(date(2026, 10, 27), rolled, 4000.0);
        let start = reading(Some(&kept), month_end(), 30.0);

        assert_eq!(format!("{:.2}", start.used_usd), "0.00");
        assert_eq!(
            start.write,
            Some(snapshot(date(2026, 10, 28), month_end(), 0.0))
        );
    }

    #[test]
    fn when_the_spend_fell_below_the_days_start_then_this_reading_starts_the_day_again() {
        let kept = snapshot(date(2026, 10, 28), month_end(), 120.0);
        let start = reading(Some(&kept), month_end(), 100.0);

        assert_eq!(format!("{:.2}", start.used_usd), "100.00");
        assert_eq!(
            start.write,
            Some(snapshot(date(2026, 10, 28), month_end(), 100.0))
        );
    }
}

mod given_a_snapshot_file {
    use super::*;

    #[test]
    fn when_written_and_read_back_then_it_is_the_same_snapshot() {
        let kept = Snapshot {
            date: date(2026, 10, 28),
            resets_at: 1_793_516_400,
            used_usd: 412.37,
        };

        assert_eq!(Snapshot::from_json(&kept.to_json()), kept);
    }

    #[test]
    #[should_panic]
    fn when_its_text_is_not_a_snapshot_then_reading_it_crashes() {
        let _ = Snapshot::from_json("{\"date\": \"2026-10-28\"}");
    }
}

mod given_figures_to_format {
    use super::*;

    #[test]
    fn when_dollars_are_formatted_then_whole_amounts_drop_their_cents() {
        let rows = [
            (0.0, "$0"),
            (21.45, "$21.45"),
            (250.0, "$250"),
            (5000.0, "$5,000"),
            (1529.21, "$1,529.21"),
            (1_234_567.5, "$1,234,567.50"),
        ];

        for (amount, shown) in rows {
            assert_eq!(render::format_dollars(amount), shown, "{amount}");
        }
    }

    #[test]
    fn when_a_reset_is_formatted_then_it_reads_month_day_and_twelve_hour_time() {
        let rows = [
            (at(date(2026, 11, 3), 1, 0), "11/3 @ 1am"),
            (at(date(2026, 11, 3), 0, 0), "11/3 @ 12am"),
            (at(date(2026, 12, 25), 12, 0), "12/25 @ 12pm"),
            (at(date(2026, 1, 9), 23, 5), "1/9 @ 11:05pm"),
        ];

        for (moment, shown) in rows {
            assert_eq!(render::format_reset(&moment), shown);
        }
    }
}

mod given_lines_to_print {
    use super::*;

    #[test]
    fn when_printed_then_each_colored_span_is_wrapped_and_lines_end_in_newlines() {
        let lines = [
            Line {
                spans: vec![Span::new("main", Color::Blue), Span::new("*", Color::Green)],
            },
            Line {
                spans: vec![Span::new("7d ", Color::Plain), Span::new("20%", Color::Red)],
            },
        ];

        assert_eq!(
            to_ansi(&lines),
            "\u{1b}[0;34mmain\u{1b}[0m\u{1b}[0;32m*\u{1b}[0m\n7d \u{1b}[0;31m20%\u{1b}[0m\n"
        );
    }
}
