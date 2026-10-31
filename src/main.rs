//! The shell: reads the arguments, stdin, the clock, the environment, git, and the day's
//! spend snapshot, prints what the core renders from them, and ends with the exit code the
//! core names.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command as Process, ExitCode, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use claude_code_transcript::lines::{LINES_PER_TRANSCRIPT_MAX, Lines, Piece};
use claude_code_transcript::record::WHOLE_LINE;
use claude_code_transcript::store::{self, AgentType, Folder, Found, StoreFault};
use jiff::tz::TimeZone;
use jiff::{Timestamp, Zoned};
use meter::cli::{self, Command, Format, ReportArguments};
use meter::report::{
    Fold, Gathered, Line, Location, Range, Report, Selection, TRANSCRIPTS_MAX, Thread, Transcript,
    agent_type, read_line, to_json, to_text,
};
use meter::statusline::{
    DayStartInput, Facts, GitState, Input, Snapshot, day_start, parse_porcelain, render, to_ansi,
};
use meter::{exit_code, message};

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
            print!("{}", message::HELP);
            exit_code::SUCCESS
        }
        Ok(Command::Version) => {
            println!("meter {}", env!("CARGO_PKG_VERSION"));
            exit_code::SUCCESS
        }
        Ok(Command::Statusline) => run_statusline(),
        Ok(Command::Report(arguments)) => run_report(&arguments),
        Err(fault) => {
            eprint!("{}", message::usage_fault(&fault));
            exit_code::USAGE
        }
    };
    ExitCode::from(code)
}

fn run_statusline() -> u8 {
    let mut text = String::new();
    let read = io::stdin().take(STDIN_BYTES_MAX).read_to_string(&mut text);
    if let Err(error) = read {
        eprint!("{}", message::unreadable_input(&error.to_string()));
        return exit_code::UNREADABLE_INPUT;
    }
    let value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(error) => {
            eprint!("{}", message::unreadable_input(&error.to_string()));
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
            eprint!("{}", message::snapshot_unread(&path, &error));
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
            eprint!("{}", message::snapshot_unwritten(&path, &error));
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

fn run_report(arguments: &ReportArguments) -> u8 {
    let zone = TimeZone::system();
    let range = match Range::resolve(arguments.bounds, &zone) {
        Ok(range) => range,
        Err(fault) => {
            eprint!("{}", message::range_fault(&fault));
            return exit_code::USAGE;
        }
    };
    let projects = match resolve_projects(&arguments.projects) {
        Ok(projects) => projects,
        Err(project) => {
            eprint!("{}", message::project_unresolved(project));
            return exit_code::USAGE;
        }
    };
    let selection = Selection {
        sessions: arguments.sessions.clone(),
        projects,
    };
    let Some(home) = std::env::var_os("HOME") else {
        eprint!("{}", message::home_unset());
        return exit_code::UNREADABLE_STORE;
    };
    let store = store::projects_folder(Path::new(&home));
    let found = match find_transcripts(&store) {
        Ok(found) => found,
        Err(unfound) => {
            eprint!("{}", unfound_message(&store, &unfound));
            return exit_code::UNREADABLE_STORE;
        }
    };
    let selected: Vec<&(Location, Found)> = found
        .iter()
        .filter(|(location, _)| selection.selects(location))
        .collect();
    let (transcripts, unread) = read_transcripts(&selected, range);
    let report = Report::of(Gathered {
        range,
        selection: &selection,
        transcripts: &transcripts,
        unread,
        cap: arguments.cap,
    });
    let printed = match arguments.format {
        Format::Text => to_text(&report, &zone),
        Format::Json => to_json(&report),
    };
    let written = io::stdout().lock().write_all(printed.as_bytes());
    match written {
        Ok(()) => exit_code::SUCCESS,
        Err(_) => exit_code::OUTPUT,
    }
}

/// Each `--project` as the directory name the store keeps it under. A directory that
/// exists is resolved through `..` and symbolic links; one that no longer exists, such as
/// a removed scratch directory, is taken as written, made absolute.
fn resolve_projects(projects: &[String]) -> Result<Vec<String>, &str> {
    let mut resolved = Vec::with_capacity(projects.len());
    for project in projects {
        let path = match std::fs::canonicalize(project) {
            Ok(path) => path,
            Err(_) => match std::path::absolute(project) {
                Ok(path) => path.components().collect::<PathBuf>(),
                Err(_) => return Err(project),
            },
        };
        assert!(path.is_absolute(), "a resolved project path is absolute");
        let Some(text) = path.to_str() else {
            return Err(project);
        };
        resolved.push(store::folder_name_of(text));
    }
    assert_eq!(
        resolved.len(),
        projects.len(),
        "each project resolves to one folder"
    );
    Ok(resolved)
}

/// Why the store's transcripts could not be found.
enum Unfound {
    /// A folder of the store could not be listed, or passed a bound of the walk.
    Unlisted(StoreFault),
    /// The store holds more than `TRANSCRIPTS_MAX` transcripts.
    TooLarge,
}

/// Every transcript in the store with where it sits, folder by folder in name order and
/// each folder's in path order, at most `TRANSCRIPTS_MAX` of them.
fn find_transcripts(projects: &Path) -> Result<Vec<(Location, Found)>, Unfound> {
    let folders = store::folders(projects).map_err(Unfound::Unlisted)?;
    let mut found = Vec::new();
    for folder in &folders {
        let transcripts = store::transcripts_in(folder).map_err(Unfound::Unlisted)?;
        for transcript in transcripts {
            if found.len() == TRANSCRIPTS_MAX {
                return Err(Unfound::TooLarge);
            }
            let location = location_of(folder, &transcript);
            found.push((location, transcript));
        }
    }
    assert!(
        found.len() <= TRANSCRIPTS_MAX,
        "the transcripts found stop at their bound"
    );
    Ok(found)
}

fn location_of(folder: &Folder, found: &Found) -> Location {
    let thread = match found.thread {
        store::Thread::Main => Thread::Main,
        store::Thread::Subagent { .. } => Thread::Subagent,
    };
    Location {
        project: folder.name.clone(),
        session: found.session.clone(),
        thread,
    }
}

fn unfound_message(projects: &Path, unfound: &Unfound) -> String {
    match unfound {
        Unfound::Unlisted(StoreFault::Unlisted { path, kind }) => {
            message::store_unread(path, *kind)
        }
        Unfound::Unlisted(StoreFault::TooManyEntries { path }) => {
            message::store_too_large(path, store::FOLDER_ENTRIES_MAX)
        }
        Unfound::Unlisted(StoreFault::TooManyTranscripts { path }) => {
            message::store_too_many_transcripts(path, store::TRANSCRIPTS_PER_FOLDER_MAX)
        }
        Unfound::TooLarge => message::store_too_many_transcripts(projects, TRANSCRIPTS_MAX),
    }
}

/// Reads each transcript. A transcript that cannot be read is named on stderr and counted,
/// and the report goes on without it.
fn read_transcripts(selected: &[&(Location, Found)], range: Range) -> (Vec<Transcript>, u64) {
    let mut transcripts = Vec::new();
    let mut unread: u64 = 0;
    for found in selected {
        match read_transcript(found, range) {
            Ok(Some(transcript)) => transcripts.push(transcript),
            Ok(None) => {}
            Err(error) => {
                eprint!("{}", message::transcript_unread(&found.1.path, &error));
                let Some(more) = unread.checked_add(1) else {
                    unreachable!("the transcripts read are bounded far below a u64")
                };
                unread = more;
            }
        }
    }
    (transcripts, unread)
}

/// The transcript's calls in the range, or nothing where its last write falls before the
/// range starts.
fn read_transcript(found: &(Location, Found), range: Range) -> io::Result<Option<Transcript>> {
    let (location, transcript) = found;
    let written = std::fs::metadata(&transcript.path)?.modified()?;
    let written = Timestamp::try_from(written).map_err(io::Error::other)?;
    if !range.may_hold_records_written_until(written) {
        return Ok(None);
    }
    let fold = fold_transcript(&transcript.path, range)?;
    let folded = fold.finish(location.session.clone(), location.thread);
    let agent_type = match &transcript.thread {
        store::Thread::Main => None,
        store::Thread::Subagent { agent_type, .. } => subagent_type(&transcript.path, agent_type),
    };
    Ok(Some(Transcript {
        agent_type,
        ..folded
    }))
}

/// The fold of the transcript's lines, under the shared parser's bounds on them.
fn fold_transcript(path: &Path, range: Range) -> io::Result<Fold> {
    let mut lines = Lines::open(path)?;
    let mut fold = Fold::new(range);
    for _ in 0..=LINES_PER_TRANSCRIPT_MAX {
        let piece = lines.next_piece()?;
        let line = match piece {
            Piece::Text { text, .. } => read_line(&text),
            Piece::TooLong { .. } | Piece::NotText { .. } => Line::Malformed(WHOLE_LINE),
            Piece::End => return Ok(fold),
            Piece::PastTheBound => {
                eprint!(
                    "{}",
                    message::transcript_truncated(path, LINES_PER_TRANSCRIPT_MAX)
                );
                return Ok(fold);
            }
        };
        fold.take(line);
    }
    unreachable!("a read ends within {LINES_PER_TRANSCRIPT_MAX} lines and the piece past them")
}

/// The subagent's type as its meta file names it, naming on stderr a meta file that could
/// not be read.
fn subagent_type(transcript: &Path, read: &AgentType) -> Option<String> {
    if let AgentType::Unread(kind) = read {
        eprint!("{}", message::meta_unread(transcript, *kind));
    }
    agent_type(read)
}
