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
