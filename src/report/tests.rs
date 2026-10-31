//! The report's core against values: transcript lines built as JSON in each case are read,
//! folded into calls, priced, measured, and rendered, and each case asserts on what one of
//! those steps returns. Times are placed in a fixed zone seven hours behind UTC.

use super::render::{Portion, dollars, millions, share};
use super::*;
use crate::cli::{Bound, Bounds};
use jiff::civil::date;
use jiff::tz::{TimeZone, offset};
use serde_json::json;

fn zone() -> TimeZone {
    TimeZone::fixed(offset(-7))
}

fn instant(text: &str) -> Timestamp {
    text.parse().unwrap()
}

fn range(bounds: Bounds) -> Range {
    Range::resolve(bounds, &zone()).unwrap()
}

/// One assistant record of a call.
struct Fixture {
    id: &'static str,
    at: &'static str,
    model: &'static str,
    usage: Usage,
}

fn record(fixture: &Fixture) -> String {
    let usage = &fixture.usage;
    let written = usage
        .cache_write_5m
        .checked_add(usage.cache_write_1h)
        .unwrap();
    json!({
        "type": "assistant",
        "timestamp": fixture.at,
        "effort": "xhigh",
        "message": {
            "id": fixture.id,
            "model": fixture.model,
            "usage": {
                "input_tokens": usage.input,
                "cache_read_input_tokens": usage.cache_read,
                "cache_creation_input_tokens": written,
                "cache_creation": {
                    "ephemeral_5m_input_tokens": usage.cache_write_5m,
                    "ephemeral_1h_input_tokens": usage.cache_write_1h
                },
                "output_tokens": usage.output
            }
        }
    })
    .to_string()
}

/// The main thread of session `s1` holding `fixtures`, in order.
fn folded(fixtures: &[Fixture], range: Range) -> Transcript {
    let mut fold = Fold::new(range);
    for fixture in fixtures {
        fold.take(read_line(&record(fixture)));
    }
    fold.finish("s1".to_owned(), Thread::Main)
}

/// A main thread of two calls, the first written twice with its output growing, and a
/// subagent of one priced call, one synthetic notice, and one call by a model the table
/// lacks.
fn session() -> Vec<Transcript> {
    let first_call = Usage {
        input: 10,
        cache_read: 100_000,
        cache_write_1h: 20_000,
        output: 400,
        ..Usage::default()
    };
    let main = [
        Fixture {
            id: "a",
            at: "2026-10-30T10:00:00Z",
            model: "claude-opus-5-5",
            usage: first_call,
        },
        Fixture {
            id: "a",
            at: "2026-10-30T10:00:01Z",
            model: "claude-opus-5-5",
            usage: Usage {
                output: 1_000,
                ..first_call
            },
        },
        Fixture {
            id: "b",
            at: "2026-10-30T10:05:00Z",
            model: "claude-fable-5-1",
            usage: Usage {
                cache_write_1h: 50_000,
                output: 2_000,
                ..Usage::default()
            },
        },
    ];
    let subagent = [
        Fixture {
            id: "c",
            at: "2026-10-30T10:02:00Z",
            model: "claude-opus-5-5",
            usage: Usage {
                input: 5,
                cache_read: 30_000,
                cache_write_5m: 10_000,
                output: 500,
                ..Usage::default()
            },
        },
        Fixture {
            id: "d",
            at: "2026-10-30T10:03:00Z",
            model: SYNTHETIC_MODEL,
            usage: Usage::default(),
        },
        Fixture {
            id: "e",
            at: "2026-10-30T10:04:00Z",
            model: "claude-next",
            usage: Usage {
                output: 100,
                ..Usage::default()
            },
        },
    ];
    vec![
        folded(&main, Range::default()),
        Transcript {
            thread: Thread::Subagent,
            ..folded(&subagent, Range::default())
        },
    ]
}

mod given_a_range_flag {
    use super::*;

    #[test]
    fn when_a_date_is_given_to_since_then_the_range_starts_at_its_local_midnight() {
        let resolved = range(Bounds {
            since: Some(Bound::Date(date(2026, 10, 1))),
            until: None,
        });

        assert_eq!(resolved.start, Some(instant("2026-10-01T07:00:00Z")));
        assert_eq!(resolved.end, None);
    }

    #[test]
    fn when_a_date_is_given_to_until_then_the_range_runs_through_its_local_end() {
        let resolved = range(Bounds {
            since: None,
            until: Some(Bound::Date(date(2026, 10, 30))),
        });

        assert!(resolved.holds(instant("2026-10-31T06:59:59.999Z")));
        assert!(!resolved.holds(instant("2026-10-31T07:00:00Z")));
    }

    #[test]
    fn when_a_timestamp_is_given_to_until_then_that_instant_is_inside_the_range() {
        let cutoff = instant("2026-10-30T22:35:23.003Z");
        let resolved = range(Bounds {
            since: None,
            until: Some(Bound::Instant(cutoff)),
        });

        assert!(resolved.holds(cutoff));
        assert!(!resolved.holds(instant("2026-10-30T22:35:23.004Z")));
    }

    #[test]
    fn when_since_falls_after_until_then_the_range_is_refused() {
        let bounds = Bounds {
            since: Some(Bound::Date(date(2026, 10, 30))),
            until: Some(Bound::Date(date(2026, 10, 29))),
        };

        assert_eq!(Range::resolve(bounds, &zone()), Err(RangeFault::Empty));
    }

    #[test]
    fn when_a_transcript_was_last_written_before_the_start_then_it_holds_no_record_in_range() {
        let resolved = range(Bounds {
            since: Some(Bound::Date(date(2026, 10, 1))),
            until: None,
        });

        assert!(!resolved.may_hold_records_written_until(instant("2026-10-01T06:59:59Z")));
        assert!(resolved.may_hold_records_written_until(instant("2026-10-01T07:00:00Z")));
    }
}

mod given_a_transcript_location {
    use super::*;

    fn location(project: &str, session: &str, thread: Thread) -> Location {
        Location {
            project: project.to_owned(),
            session: session.to_owned(),
            thread,
        }
    }

    #[test]
    fn when_selected_then_each_list_narrows_and_an_empty_one_takes_everything() {
        let selection = Selection {
            sessions: vec!["s1".to_owned()],
            projects: vec!["-home-p".to_owned()],
        };
        let rows = [
            (location("-home-p", "s1", Thread::Main), true),
            (location("-home-p", "s1", Thread::Subagent), true),
            (location("-home-p", "s2", Thread::Main), false),
            (location("-home-q", "s1", Thread::Main), false),
        ];

        for (location, selected) in &rows {
            assert_eq!(selection.selects(location), *selected, "{location:?}");
            assert!(Selection::default().selects(location), "{location:?}");
        }
    }
}

mod given_a_project_path {
    use super::*;

    #[test]
    fn when_encoded_then_every_character_but_letters_and_digits_becomes_a_hyphen() {
        let rows = [
            ("/home/admin/projects/meter", "-home-admin-projects-meter"),
            (
                "/tmp/claude-1001/-opt-homelab",
                "-tmp-claude-1001--opt-homelab",
            ),
            ("/home/admin/.claude", "-home-admin--claude"),
        ];

        for (path, directory) in rows {
            assert_eq!(project_directory(path), directory, "{path}");
        }
    }
}

mod given_a_transcript_line {
    use super::*;

    fn fixture() -> Fixture {
        Fixture {
            id: "a",
            at: "2026-10-30T10:00:00Z",
            model: "claude-opus-5-5",
            usage: Usage {
                input: 1,
                cache_read: 2,
                cache_write_5m: 3,
                cache_write_1h: 4,
                output: 5,
            },
        }
    }

    #[test]
    fn when_it_is_an_assistant_record_carrying_usage_then_its_fields_are_read() {
        let line = record(&fixture());

        assert_eq!(
            read_line(&line),
            Line::Record(Record {
                message_id: "a".to_owned(),
                timestamp: instant("2026-10-30T10:00:00Z"),
                model: "claude-opus-5-5".to_owned(),
                effort: Some("xhigh".to_owned()),
                usage: fixture().usage,
            })
        );
    }

    #[test]
    fn when_it_holds_no_assistant_record_carrying_usage_then_it_is_skipped() {
        let rows = [
            json!({ "type": "user", "message": { "content": "hello" } }).to_string(),
            json!({ "type": "assistant", "message": { "id": "a", "model": "m" } }).to_string(),
            json!({ "type": "assistant", "message": { "model": "m", "usage": {} } }).to_string(),
            "not json".to_owned(),
        ];

        for line in &rows {
            assert_eq!(read_line(line), Line::Other, "{line}");
        }
    }

    #[test]
    fn when_a_field_fails_its_check_then_the_record_names_that_field() {
        let above_bound = TOKENS_PER_FIELD_MAX.checked_add(1).unwrap();
        let rows = [
            ("/message/usage/input_tokens", json!("1")),
            ("/message/usage/output_tokens", json!(-1)),
            ("/message/usage/cache_read_input_tokens", json!(above_bound)),
            ("/message/model", json!("")),
            ("/timestamp", json!("yesterday")),
            ("/effort", json!(3)),
        ];

        for (pointer, broken) in rows {
            let mut line: serde_json::Value = serde_json::from_str(&record(&fixture())).unwrap();
            *line.pointer_mut(pointer).unwrap() = broken;

            assert_eq!(
                read_line(&line.to_string()),
                Line::Malformed(pointer),
                "{pointer}"
            );
        }
    }
}

mod given_a_calls_records {
    use super::*;

    #[test]
    fn when_folded_then_the_call_keeps_its_first_records_time_and_its_last_records_usage() {
        let transcript = &session()[0];

        let first = transcript.calls.first().unwrap();
        assert_eq!(transcript.calls.len(), 2);
        assert_eq!(first.timestamp, instant("2026-10-30T10:00:00Z"));
        assert_eq!(first.usage.output, 1_000);
    }

    #[test]
    fn when_a_record_falls_outside_the_range_then_it_is_skipped() {
        let fixtures = [
            Fixture {
                id: "a",
                at: "2026-10-30T09:59:59Z",
                model: "claude-opus-5-5",
                usage: Usage {
                    output: 1,
                    ..Usage::default()
                },
            },
            Fixture {
                id: "b",
                at: "2026-10-30T10:00:00Z",
                model: "claude-opus-5-5",
                usage: Usage {
                    output: 2,
                    ..Usage::default()
                },
            },
        ];
        let since = range(Bounds {
            since: Some(Bound::Instant(instant("2026-10-30T10:00:00Z"))),
            until: None,
        });

        let transcript = folded(&fixtures, since);

        let outputs: Vec<u64> = transcript
            .calls
            .iter()
            .map(|call| call.usage.output)
            .collect();
        assert_eq!(outputs, [2]);
    }

    #[test]
    fn when_the_model_is_synthetic_then_the_call_is_counted_apart() {
        let transcript = &session()[1];

        let models: Vec<&str> = transcript
            .calls
            .iter()
            .map(|call| call.model.as_str())
            .collect();
        assert_eq!(models, ["claude-opus-5-5", "claude-next"]);
        assert_eq!(transcript.synthetic, 1);
    }
}

mod given_a_price_table {
    use super::*;

    #[test]
    fn when_a_call_is_priced_then_each_bucket_takes_its_rate_and_writes_their_multiple() {
        let usage = Usage {
            input: 1_000_000,
            cache_read: 1_000_000,
            cache_write_5m: 1_000_000,
            cache_write_1h: 1_000_000,
            output: 1_000_000,
        };

        assert_eq!(
            cost("claude-opus-5-5", &usage),
            Some(Cost {
                input: 4_000_000_000,
                cache_read: 200_000_000,
                cache_write_5m: 5_000_000_000,
                cache_write_1h: 8_000_000_000,
                output: 20_000_000_000,
            })
        );
    }

    #[test]
    fn when_haiku_5_5_sends_more_than_its_lower_rate_context_then_it_takes_the_higher_rates() {
        let short = Usage {
            input: 100_000,
            output: 1,
            ..Usage::default()
        };
        let long = Usage {
            input: 100_001,
            ..short
        };

        let short_cost = cost("claude-haiku-5-5", &short).unwrap();
        let long_cost = cost("claude-haiku-5-5", &long).unwrap();

        assert_eq!((short_cost.input, short_cost.output), (10_000_000, 500));
        assert_eq!((long_cost.input, long_cost.output), (50_000_500, 2_500));
    }

    #[test]
    fn when_the_table_lacks_the_model_then_the_call_is_unpriced() {
        assert_eq!(cost("claude-next", &Usage::default()), None);
    }
}

mod given_transcripts_to_measure {
    use super::*;

    #[test]
    fn when_measured_then_sessions_subagents_calls_and_what_was_left_out_are_counted() {
        let measured = Measured::of(&session(), 1);

        assert_eq!(
            measured,
            Measured {
                sessions: 1,
                subagents: 1,
                calls: 4,
                synthetic: 1,
                malformed: 0,
                unread: 1,
                first_call: Some(instant("2026-10-30T10:00:00Z")),
                last_call: Some(instant("2026-10-30T10:05:00Z")),
            }
        );
    }

    #[test]
    fn when_costed_then_every_grouping_sums_to_the_buckets_total() {
        let section = cost_section(&session());

        let total = section.buckets.total();
        let by_thread: u128 = section
            .by_thread_and_model
            .values()
            .map(|spend| spend.nanodollars)
            .sum();
        let by_effort: u128 = section
            .by_model_and_effort
            .values()
            .map(|spend| spend.nanodollars)
            .sum();
        assert_eq!(total, 1_366_060_000);
        assert_eq!(by_thread, total);
        assert_eq!(by_effort, total);
    }

    #[test]
    fn when_a_model_is_unpriced_then_its_calls_are_named_and_left_out_of_every_amount() {
        let section = cost_section(&session());

        let unpriced_key = (Thread::Subagent, "claude-next".to_owned());
        assert_eq!(section.unpriced.get("claude-next"), Some(&1));
        assert!(!section.by_thread_and_model.contains_key(&unpriced_key));
    }

    #[test]
    fn when_writes_are_grouped_then_each_ttl_carries_its_own_tokens() {
        let section = cost_section(&session());

        let writes: Vec<((Thread, Ttl), u128)> = section
            .cache_writes
            .iter()
            .map(|(key, writes)| (*key, writes.tokens))
            .collect();
        assert_eq!(
            writes,
            [
                ((Thread::Main, Ttl::OneHour), 70_000),
                ((Thread::Subagent, Ttl::FiveMinutes), 10_000),
            ]
        );
    }
}

mod given_figures_to_format {
    use super::*;

    #[test]
    fn when_dollars_are_formatted_then_they_round_half_up_to_the_cent_and_group_thousands() {
        let rows = [
            (0, "$0.00"),
            (4_999_999, "$0.00"),
            (5_000_000, "$0.01"),
            (7_493_424_000_000, "$7,493.42"),
        ];

        for (nanodollars, text) in rows {
            assert_eq!(dollars(nanodollars), text, "{nanodollars}");
        }
    }

    #[test]
    fn when_tokens_are_formatted_then_they_read_in_millions_to_a_tenth() {
        assert_eq!(millions(137_650_000), "137.7M");
        assert_eq!(millions(1_049_999), "1.0M");
    }

    #[test]
    fn when_a_share_is_formatted_then_it_reads_as_a_percentage_to_a_tenth() {
        let writes = Portion {
            part: 1_210_000_000,
            whole: 1_366_060_000,
        };
        let nothing = Portion { part: 0, whole: 0 };

        assert_eq!(share(writes), "88.6%");
        assert_eq!(share(nothing), "0.0%");
    }
}

mod given_a_report_to_render {
    use super::*;

    fn report(transcripts: &[Transcript]) -> Report {
        Report::of(Gathered {
            range: Range::default(),
            selection: &Selection::default(),
            transcripts,
            unread: 0,
        })
    }

    #[test]
    fn when_rendered_as_text_then_it_reads_as_the_example() {
        let text = to_text(&report(&session()), &zone());

        assert_eq!(
            text,
            "\
Measured 1 session and 1 subagent: 4 calls, 2026-10-30 03:00 to 2026-10-30 03:05.
Left out: 1 synthetic record.

Cost at list price, as of 2026-10-06: $1.37

By bucket
  cache write   $1.21   88.6%
  output        $0.13    9.5%
  cache read    $0.03    1.9%
  input         $0.00    0.0%

By thread and model
  main       claude-fable-5-1   1 call   $1.10
  main       claude-opus-5-5    1 call   $0.20
  subagent   claude-opus-5-5    1 call   $0.07

By model and effort
  claude-fable-5-1   xhigh    1 call   $1.10
  claude-opus-5-5    xhigh   2 calls   $0.27

Cache writes by thread and TTL
  main       1h   0.1M tokens   $1.16
  subagent   5m   0.0M tokens   $0.05

Unpriced, left out of every amount
  claude-next   1 call

Main-thread calls by context, and their input-side cost
  under 50k       0 calls   $0.00
  50k to 100k      1 call   $1.00
  100k to 200k     1 call   $0.18
  200k to 400k    0 calls   $0.00
  400k and over   0 calls   $0.00
  Calls over 300k carry 0.0% of the main thread's cache-read cost.

Per main session, over 1 session
                         p10    median       p90       max
  starting context   120,010   120,010   120,010   120,010
  calls                    2         2         2         2

Cache rebuilds: calls after their transcript's first writing over half of a context above 30k
  main   gap under 5m   within both TTLs   1 rebuild   $1.00   1 after a model switch

Cache writes by thread and kind
  main       first call    $0.16
  main       rebuild       $1.00
  subagent   first call    $0.05
  subagent   incremental   $0.00
"
        );
    }

    #[test]
    fn when_rendered_as_json_then_amounts_are_dollars_and_groups_run_most_costly_first() {
        let value: serde_json::Value = serde_json::from_str(&to_json(&report(&session()))).unwrap();

        assert_eq!(value["cost"]["total_usd"], json!(1.36606));
        assert_eq!(
            value["cost"]["by_thread_and_model"][0]["model"],
            json!("claude-fable-5-1")
        );
        assert_eq!(
            value["cost"]["unpriced"],
            json!([{ "model": "claude-next", "calls": 1 }])
        );
        assert_eq!(value["measured"]["calls"], json!(4));
        assert_eq!(value["context"]["bands"][1]["calls"], json!(1));
        assert_eq!(value["sessions"]["calls"]["median"], json!(2));
    }

    #[test]
    fn when_nothing_is_measured_then_the_text_says_so_and_holds_no_cost() {
        assert_eq!(to_text(&report(&[]), &zone()), "Measured no calls.\n");
    }
}

/// A main thread of Opus 5.5 calls reading `contexts` tokens from the cache, a minute apart
/// from 10:00, for at most six calls.
fn reading(contexts: &[u64]) -> Transcript {
    const IDS: [&str; 6] = ["a", "b", "c", "d", "e", "f"];
    const TIMES: [&str; 6] = [
        "2026-10-30T10:00:00Z",
        "2026-10-30T10:01:00Z",
        "2026-10-30T10:02:00Z",
        "2026-10-30T10:03:00Z",
        "2026-10-30T10:04:00Z",
        "2026-10-30T10:05:00Z",
    ];
    assert!(
        contexts.len() <= IDS.len(),
        "a reading holds at most six calls"
    );
    let fixtures: Vec<Fixture> = contexts
        .iter()
        .zip(IDS.into_iter().zip(TIMES))
        .map(|(&context, (id, at))| Fixture {
            id,
            at,
            model: "claude-opus-5-5",
            usage: Usage {
                cache_read: context,
                ..Usage::default()
            },
        })
        .collect();
    folded(&fixtures, Range::default())
}

mod given_main_thread_calls_to_band {
    use super::*;

    #[test]
    fn when_banded_then_each_call_falls_below_the_first_edge_above_its_context() {
        let transcripts = [
            reading(&[49_999, 50_000, 100_000, 399_999, 400_000]),
            Transcript {
                thread: Thread::Subagent,
                ..reading(&[10])
            },
        ];

        let section = context_section(&transcripts);

        let calls: Vec<u64> = section.bands.iter().map(|band| band.calls).collect();
        assert_eq!(calls, [1, 1, 1, 1, 1]);
    }

    #[test]
    fn when_the_cache_read_share_is_taken_then_calls_above_the_floor_alone_are_long() {
        let floor = LONG_CONTEXT_FLOOR_TOKENS;
        let above = floor.checked_add(1).unwrap();

        let section = context_section(&[reading(&[floor, above])]);

        assert_eq!(section.cache_read, 120_000_200);
        assert_eq!(section.long_context_cache_read, 60_000_200);
    }
}

mod given_values_to_spread {
    use super::*;

    #[test]
    fn when_spread_then_each_percentile_is_the_value_at_its_floored_position() {
        let values: Vec<u64> = (1..=20).rev().collect();

        assert_eq!(
            spread(&values),
            Some(Spread {
                p10: 2,
                median: 10,
                p90: 18,
                max: 20,
            })
        );
    }

    #[test]
    fn when_a_floored_position_is_zero_then_the_percentile_takes_the_first_value() {
        assert_eq!(
            spread(&[7, 5]),
            Some(Spread {
                p10: 5,
                median: 5,
                p90: 5,
                max: 7,
            })
        );
    }

    #[test]
    fn when_there_are_no_values_then_there_is_no_spread() {
        assert_eq!(spread(&[]), None);
    }
}

mod given_main_sessions {
    use super::*;

    #[test]
    fn when_a_sessions_calls_are_written_out_of_order_then_it_starts_at_its_earliest() {
        let fixtures = [
            Fixture {
                id: "b",
                at: "2026-10-30T10:05:00Z",
                model: "claude-opus-5-5",
                usage: Usage {
                    cache_read: 9,
                    ..Usage::default()
                },
            },
            Fixture {
                id: "a",
                at: "2026-10-30T10:00:00Z",
                model: "claude-opus-5-5",
                usage: Usage {
                    cache_read: 7,
                    ..Usage::default()
                },
            },
        ];

        let section = session_section(&[folded(&fixtures, Range::default())]);

        assert_eq!(section.starting_context.map(|spread| spread.max), Some(7));
    }

    #[test]
    fn when_a_session_began_before_the_range_then_it_is_left_out_of_the_figures_and_counted() {
        let since = range(Bounds {
            since: Some(Bound::Instant(instant("2026-10-30T10:02:30Z"))),
            until: None,
        });
        let fixtures = [
            Fixture {
                id: "a",
                at: "2026-10-30T10:00:00Z",
                model: "claude-opus-5-5",
                usage: Usage::default(),
            },
            Fixture {
                id: "b",
                at: "2026-10-30T10:03:00Z",
                model: "claude-opus-5-5",
                usage: Usage::default(),
            },
        ];
        let transcripts = [folded(&fixtures, since), reading(&[5, 6])];

        let section = session_section(&transcripts);

        assert_eq!(section.sessions, 1);
        assert_eq!(section.begun_before_range, 1);
        assert_eq!(section.calls.map(|spread| spread.max), Some(2));
    }
}

/// An Opus 5.5 call `id` at `at` sending `cache_read` tokens from the cache and writing
/// `written` to it at the 1-hour TTL.
fn writing(id: &'static str, at: &'static str, usage: (u64, u64)) -> Fixture {
    let (cache_read, written) = usage;
    Fixture {
        id,
        at,
        model: "claude-opus-5-5",
        usage: Usage {
            cache_read,
            cache_write_1h: written,
            ..Usage::default()
        },
    }
}

mod given_calls_that_write_the_cache {
    use super::*;

    fn rebuilds(section: &RebuildSection) -> u64 {
        section.rebuilds.values().map(|band| band.count).sum()
    }

    #[test]
    fn when_a_gap_is_banded_then_each_band_holds_its_upper_edge() {
        let rows = [
            (TTL_5M_MILLISECONDS, Gap::UnderFiveMinutes),
            (
                TTL_5M_MILLISECONDS.checked_add(1).unwrap(),
                Gap::FiveMinutesToAnHour,
            ),
            (TTL_1H_MILLISECONDS, Gap::FiveMinutesToAnHour),
            (TTL_1H_MILLISECONDS.checked_add(1).unwrap(), Gap::OverAnHour),
        ];

        for (milliseconds, gap) in rows {
            assert_eq!(Gap::of(milliseconds), gap, "{milliseconds}");
        }
    }

    #[test]
    fn when_a_later_call_writes_over_half_of_a_long_context_then_it_is_a_rebuild() {
        let rows = [
            ((19_999, 20_001), 1),
            ((20_000, 20_000), 0),
            ((0, 30_000), 0),
            ((0, 30_001), 1),
        ];

        for (usage, count) in rows {
            let fixtures = [
                writing("a", "2026-10-30T10:00:00Z", (0, 50_000)),
                writing("b", "2026-10-30T10:01:00Z", usage),
            ];

            let section = rebuild_section(&[folded(&fixtures, Range::default())]);

            assert_eq!(rebuilds(&section), count, "{usage:?}");
        }
    }

    #[test]
    fn when_classified_then_each_writes_cost_falls_to_its_kind() {
        let fixtures = [
            writing("a", "2026-10-30T10:00:00Z", (0, 1_000_000)),
            writing("b", "2026-10-30T10:01:00Z", (1_000_000, 1_000)),
            writing("c", "2026-10-30T11:30:00Z", (0, 1_001_000)),
        ];

        let section = rebuild_section(&[folded(&fixtures, Range::default())]);

        let kinds: Vec<(Kind, u128)> = section
            .writes_by_kind
            .iter()
            .map(|((_, kind), nanodollars)| (*kind, *nanodollars))
            .collect();
        assert_eq!(
            kinds,
            [
                (Kind::First, 8_000_000_000),
                (Kind::Rebuild, 8_008_000_000),
                (Kind::Incremental, 8_000_000),
            ]
        );
        assert_eq!(
            section.rebuilds.keys().copied().collect::<Vec<_>>(),
            [(Thread::Main, Gap::OverAnHour)]
        );
    }

    #[test]
    fn when_a_rebuild_names_another_model_than_the_call_before_then_it_counts_as_a_switch() {
        let fixtures = [
            writing("a", "2026-10-30T10:00:00Z", (0, 50_000)),
            Fixture {
                model: "claude-fable-5-1",
                ..writing("b", "2026-10-30T10:01:00Z", (0, 50_000))
            },
        ];

        let section = rebuild_section(&[folded(&fixtures, Range::default())]);

        let band = section
            .rebuilds
            .get(&(Thread::Main, Gap::UnderFiveMinutes))
            .unwrap();
        assert_eq!((band.count, band.after_model_switch), (1, 1));
    }

    #[test]
    fn when_a_session_began_before_the_range_then_its_first_call_in_range_follows_the_last_before()
    {
        let fixtures = [
            writing("a", "2026-10-30T10:00:00Z", (0, 50_000)),
            writing("b", "2026-10-30T11:45:00Z", (0, 50_000)),
        ];
        let since = range(Bounds {
            since: Some(Bound::Instant(instant("2026-10-30T10:30:00Z"))),
            until: None,
        });

        let section = rebuild_section(&[folded(&fixtures, since)]);

        assert!(
            !section
                .writes_by_kind
                .contains_key(&(Thread::Main, Kind::First))
        );
        assert_eq!(
            section.rebuilds.keys().copied().collect::<Vec<_>>(),
            [(Thread::Main, Gap::OverAnHour)]
        );
    }
}
