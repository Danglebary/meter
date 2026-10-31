//! The shell: reads the arguments, stdin, the clock, the environment, git, and the day's
//! spend snapshot, prints what the core renders from them, and ends with the exit code the
//! core names.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command as Process, ExitCode, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use jiff::tz::TimeZone;
use jiff::{Timestamp, Zoned};
use meter::cli::{self, Command, Format, ReportArguments};
use meter::report::{
    Fold, Gathered, LINE_BYTES_MAX, LINES_PER_TRANSCRIPT_MAX, Line, Location, META_BYTES_MAX,
    Range, Report, Selection, TRANSCRIPTS_MAX, Thread, Transcript, agent_type, project_directory,
    read_line, to_json, to_text,
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
const STORE_DIRECTORY: &str = ".claude/projects";
const SUBAGENT_DIRECTORY: &str = "subagents";
const TRANSCRIPT_EXTENSION: &str = "jsonl";
/// The most directory entries the store's walk visits; the store holds a few thousand.
const STORE_ENTRIES_MAX: usize = 1_000_000;
/// The deepest a subagent transcript sits below its session's `subagents` directory; a
/// workflow's subagents sit two levels down.
const SUBAGENT_DEPTH_MAX: usize = 4;
const TRANSCRIPT_BUFFER_BYTES: usize = 1024 * 1024;

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
    let store = PathBuf::from(home).join(STORE_DIRECTORY);
    let found = match find_transcripts(&store) {
        Ok(found) => found,
        Err(StoreFault::Unread(path, error)) => {
            eprint!("{}", message::store_unread(&path, &error));
            return exit_code::UNREADABLE_STORE;
        }
        Err(StoreFault::TooLarge(entries_max)) => {
            eprint!("{}", message::store_too_large(&store, entries_max));
            return exit_code::UNREADABLE_STORE;
        }
    };
    let selected: Vec<&(Location, PathBuf)> = found
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
        let Some(text) = path.to_str() else {
            return Err(project);
        };
        resolved.push(project_directory(text));
    }
    Ok(resolved)
}

/// Why the store's transcripts could not be found.
enum StoreFault {
    Unread(PathBuf, io::Error),
    /// A directory holds more entries, or the store more transcripts, than this bound.
    TooLarge(usize),
}

/// Every transcript in the store with where it sits, in path order: a session's main
/// thread at `<project>/<session>.jsonl`, and its subagents anywhere under
/// `<project>/<session>/subagents/`.
fn find_transcripts(store: &Path) -> Result<Vec<(Location, PathBuf)>, StoreFault> {
    let mut found = Vec::new();
    for project_path in list(store)? {
        let Some(project) = file_name(&project_path) else {
            continue;
        };
        if !project_path.is_dir() {
            continue;
        }
        for entry in list(&project_path)? {
            let Some(name) = file_name(&entry) else {
                continue;
            };
            let subagents = entry.join(SUBAGENT_DIRECTORY);
            if is_transcript(&entry) {
                let Some(session) = entry.file_stem().and_then(|stem| stem.to_str()) else {
                    continue;
                };
                let location = Location {
                    project: project.clone(),
                    session: session.to_owned(),
                    thread: Thread::Main,
                };
                found.push((location, entry));
            } else if subagents.is_dir() {
                for path in subagent_transcripts(&subagents)? {
                    let location = Location {
                        project: project.clone(),
                        session: name.clone(),
                        thread: Thread::Subagent,
                    };
                    found.push((location, path));
                }
            }
            if found.len() > TRANSCRIPTS_MAX {
                return Err(StoreFault::TooLarge(TRANSCRIPTS_MAX));
            }
        }
    }
    found.sort_by(|left, right| left.1.cmp(&right.1));
    Ok(found)
}

/// The transcripts under a session's `subagents` directory, at most `SUBAGENT_DEPTH_MAX`
/// levels down.
fn subagent_transcripts(subagents: &Path) -> Result<Vec<PathBuf>, StoreFault> {
    let mut transcripts = Vec::new();
    let mut pending = vec![(subagents.to_path_buf(), 0_usize)];
    while let Some((dir, depth)) = pending.pop() {
        for entry in list(&dir)? {
            if is_transcript(&entry) {
                transcripts.push(entry);
            } else if entry.is_dir() {
                if depth < SUBAGENT_DEPTH_MAX {
                    let Some(deeper) = depth.checked_add(1) else {
                        unreachable!("the depth stays below its bound")
                    };
                    pending.push((entry, deeper));
                }
            }
        }
        if transcripts.len() > TRANSCRIPTS_MAX {
            return Err(StoreFault::TooLarge(TRANSCRIPTS_MAX));
        }
    }
    Ok(transcripts)
}

/// The entries of a directory, at most `STORE_ENTRIES_MAX` of them.
fn list(dir: &Path) -> Result<Vec<PathBuf>, StoreFault> {
    let entries =
        std::fs::read_dir(dir).map_err(|error| StoreFault::Unread(dir.to_path_buf(), error))?;
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| StoreFault::Unread(dir.to_path_buf(), error))?;
        if paths.len() >= STORE_ENTRIES_MAX {
            return Err(StoreFault::TooLarge(STORE_ENTRIES_MAX));
        }
        paths.push(entry.path());
    }
    Ok(paths)
}

fn file_name(path: &Path) -> Option<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
}

fn is_transcript(path: &Path) -> bool {
    let named = path
        .extension()
        .is_some_and(|extension| extension == TRANSCRIPT_EXTENSION);
    if named { path.is_file() } else { false }
}

/// Reads each transcript. A transcript that cannot be read is named on stderr and counted,
/// and the report goes on without it.
fn read_transcripts(selected: &[&(Location, PathBuf)], range: Range) -> (Vec<Transcript>, u64) {
    let mut transcripts = Vec::new();
    let mut unread: u64 = 0;
    for found in selected {
        match read_transcript(found, range) {
            Ok(Some(transcript)) => transcripts.push(transcript),
            Ok(None) => {}
            Err(error) => {
                eprint!("{}", message::transcript_unread(&found.1, &error));
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
fn read_transcript(found: &(Location, PathBuf), range: Range) -> io::Result<Option<Transcript>> {
    let (location, path) = found;
    let written = std::fs::metadata(path)?.modified()?;
    let written = Timestamp::try_from(written).map_err(io::Error::other)?;
    if !range.may_hold_records_written_until(written) {
        return Ok(None);
    }
    let fold = fold_transcript(path, range)?;
    let folded = fold.finish(location.session.clone(), location.thread);
    let agent_type = match location.thread {
        Thread::Main => None,
        Thread::Subagent => read_agent_type(path),
    };
    Ok(Some(Transcript {
        agent_type,
        ..folded
    }))
}

/// The fold of the transcript's lines, at most `LINES_PER_TRANSCRIPT_MAX` of them.
fn fold_transcript(path: &Path, range: Range) -> io::Result<Fold> {
    let file = File::open(path)?;
    let mut reader = BufReader::with_capacity(TRANSCRIPT_BUFFER_BYTES, file);
    let mut fold = Fold::new(range);
    for _ in 0..LINES_PER_TRANSCRIPT_MAX {
        let line = match next_line(&mut reader)? {
            Piece::End => return Ok(fold),
            Piece::TooLong => Line::Malformed(""),
            Piece::Text(bytes) => match std::str::from_utf8(&bytes) {
                Ok(text) => read_line(text),
                Err(_) => Line::Malformed(""),
            },
        };
        fold.take(line);
    }
    eprint!(
        "{}",
        message::transcript_truncated(path, LINES_PER_TRANSCRIPT_MAX)
    );
    Ok(fold)
}

/// The type the subagent's sibling meta file names: none where the file is missing or
/// names none, and none, named on stderr, where it cannot be read.
fn read_agent_type(transcript: &Path) -> Option<String> {
    let meta = transcript.with_extension("meta.json");
    let file = match File::open(&meta) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return None,
        Err(error) => {
            eprint!("{}", message::meta_unread(&meta, &error));
            return None;
        }
    };
    let mut text = String::new();
    if let Err(error) = file.take(META_BYTES_MAX).read_to_string(&mut text) {
        eprint!("{}", message::meta_unread(&meta, &error));
        return None;
    }
    agent_type(&text)
}

/// One line of a transcript.
enum Piece {
    Text(Vec<u8>),
    /// A line longer than `LINE_BYTES_MAX`, skipped through its end.
    TooLong,
    End,
}

fn next_line(reader: &mut impl BufRead) -> io::Result<Piece> {
    let Ok(limit) = u64::try_from(LINE_BYTES_MAX) else {
        unreachable!("the line bound fits a u64")
    };
    let mut bytes = Vec::new();
    let read = reader.by_ref().take(limit).read_until(b'\n', &mut bytes)?;
    if read == 0 {
        return Ok(Piece::End);
    }
    let ended = bytes.last() == Some(&b'\n');
    if ended || read < LINE_BYTES_MAX {
        return Ok(Piece::Text(bytes));
    }
    reader.skip_until(b'\n')?;
    Ok(Piece::TooLong)
}
