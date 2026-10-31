//! The report's core against values: transcript lines built as JSON in each case are read,
//! folded into calls, priced, measured, and rendered, and each case asserts on what one of
//! those steps returns. Times are placed in a fixed zone seven hours behind UTC.

use super::render::{Portion, dollars, millions, share};
use super::*;
use crate::cli::{Bound, Bounds};
use jiff::civil::date;
use jiff::tz::{TimeZone, offset};
use serde_json::json;

use claude_code_transcript::record::{
    ResultPart, TOKENS_PER_FIELD_MAX, ToolResult, UserBlock, class_of,
};
use claude_code_transcript::store::AgentType;

/// The session every record a case builds names.
const SESSION: &str = "s1";

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
        "sessionId": SESSION,
        "timestamp": fixture.at,
        "effort": "xhigh",
        "message": {
            "id": fixture.id,
            "model": fixture.model,
            "content": [],
            "usage": {
                "input_tokens": usage.input,
                "cache_read_input_tokens": usage.cache_read,
                "cache_creation_input_tokens": written,
                "cache_creation": {
                    "ephemeral_5m_input_tokens": usage.cache_write_5m,
                    "ephemeral_1h_input_tokens": usage.cache_write_1h
                },
                "output_tokens": usage.output,
                "output_tokens_details": { "thinking_tokens": usage.thinking }
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
                thinking: 2,
            },
        }
    }

    #[test]
    fn when_it_is_an_assistant_record_carrying_usage_then_its_fields_are_read() {
        let line = record(&fixture());

        assert_eq!(
            read_line(&line),
            Line::Record {
                record: Record {
                    message_id: "a".to_owned(),
                    timestamp: instant("2026-10-30T10:00:00Z"),
                    model: "claude-opus-5-5".to_owned(),
                    effort: Some("xhigh".to_owned()),
                    usage: fixture().usage,
                },
                tool_calls: Vec::new(),
            }
        );
    }

    #[test]
    fn when_it_holds_no_record_the_report_reads_then_it_is_skipped() {
        let rows = [
            json!({ "type": "summary", "summary": "the assistant and the user" }).to_string(),
            json!({ "type": "user", "sessionId": SESSION, "timestamp": "2026-10-30T10:00:00Z", "message": { "content": [] } }).to_string(),
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
            ("/sessionId", json!("")),
            ("/message/id", json!(null)),
            ("/message/usage", json!(null)),
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

    /// A call to `model` reporting `usage`, made after every rate change the table holds.
    fn call(model: &str, usage: Usage) -> Call {
        Call {
            timestamp: instant("2026-10-30T12:00:00Z"),
            model: model.to_owned(),
            effort: None,
            usage,
        }
    }

    #[test]
    fn when_a_call_is_priced_then_each_bucket_takes_its_rate_and_writes_their_multiple() {
        let usage = Usage {
            input: 1_000_000,
            cache_read: 1_000_000,
            cache_write_5m: 1_000_000,
            cache_write_1h: 1_000_000,
            output: 1_000_000,
            thinking: 0,
        };
        let opus = call("claude-opus-5-5", usage);

        assert_eq!(
            cost(&opus, &opus.usage),
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

        let short_call = call("claude-haiku-5-5", short);
        let long_call = call("claude-haiku-5-5", long);

        let short_cost = cost(&short_call, &short_call.usage).unwrap();
        let long_cost = cost(&long_call, &long_call.usage).unwrap();

        assert_eq!((short_cost.input, short_cost.output), (10_000_000, 500));
        assert_eq!((long_cost.input, long_cost.output), (50_000_500, 2_500));
    }

    #[test]
    fn when_the_table_lacks_the_model_then_the_call_is_unpriced() {
        let unknown = call("claude-next", Usage::default());

        assert_eq!(cost(&unknown, &unknown.usage), None);
    }

    #[test]
    fn when_a_rate_changed_then_each_call_takes_the_rate_standing_on_its_day_in_utc() {
        let usage = Usage {
            cache_read: 1_000_000,
            ..Usage::default()
        };
        let cases = [
            ("2026-10-06T23:59:59Z", 200_000_000),
            ("2026-10-07T00:00:00Z", 100_000_000),
            ("2026-10-06T20:00:00-07:00", 100_000_000),
        ];

        for (at, cache_read) in cases {
            let sonnet = Call {
                timestamp: instant(at),
                ..call("claude-sonnet-5-5", usage)
            };
            let priced = cost(&sonnet, &sonnet.usage).unwrap();

            assert_eq!(priced.cache_read, cache_read, "a call made at {at}");
        }
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
            cap: None,
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

Cost at list price, as of 2026-10-30: $1.37

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

Subagents by type, with one subagent's mean cost, calls, and context
            agents    cost   cost each   calls each   starts at   peaks at
  unknown        1   $0.07       $0.07            2       40.0k      40.0k

What fills the main thread's context: an item counts its characters over 4 as tokens, and each later call re-reads them at its own cache-read rate.

The model's own output
  output            0.0M tokens   $0.00
  of it, thinking   0.0M tokens   $0.00

Context cap: none given; `--cap <tokens>` simulates restarting each main session through a pickup before a call would pass it.
  No pickup turn opens a main session in the range.
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

mod given_a_subagents_meta_file {
    use super::*;

    #[test]
    fn when_it_names_a_type_then_that_type_is_kept_and_otherwise_none() {
        let rows = [
            (AgentType::Named("Explore".to_owned()), Some("Explore")),
            (AgentType::Unnamed, None),
            (AgentType::NoMeta, None),
            (AgentType::Malformed, None),
            (
                AgentType::Unread(std::io::ErrorKind::PermissionDenied),
                None,
            ),
        ];

        for (read, name) in rows {
            assert_eq!(agent_type(&read).as_deref(), name, "{read:?}");
        }
    }
}

mod given_subagents_to_group {
    use super::*;

    fn agent(name: Option<&str>, contexts: &[u64]) -> Transcript {
        Transcript {
            thread: Thread::Subagent,
            agent_type: name.map(str::to_owned),
            ..reading(contexts)
        }
    }

    #[test]
    fn when_grouped_then_a_type_sums_its_agents_and_means_round_half_up() {
        let transcripts = [
            agent(Some("Explore"), &[10, 30]),
            agent(Some("Explore"), &[21]),
            reading(&[99]),
        ];

        let section = subagent_section(&transcripts);

        let explore = section.by_type.get(&Some("Explore".to_owned())).unwrap();
        assert_eq!(
            *explore,
            Agents {
                agents: 2,
                nanodollars: 12_200,
                calls: 3,
                starting_context: 31,
                peak_context: 51,
            }
        );
        assert_eq!(explore.mean(explore.starting_context), 16);
        assert_eq!(explore.mean(explore.calls), 2);
        assert_eq!(section.by_type.len(), 1);
    }

    #[test]
    fn when_a_subagents_type_is_unknown_then_it_groups_under_none() {
        let section = subagent_section(&[agent(None, &[5])]);

        assert_eq!(
            section.by_type.get(&None).map(|agents| agents.agents),
            Some(1)
        );
    }
}

mod given_a_bash_command_line {
    use super::*;

    #[test]
    fn when_its_program_is_named_then_a_leading_cd_and_variable_assignments_are_passed() {
        let rows = [
            ("git status", Some("git")),
            ("  cd /home/p && just check", Some("just")),
            ("cd /home/p; rg foo", Some("rg")),
            ("S=/tmp/scratch; jq . \"$S/a.json\"", Some("jq")),
            ("FOO=1 BAR=2 cargo test", Some("cargo")),
            ("for f in a b; do echo $f; done", Some("for")),
            ("1>&2 echo", Some("1>&2")),
            ("", None),
        ];

        for (command, name) in rows {
            assert_eq!(program(command).as_deref(), name, "{command}");
        }
    }
}

mod given_a_user_record {
    use super::*;

    #[test]
    fn when_its_text_is_classed_then_typed_text_and_a_shell_command_alone_count_as_typed() {
        let rows = [
            ("Base directory for this skill: /x", TextClass::SkillBody),
            (
                "<command-message>x</command-message><command-name>/x</command-name>",
                TextClass::SlashCommand,
            ),
            (
                "<system-reminder>x</system-reminder>",
                TextClass::HarnessText,
            ),
            ("<task-notification>x", TextClass::HarnessText),
            ("<local-command-stdout>x", TextClass::HarnessText),
            ("[Request interrupted by user]", TextClass::HarnessText),
            ("<bash-stdout>x</bash-stdout>", TextClass::HarnessText),
            ("<bash-input>ls</bash-input>", TextClass::UserTyped),
            ("please continue", TextClass::UserTyped),
        ];

        for (text, expected) in rows {
            assert_eq!(text_class(&class_of(text)), expected, "{text}");
        }
    }

    #[test]
    fn when_a_tool_results_content_is_measured_then_each_part_counts_by_its_type() {
        let parts = [
            ResultPart::Text("héllo".to_owned()),
            ResultPart::Image,
            ResultPart::Other(json!({ "type": "other" })),
        ];

        let other = json!({ "type": "other" }).to_string().chars().count();
        let expected = 5_u64
            .checked_add(IMAGE_CHARACTERS)
            .and_then(|sum| sum.checked_add(u64::try_from(other).unwrap()))
            .unwrap();
        assert_eq!(content_characters(&parts), expected);
        assert_eq!(content_characters(&[]), 0);
    }

    #[test]
    fn when_read_then_its_tool_results_and_text_are_its_items() {
        let line = json!({
            "type": "user",
            "sessionId": SESSION,
            "timestamp": "2026-10-30T10:00:00Z",
            "message": { "content": [
                { "type": "tool_result", "tool_use_id": "t1", "content": "four" },
                { "type": "text", "text": "hi" }
            ] }
        })
        .to_string();

        assert_eq!(
            read_line(&line),
            Line::Added {
                timestamp: instant("2026-10-30T10:00:00Z"),
                items: vec![
                    Item::ToolResult(ToolResult {
                        tool_use_id: "t1".to_owned(),
                        is_error: false,
                        parts: vec![ResultPart::Text("four".to_owned())],
                        persisted_output: None,
                    }),
                    Item::Text {
                        class: TextClass::UserTyped,
                        characters: 2,
                    },
                ],
                prompt: Some(Prompt::Other),
            }
        );
    }
}

mod given_items_carried_in_a_context {
    use super::*;
    use claude_code_transcript::calls::CALLS_AWAITING_MAX;

    /// An assistant record of call `id` at `at` by `model`, calling tool `t1` where `bash`
    /// names a command.
    fn call_line(id: &str, at: &str, model: &str, bash: Option<&str>) -> String {
        let content = match bash {
            Some(command) => {
                json!([{ "type": "tool_use", "id": "t1", "name": "Bash", "input": { "command": command } }])
            }
            None => json!([]),
        };
        json!({
            "type": "assistant",
            "sessionId": SESSION,
            "timestamp": at,
            "message": { "id": id, "model": model, "content": content, "usage": { "output_tokens": 10 } }
        })
        .to_string()
    }

    fn result_line(at: &str, characters: usize) -> String {
        json!({
            "type": "user",
            "sessionId": SESSION,
            "timestamp": at,
            "message": { "content": [{ "type": "tool_result", "tool_use_id": "t1", "content": "x".repeat(characters) }] }
        })
        .to_string()
    }

    fn transcript(lines: &[String]) -> Transcript {
        let mut fold = Fold::new(Range::default());
        for line in lines {
            fold.take(read_line(line));
        }
        fold.finish("s1".to_owned(), Thread::Main)
    }

    /// An assistant record of call `m<index>` on 2026-10-29, calling Read under the tool id
    /// `p<index>`.
    fn read_call_line(index: usize) -> String {
        json!({
            "type": "assistant",
            "sessionId": SESSION,
            "timestamp": "2026-10-29T10:00:00Z",
            "message": {
                "id": format!("m{index}"),
                "model": "claude-opus-5-5",
                "content": [{ "type": "tool_use", "id": format!("p{index}"), "name": "Read", "input": {} }],
                "usage": { "output_tokens": 10 }
            }
        })
        .to_string()
    }

    /// The result of the tool call `p<index>` on 2026-10-29.
    fn read_result_line(index: usize) -> String {
        json!({
            "type": "user",
            "sessionId": SESSION,
            "timestamp": "2026-10-29T10:00:01Z",
            "message": { "content": [{ "type": "tool_result", "tool_use_id": format!("p{index}"), "content": "x" }] }
        })
        .to_string()
    }

    /// A Bash call of `git log` on 2026-10-30 returning 400 characters, then a later call.
    fn bash_exchange() -> [String; 3] {
        [
            call_line(
                "a",
                "2026-10-30T10:00:00Z",
                "claude-opus-5-5",
                Some("git log"),
            ),
            result_line("2026-10-30T10:00:01Z", 400),
            call_line("b", "2026-10-30T10:01:00Z", "claude-opus-5-5", None),
        ]
    }

    #[test]
    fn when_an_item_is_carried_then_each_later_call_re_reads_it_at_its_own_rate() {
        let lines = [
            call_line(
                "a",
                "2026-10-30T10:00:00Z",
                "claude-opus-5-5",
                Some("git log"),
            ),
            result_line("2026-10-30T10:00:01Z", 400),
            call_line("b", "2026-10-30T10:01:00Z", "claude-opus-5-5", None),
            call_line("c", "2026-10-30T10:02:00Z", "claude-fable-5-1", None),
        ];

        let carriage = transcript(&lines).carriage;

        let bash = carriage.tools.get("Bash").unwrap();
        assert_eq!(bash.tokens(), 100);
        assert_eq!(bash.nanodollars(), 45_000);
        assert_eq!(carriage.commands.get("git"), Some(bash));
    }

    #[test]
    fn when_output_is_carried_then_the_calls_after_it_alone_re_read_it() {
        let lines = [
            call_line("a", "2026-10-30T10:00:00Z", "claude-opus-5-5", None),
            call_line("b", "2026-10-30T10:01:00Z", "claude-opus-5-5", None),
        ];

        let output = transcript(&lines).carriage.output;

        assert_eq!(output.tokens, 20);
        assert_eq!(output.nanodollars, 2_000);
    }

    #[test]
    fn when_a_result_names_no_call_the_transcript_made_then_it_is_left_out() {
        let lines = [
            result_line("2026-10-30T10:00:01Z", 400),
            call_line("b", "2026-10-30T10:01:00Z", "claude-opus-5-5", None),
        ];

        assert!(transcript(&lines).carriage.tools.is_empty());
    }

    #[test]
    fn when_a_calls_result_is_written_twice_then_the_second_is_left_out() {
        let [call, result, later] = bash_exchange();
        let lines = [call, result.clone(), result, later];

        let carriage = transcript(&lines).carriage;

        assert_eq!(carriage.tools.get("Bash").unwrap().tokens(), 100);
    }

    #[test]
    fn when_results_fall_before_the_range_then_their_calls_await_no_longer() {
        let mut lines = Vec::new();
        for index in 0..CALLS_AWAITING_MAX {
            lines.push(read_call_line(index));
            lines.push(read_result_line(index));
        }
        for line in bash_exchange() {
            lines.push(line);
        }
        let since = range(Bounds {
            since: Some(Bound::Date(date(2026, 10, 30))),
            until: None,
        });

        let carriage = folded_lines(&lines, since).carriage;

        assert_eq!(carriage.tools.get("Bash").unwrap().tokens(), 100);
    }

    #[test]
    fn when_more_calls_await_than_the_pairings_bound_then_a_later_calls_result_is_left_out() {
        let mut lines = Vec::new();
        for index in 0..CALLS_AWAITING_MAX {
            lines.push(read_call_line(index));
        }
        for line in bash_exchange() {
            lines.push(line);
        }

        let carriage = transcript(&lines).carriage;

        assert_eq!(carriage.tools.get("Bash"), None);
    }
}

const PICKUP_PROMPT: &str =
    "<command-message>pirr:pickup</command-message>\n<command-name>/pirr:pickup</command-name>";

/// A user record at `at` whose content is the text `text`.
fn prompt_line(at: &str, text: &str) -> String {
    json!({ "type": "user", "sessionId": SESSION, "timestamp": at, "message": { "content": text } })
        .to_string()
}

/// An Opus 5.5 call `id` at `at` reading `cache_read` tokens from the cache.
fn reading_line(id: &'static str, at: &'static str, cache_read: u64) -> String {
    record(&Fixture {
        id,
        at,
        model: "claude-opus-5-5",
        usage: Usage {
            cache_read,
            ..Usage::default()
        },
    })
}

/// A compaction boundary at `at`.
fn compaction_line(at: &str) -> String {
    json!({ "type": "system", "subtype": "compact_boundary", "sessionId": SESSION, "timestamp": at })
        .to_string()
}

/// The main thread of session `s1` folded from `lines`, in order, over `range`.
fn folded_lines(lines: &[String], range: Range) -> Transcript {
    let mut fold = Fold::new(range);
    for line in lines {
        fold.take(read_line(line));
    }
    fold.finish("s1".to_owned(), Thread::Main)
}

/// A main session opening on a pickup turn of two calls reading 50k and then 80k tokens
/// from the cache, $0.026, then a typed prompt and a call reading 81k.
fn picked_up(range: Range) -> Transcript {
    let lines = [
        prompt_line("2026-10-30T09:00:00Z", PICKUP_PROMPT),
        reading_line("p1", "2026-10-30T09:00:01Z", 50_000),
        reading_line("p2", "2026-10-30T09:00:02Z", 80_000),
        prompt_line("2026-10-30T09:01:00Z", "go on"),
        reading_line("p3", "2026-10-30T09:01:01Z", 81_000),
    ];
    folded_lines(&lines, range)
}

mod given_a_user_records_prompt {
    use super::*;

    fn text(text: &str) -> UserBlock {
        UserBlock::Text {
            text: text.to_owned(),
            class: class_of(text),
        }
    }

    #[test]
    fn when_read_then_a_typed_text_or_slash_command_opens_a_turn_and_a_pickup_is_named() {
        let result = UserBlock::ToolResult(ToolResult {
            tool_use_id: "t1".to_owned(),
            is_error: false,
            parts: vec![ResultPart::Text("x".to_owned())],
            persisted_output: None,
        });
        let rows = [
            (text(PICKUP_PROMPT), Some(Prompt::Pickup)),
            (
                text("<command-message>x</command-message><command-name>/x</command-name>"),
                Some(Prompt::Other),
            ),
            (text("please continue"), Some(Prompt::Other)),
            (text("<bash-input>ls</bash-input>"), Some(Prompt::Other)),
            (text("<system-reminder>x</system-reminder>"), None),
            (text("[Request interrupted by user]"), None),
            (text("Base directory for this skill: /x"), None),
            (result, None),
        ];

        for (block, expected) in rows {
            let blocks = [block];
            assert_eq!(prompt(&blocks), expected, "{blocks:?}");
        }
    }
}

mod given_a_system_record {
    use super::*;

    #[test]
    fn when_it_marks_a_compaction_boundary_then_its_time_is_read() {
        assert_eq!(
            read_line(&compaction_line("2026-10-30T10:00:00Z")),
            Line::Compaction {
                timestamp: instant("2026-10-30T10:00:00Z"),
            }
        );
    }

    #[test]
    fn when_it_marks_anything_else_then_it_is_skipped() {
        let line = json!({ "type": "system", "subtype": "api_error", "timestamp": "2026-10-30T10:00:00Z" })
            .to_string();

        assert_eq!(read_line(&line), Line::Other);
    }
}

mod given_a_transcripts_prompts {
    use super::*;

    #[test]
    fn when_its_first_prompt_runs_a_pickup_then_the_turn_runs_up_to_the_second_prompt() {
        let transcript = picked_up(Range::default());

        assert_eq!(
            transcript.pickup_turn,
            Some(Turn {
                start: instant("2026-10-30T09:00:00Z"),
                end: Some(instant("2026-10-30T09:01:00Z")),
            })
        );
    }

    #[test]
    fn when_its_first_prompt_is_another_then_a_later_pickup_opens_no_pickup_turn() {
        let lines = [
            prompt_line("2026-10-30T09:00:00Z", "hello"),
            reading_line("p1", "2026-10-30T09:00:01Z", 50_000),
            prompt_line("2026-10-30T09:01:00Z", PICKUP_PROMPT),
            reading_line("p2", "2026-10-30T09:01:01Z", 80_000),
        ];

        assert_eq!(folded_lines(&lines, Range::default()).pickup_turn, None);
    }
}

mod given_a_context_cap {
    use super::*;

    const CAP_TOKENS: u64 = 180_000;
    /// What the pickup session's calls cost on the input side, as run, all under the cap.
    const PICKED_UP_INPUT_SIDE_NANODOLLARS: u128 = 42_200_000;

    fn ran(transcripts: &[Transcript]) -> Outcome {
        match cap_section(transcripts, Some(CAP_TOKENS)).simulation {
            Simulation::Ran(outcome) => outcome,
            other => panic!("the simulation ran: {other:?}"),
        }
    }

    #[test]
    fn when_no_cap_is_given_then_the_pickup_turns_are_measured_and_nothing_is_simulated() {
        let section = cap_section(&[picked_up(Range::default())], None);

        assert_eq!(
            section,
            CapSection {
                pickup: Some(Pickup {
                    turns: 1,
                    median_nanodollars: 26_000_000,
                    median_context: 80_000,
                }),
                simulation: Simulation::NoCap,
            }
        );
    }

    #[test]
    fn when_no_pickup_turn_opens_a_session_then_no_restart_is_priced() {
        let section = cap_section(&[reading(&[100_000, 200_000])], Some(CAP_TOKENS));

        assert_eq!(
            section.simulation,
            Simulation::NoPickupTurn { cap: CAP_TOKENS }
        );
    }

    #[test]
    fn when_the_pickup_session_began_before_the_range_then_its_turn_is_left_out() {
        let since = Bounds {
            since: Some(Bound::Instant(instant("2026-10-30T09:00:02Z"))),
            until: None,
        };

        let section = cap_section(&[picked_up(range(since))], None);

        assert_eq!(section.pickup, None);
    }

    #[test]
    fn when_the_cap_is_at_or_below_the_context_a_pickup_turn_ends_at_then_no_restart_is_priced() {
        let section = cap_section(&[picked_up(Range::default())], Some(80_000));

        assert_eq!(
            section.simulation,
            Simulation::CapAtOrBelowPickup { cap: 80_000 }
        );
    }

    #[test]
    fn when_a_call_would_pass_the_cap_then_the_session_restarts_and_later_calls_keep_the_growth_since()
     {
        // The restart before the 200k call drops the 150k before it to the pickup's 80k,
        // so the calls send 100k, 150k, 130k, and 180k.
        let transcripts = [
            picked_up(Range::default()),
            reading(&[100_000, 150_000, 200_000, 250_000]),
        ];

        assert_eq!(
            ran(&transcripts),
            Outcome {
                cap: CAP_TOKENS,
                sessions: 2,
                sessions_restarted: 1,
                restarts: 1,
                as_run: PICKED_UP_INPUT_SIDE_NANODOLLARS + 140_000_000,
                simulated: PICKED_UP_INPUT_SIDE_NANODOLLARS + 112_000_000,
                restart_cost: 26_000_000,
            }
        );
    }

    #[test]
    fn when_a_restart_would_not_shrink_the_call_then_none_is_made() {
        // The call before the 300k call sent less than a pickup turn leaves.
        let outcome = ran(&[picked_up(Range::default()), reading(&[50_000, 300_000])]);

        assert_eq!(outcome.restarts, 0);
        assert_eq!(outcome.simulated, outcome.as_run);
    }

    #[test]
    fn when_a_drop_is_taken_then_it_comes_out_of_the_cache_read_and_then_the_writes() {
        // The 200k call reads 30k and writes 170k; the 70k drop takes the read and 40k of
        // the write, leaving 130k written at $8 per million.
        let rebuild = Fixture {
            id: "c",
            at: "2026-10-30T10:02:00Z",
            model: "claude-opus-5-5",
            usage: Usage {
                cache_read: 30_000,
                cache_write_1h: 170_000,
                ..Usage::default()
            },
        };
        let lines = [
            reading_line("a", "2026-10-30T10:00:00Z", 100_000),
            reading_line("b", "2026-10-30T10:01:00Z", 150_000),
            record(&rebuild),
        ];

        let outcome = ran(&[
            picked_up(Range::default()),
            folded_lines(&lines, Range::default()),
        ]);

        assert_eq!(
            outcome.simulated,
            PICKED_UP_INPUT_SIDE_NANODOLLARS + 50_000_000 + 1_040_000_000
        );
    }

    #[test]
    fn when_a_later_call_sends_less_than_the_drop_leaves_then_it_keeps_the_context_a_pickup_leaves()
    {
        // After the restart the 120k call loses 40k of the 70k drop, sending the 80k a
        // pickup turn leaves.
        let outcome = ran(&[
            picked_up(Range::default()),
            reading(&[100_000, 150_000, 200_000, 120_000]),
        ]);

        assert_eq!(
            outcome.simulated,
            PICKED_UP_INPUT_SIDE_NANODOLLARS + 92_000_000
        );
    }

    #[test]
    fn when_a_compaction_boundary_falls_then_the_calls_after_it_carry_no_drop() {
        // The restart before the 200k call drops 70k; after the boundary the 100k and 150k
        // calls send what they sent.
        let lines = [
            reading_line("a", "2026-10-30T10:00:00Z", 100_000),
            reading_line("b", "2026-10-30T10:01:00Z", 150_000),
            reading_line("c", "2026-10-30T10:02:00Z", 200_000),
            compaction_line("2026-10-30T10:02:30Z"),
            reading_line("d", "2026-10-30T10:03:00Z", 100_000),
            reading_line("e", "2026-10-30T10:04:00Z", 150_000),
        ];

        let outcome = ran(&[
            picked_up(Range::default()),
            folded_lines(&lines, Range::default()),
        ]);

        assert_eq!(outcome.restarts, 1);
        assert_eq!(
            outcome.simulated,
            PICKED_UP_INPUT_SIDE_NANODOLLARS + 126_000_000
        );
    }

    #[test]
    fn when_rendered_then_the_text_ends_on_the_simulation_and_the_json_carries_it() {
        let transcripts = [
            picked_up(Range::default()),
            reading(&[100_000, 150_000, 200_000, 250_000]),
        ];
        let report = Report::of(Gathered {
            range: Range::default(),
            selection: &Selection::default(),
            transcripts: &transcripts,
            unread: 0,
            cap: Some(CAP_TOKENS),
        });

        let text = to_text(&report, &zone());
        let value: serde_json::Value = serde_json::from_str(&to_json(&report)).unwrap();

        assert!(
            text.ends_with(
                "
Context cap at 180,000 tokens: each main session restarts through a pickup before a call would pass it.
  Pickup turns opening a main session: 1, a median of $0.03 each, ending at a median of 80,000 tokens.
  Restarts: 1, in 1 of 2 sessions, at $0.03.

Main-thread input side under the cap
  as run                            $0.18
  as simulated, restarts included   $0.18
  saved, at most                    $0.00   1.1%
"
            ),
            "{text}"
        );
        assert_eq!(value["cap"]["outcome"], json!("simulated"));
        assert_eq!(value["cap"]["simulated"]["saved_at_most_usd"], json!(0.002));
    }
}

mod given_a_cap_outcome {
    use super::*;

    /// A cap the comparison does not read.
    const CAP_TOKENS: u64 = 1;

    #[test]
    fn when_compared_then_the_simulated_side_and_the_restarts_together_save_or_add() {
        let rows = [
            (50, Difference::Saved(30)),
            (90, Difference::Added(10)),
            (80, Difference::Saved(0)),
        ];

        for (simulated, expected) in rows {
            let outcome = Outcome {
                cap: CAP_TOKENS,
                sessions: 1,
                sessions_restarted: 1,
                restarts: 1,
                as_run: 100,
                simulated,
                restart_cost: 20,
            };
            assert_eq!(outcome.difference(), expected, "{simulated}");
        }
    }
}
