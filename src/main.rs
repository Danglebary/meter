//! The shell: reads the arguments, stdin, the clock, the environment, git, and the day's
//! spend snapshot, prints what the core renders from them, and ends with the exit code the
//! core names.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command as Process, ExitCode, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use jiff::Zoned;
use meter::cli::{self, Command};
use meter::statusline::{
    DayStartInput, Facts, GitState, Input, Snapshot, day_start, parse_porcelain, render, to_ansi,
};
use meter::{exit_code, report};

/// The most of stdin the status line reads; Claude Code's input runs to a few kilobytes.
const STDIN_BYTES_MAX: u64 = 1_048_576;
/// The most of git's output read; a checkout with more changes than this shows no git part.
const GIT_OUTPUT_BYTES_MAX: u64 = 1_048_576;
const GIT_WAIT_MS: u64 = 500;
const GIT_POLL_MS: u64 = 5;
const GIT_POLLS_MAX: u64 = GIT_WAIT_MS / GIT_POLL_MS + 1;
const SNAPSHOT_FILE: &str = "meter/spend-day.json";

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
        Ok(Command::Statusline) => run_statusline(),
        Err(fault) => {
            eprint!("{}", report::usage_fault(&fault));
            exit_code::USAGE
        }
    };
    ExitCode::from(code)
}

fn run_statusline() -> u8 {
    let mut text = String::new();
    let read = io::stdin().take(STDIN_BYTES_MAX).read_to_string(&mut text);
    if let Err(error) = read {
        eprint!("{}", report::unreadable_input(&error.to_string()));
        return exit_code::UNREADABLE_INPUT;
    }
    let value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(error) => {
            eprint!("{}", report::unreadable_input(&error.to_string()));
            return exit_code::UNREADABLE_INPUT;
        }
    };
    let input = Input::read(&value);
    let now = Zoned::now();
    let home = std::env::var("HOME").ok();
    let git = input.current_dir.value().and_then(|dir| read_git(dir));
    let pirr_checkout = input
        .project_dir
        .value()
        .is_some_and(|dir| Path::new(dir).join(".pirr").is_dir());
    let facts = Facts {
        now: &now,
        home: home.as_deref(),
        git: git.as_ref(),
        pirr_checkout,
        day_start_usd: day_start_usd(&input, &now),
    };
    let lines = render(&input, &facts);
    let printed = to_ansi(&lines);
    let written = io::stdout().lock().write_all(printed.as_bytes());
    match written {
        Ok(()) => exit_code::SUCCESS,
        Err(_) => exit_code::OUTPUT,
    }
}

/// Why git's state could not be read.
enum GitFailure {
    Spawn,
    TimedOut,
    Exited,
    Unreadable,
    Unreaped,
}

fn read_git(dir: &str) -> Option<GitState> {
    let output = run_git(dir).ok()?;
    parse_porcelain(&output).ok()
}

/// Runs git's porcelain status in `dir`, reading its output on a thread of its own so a
/// full pipe cannot hold git past the wait.
fn run_git(dir: &str) -> Result<String, GitFailure> {
    let spawned = Process::new("git")
        .args([
            "-C",
            dir,
            "--no-optional-locks",
            "status",
            "--porcelain=v2",
            "--branch",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = spawned else {
        return Err(GitFailure::Spawn);
    };
    let Some(stdout) = child.stdout.take() else {
        unreachable!("git's stdout was piped")
    };
    let reader = thread::spawn(move || {
        let mut output = String::new();
        stdout
            .take(GIT_OUTPUT_BYTES_MAX)
            .read_to_string(&mut output)
            .map(|_| output)
    });
    let status = wait_bounded(&mut child);
    let Ok(read) = reader.join() else {
        unreachable!("the git reader thread panicked")
    };
    let status = status?;
    if status.success() {
        read.map_err(|_| GitFailure::Unreadable)
    } else {
        Err(GitFailure::Exited)
    }
}

/// Waits for git until the bound, killing it past that.
fn wait_bounded(child: &mut Child) -> Result<ExitStatus, GitFailure> {
    let Some(deadline) = Instant::now().checked_add(Duration::from_millis(GIT_WAIT_MS)) else {
        unreachable!("the wait is a fraction of a second")
    };
    for _ in 0..GIT_POLLS_MAX {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(GIT_POLL_MS));
            }
            Ok(None) | Err(_) => break,
        }
    }
    match child.kill() {
        Ok(()) => {}
        // Git exited between the last poll and the kill.
        Err(error) if error.kind() == io::ErrorKind::InvalidInput => {}
        Err(_) => return Err(GitFailure::TimedOut),
    }
    match child.wait() {
        Ok(_) => Err(GitFailure::TimedOut),
        Err(_) => Err(GitFailure::Unreaped),
    }
}

/// The spend limit's spend at the day's first reading, kept in the snapshot file.
fn day_start_usd(input: &Input, now: &Zoned) -> Option<f64> {
    let limit = input.spend_limit.value()?;
    let dollars = limit.dollars.value()?;
    let path = snapshot_path()?;
    let previous = match read_snapshot(&path) {
        Ok(previous) => previous,
        Err(error) => {
            eprint!("{}", report::snapshot_unread(&path, &error));
            return None;
        }
    };
    let start = day_start(&DayStartInput {
        previous: previous.as_ref(),
        now,
        resets_at: limit.resets_at,
        used_usd: dollars.used,
    });
    if let Some(snapshot) = &start.write {
        if let Err(error) = write_snapshot(&path, snapshot) {
            eprint!("{}", report::snapshot_unwritten(&path, &error));
        }
    }
    Some(start.used_usd)
}

/// `$XDG_STATE_HOME/meter/spend-day.json`, the state directory falling back to
/// `~/.local/state` where the variable is unset or relative, as the XDG base directory
/// specification has it.
/// <https://specifications.freedesktop.org/basedir-spec/latest/> — checked 2026-10-30
fn snapshot_path() -> Option<PathBuf> {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute());
    let state = match state {
        Some(state) => state,
        None => PathBuf::from(std::env::var_os("HOME")?).join(".local/state"),
    };
    Some(state.join(SNAPSHOT_FILE))
}

fn read_snapshot(path: &Path) -> io::Result<Option<Snapshot>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(Snapshot::from_json(&text))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Replaces the snapshot whole: the text goes to a temporary file beside it, renamed over
/// the old, so a session reading at the same moment sees one snapshot or the other.
fn write_snapshot(path: &Path, snapshot: &Snapshot) -> io::Result<()> {
    let Some(dir) = path.parent() else {
        unreachable!("the snapshot path names a file inside the state directory")
    };
    std::fs::create_dir_all(dir)?;
    let temporary = dir.join(format!(".spend-day.json.{}.tmp", std::process::id()));
    std::fs::write(&temporary, snapshot.to_json())?;
    std::fs::rename(&temporary, path)
}
