//! The binary's wiring, run as a process: each case starts the built `meter` with
//! arguments, stdin, and an environment of its own, in scratch directories it creates, and
//! reads the exit status, the streams it wrote, and the files it left.

#[cfg(test)]
mod support {
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output, Stdio};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_DIRECTORY: AtomicUsize = AtomicUsize::new(0);

    pub fn run(arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_meter"))
            .args(arguments)
            .output()
            .unwrap()
    }

    /// A scratch home directory, removed when the case ends.
    pub struct Scratch {
        pub root: PathBuf,
    }

    impl Scratch {
        pub fn new() -> Self {
            let index = NEXT_DIRECTORY.fetch_add(1, Ordering::SeqCst);
            let root =
                std::env::temp_dir().join(format!("meter-cli-{}-{index}", std::process::id()));
            std::fs::create_dir_all(root.join("work")).unwrap();
            Self { root }
        }

        pub fn work(&self) -> PathBuf {
            self.root.join("work")
        }

        pub fn state(&self) -> PathBuf {
            self.root.join("state")
        }

        /// Runs `meter statusline` with `input` on stdin, the scratch root as its home, and
        /// the scratch state directory as its `XDG_STATE_HOME`.
        pub fn statusline(&self, input: &str) -> Output {
            let mut child = Command::new(env!("CARGO_BIN_EXE_meter"))
                .arg("statusline")
                .env("HOME", &self.root)
                .env("XDG_STATE_HOME", self.state())
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let mut stdin = child.stdin.take().unwrap();
            stdin.write_all(input.as_bytes()).unwrap();
            drop(stdin);
            child.wait_with_output().unwrap()
        }

        pub fn git(&self, arguments: &[&str]) {
            let status = Command::new("git")
                .arg("-C")
                .arg(self.work())
                .args(arguments)
                .status()
                .unwrap();
            assert!(status.success(), "git {arguments:?}");
        }

        /// Writes `lines` as the transcript at `relative` under the scratch home's
        /// transcript store.
        pub fn transcript(&self, relative: &str, lines: &[String]) {
            let path = self.root.join(".claude/projects").join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, lines.join("\n")).unwrap();
        }

        /// Runs `meter report` with `arguments`, the scratch root as its home, in UTC.
        pub fn report(&self, arguments: &[&str]) -> Output {
            Command::new(env!("CARGO_BIN_EXE_meter"))
                .arg("report")
                .args(arguments)
                .env("HOME", &self.root)
                .env("TZ", "UTC")
                .output()
                .unwrap()
        }
    }

    /// An Opus 5.5 call at `at` writing 1,000,000 tokens to the 1-hour cache, $8.
    fn call(id: &str, at: &str) -> String {
        serde_json::json!({
            "type": "assistant",
            "timestamp": at,
            "effort": "xhigh",
            "message": {
                "id": id,
                "model": "claude-opus-5-5",
                "usage": { "cache_creation": { "ephemeral_1h_input_tokens": 1_000_000 } }
            }
        })
        .to_string()
    }

    /// A scratch home whose store holds two sessions in project `/home/p`, the first with a
    /// subagent, and one in `/home/q`.
    pub fn store() -> Scratch {
        let scratch = Scratch::new();
        let user = serde_json::json!({ "type": "user", "message": { "content": "hi" } });
        scratch.transcript(
            "-home-p/s1.jsonl",
            &[user.to_string(), call("a", "2026-10-30T10:00:00Z")],
        );
        scratch.transcript(
            "-home-p/s1/subagents/agent-x.jsonl",
            &[call("b", "2026-10-30T10:01:00Z")],
        );
        scratch.transcript("-home-p/s2.jsonl", &[call("c", "2026-10-29T10:00:00Z")]);
        scratch.transcript("-home-q/s3.jsonl", &[call("d", "2026-10-30T11:00:00Z")]);
        scratch
    }

    /// The `measured` object of a JSON report the run printed, having succeeded.
    pub fn measured(output: &Output) -> serde_json::Value {
        assert_eq!(
            output.status.code(),
            Some(i32::from(meter::exit_code::SUCCESS)),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        report["measured"].clone()
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.root).unwrap();
        }
    }

    /// The text with every ANSI color escape, from its escape character through its `m`,
    /// removed.
    pub fn plain(text: &[u8]) -> String {
        let text = String::from_utf8(text.to_vec()).unwrap();
        let mut plain = String::new();
        let mut escaping = false;
        for letter in text.chars() {
            if escaping {
                escaping = letter != 'm';
            } else if letter == '\u{1b}' {
                escaping = true;
            } else {
                plain.push(letter);
            }
        }
        plain
    }

    pub fn now() -> i64 {
        jiff::Timestamp::now().as_second()
    }

    /// A cold session in `dir` with no usage window, its spend limit given where `spend` is.
    pub fn input(dir: &Path, spend: Option<(f64, f64)>) -> String {
        let mut input = serde_json::json!({
            "workspace": { "current_dir": dir, "project_dir": dir },
            "model": { "id": "claude-opus-5-5" },
            "effort": { "level": "xhigh" },
            "context_window": { "total_input_tokens": 143_000 },
            "prompt_cache": { "warm": false, "caching_observed": true, "expires_at": now().checked_sub(60).unwrap() },
            "cost": { "total_cost_usd": 21.45 }
        });
        if let Some((used, limit)) = spend {
            input["rate_limits"] = serde_json::json!({ "spend_limit": {
                "used_percentage": used / limit * 100.0, "resets_at": now().checked_add(864_000).unwrap(),
                "used_usd": used, "limit_usd": limit, "period": "monthly"
            }});
        }
        input.to_string()
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

mod given_statusline_input {
    use super::support::{Scratch, input, plain};

    #[test]
    fn when_run_outside_a_checkout_then_it_prints_the_lines_with_home_as_a_tilde() {
        let scratch = Scratch::new();
        let output = scratch.statusline(&input(&scratch.work(), None));

        assert_eq!(
            output.status.code(),
            Some(i32::from(meter::exit_code::SUCCESS))
        );
        assert_eq!(
            plain(&output.stdout),
            "~/work\nopus xhigh | 143k · cold · /clear\n$21.45\n"
        );
    }

    #[test]
    fn when_run_in_a_checkout_then_the_branch_and_its_markers_follow_git() {
        let scratch = Scratch::new();
        scratch.git(&["init", "--quiet", "--initial-branch=trunk"]);
        std::fs::write(scratch.work().join("notes.md"), "draft\n").unwrap();
        let output = scratch.statusline(&input(&scratch.work(), None));

        assert_eq!(
            plain(&output.stdout).lines().next(),
            Some("~/work on trunk?")
        );
    }

    #[test]
    fn when_the_project_holds_pirr_then_the_cold_hint_adds_the_pickup() {
        let scratch = Scratch::new();
        std::fs::create_dir(scratch.work().join(".pirr")).unwrap();
        let output = scratch.statusline(&input(&scratch.work(), None));

        assert_eq!(
            plain(&output.stdout).lines().nth(1),
            Some("opus xhigh | 143k · cold · /clear → /pirr:pickup")
        );
    }

    #[test]
    fn when_the_input_is_not_json_then_it_prints_nothing_and_names_the_fault() {
        let scratch = Scratch::new();
        let output = scratch.statusline("{\"model\":");

        assert_eq!(
            output.status.code(),
            Some(i32::from(meter::exit_code::UNREADABLE_INPUT))
        );
        assert_eq!(output.stdout, Vec::<u8>::new());
        assert!(
            String::from_utf8(output.stderr)
                .unwrap()
                .starts_with("meter: the status line's input is not JSON")
        );
    }
}

mod given_a_spend_limit {
    use super::support::{Scratch, input, plain};

    #[test]
    fn when_the_day_starts_then_the_snapshot_keeps_its_first_reading() {
        let scratch = Scratch::new();
        scratch.statusline(&input(&scratch.work(), Some((100.0, 1000.0))));
        let kept = std::fs::read_to_string(scratch.state().join("meter/spend-day.json")).unwrap();
        let snapshot = meter::statusline::Snapshot::from_json(&kept);

        assert_eq!(snapshot.date, jiff::Zoned::now().date());
        assert_eq!(format!("{:.2}", snapshot.used_usd), "100.00");
    }

    #[test]
    fn when_a_later_reading_arrives_then_the_days_spend_is_its_difference_from_the_first() {
        let scratch = Scratch::new();
        scratch.statusline(&input(&scratch.work(), Some((100.0, 1000.0))));
        let output = scratch.statusline(&input(&scratch.work(), Some((110.0, 1000.0))));
        let lines = plain(&output.stdout);

        assert!(
            lines
                .lines()
                .nth(2)
                .unwrap()
                .starts_with("$21.45 | $10 / $"),
            "{lines}"
        );
    }
}

mod given_a_transcript_store {
    use super::support::{Scratch, measured, store};

    #[test]
    fn when_reported_then_every_transcript_is_measured_and_costed() {
        let scratch = store();

        let output = scratch.report(&[]);

        let text = String::from_utf8(output.stdout).unwrap();
        let lines: Vec<&str> = text.lines().take(4).collect();
        assert_eq!(
            lines,
            [
                "Measured 3 sessions and 1 subagent: 4 calls, 2026-10-29 10:00 to 2026-10-30 11:00.",
                "",
                "Cost at list price, as of 2026-10-06: $32.00",
                "",
            ]
        );
    }

    #[test]
    fn when_a_session_is_named_then_its_subagents_are_measured_with_it_alone() {
        let scratch = store();

        let output = scratch.report(&["--session", "s1", "--format", "json"]);

        let measured = measured(&output);
        assert_eq!(measured["sessions"], 1);
        assert_eq!(measured["subagents"], 1);
        assert_eq!(measured["calls"], 2);
    }

    #[test]
    fn when_a_project_is_named_then_its_sessions_alone_are_measured() {
        let scratch = store();

        let output = scratch.report(&["--project", "/home/q", "--format", "json"]);

        assert_eq!(measured(&output)["calls"], 1);
    }

    #[test]
    fn when_a_range_is_given_then_records_outside_it_are_left_out() {
        let scratch = store();

        let output = scratch.report(&[
            "--since",
            "2026-10-30",
            "--until",
            "2026-10-30T10:30:00Z",
            "--format",
            "json",
        ]);

        let measured = measured(&output);
        assert_eq!(measured["calls"], 2);
        assert_eq!(measured["first_call"], "2026-10-30T10:00:00Z");
    }

    #[test]
    fn when_the_home_holds_no_store_then_it_exits_with_the_store_code_naming_the_store() {
        let scratch = Scratch::new();

        let output = scratch.report(&[]);

        let stderr = String::from_utf8(output.stderr).unwrap();
        assert_eq!(
            output.status.code(),
            Some(i32::from(meter::exit_code::UNREADABLE_STORE))
        );
        assert_eq!(String::from_utf8(output.stdout).unwrap(), "");
        assert!(
            stderr.starts_with("meter: the transcript store at "),
            "{stderr}"
        );
    }

    #[test]
    fn when_since_falls_after_until_then_it_exits_with_the_usage_code() {
        let scratch = store();

        let output = scratch.report(&["--since", "2026-10-30", "--until", "2026-10-29"]);

        assert_eq!(
            output.status.code(),
            Some(i32::from(meter::exit_code::USAGE))
        );
        assert_eq!(String::from_utf8(output.stdout).unwrap(), "");
    }
}

mod given_a_subagents_meta_file {
    use super::support::store;

    #[test]
    fn when_reported_then_the_subagent_groups_under_the_type_it_names() {
        let scratch = store();
        scratch.transcript(
            "-home-p/s1/subagents/agent-x.meta.json",
            &[r#"{"agentType":"Explore"}"#.to_owned()],
        );

        let output = scratch.report(&["--format", "json"]);

        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["subagents"][0]["type"], "Explore");
        assert_eq!(report["subagents"][0]["agents"], 1);
    }
}
