//! The checkout's git state, read from the output of `git status --porcelain=v2 --branch`,
//! git's documented machine-readable format.
//! <https://git-scm.com/docs/git-status#_porcelain_format_version_2> — checked 2026-10-30

/// The length of a commit name shown for a detached HEAD.
const SHORT_COMMIT_LENGTH: usize = 7;

/// Where the checkout's HEAD stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Head {
    Branch(String),
    /// A detached HEAD, by its commit's short name.
    Detached(String),
}

/// The checkout's HEAD and which kinds of change it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitState {
    pub head: Head,
    pub staged: bool,
    pub unstaged: bool,
    pub untracked: bool,
}

/// Why porcelain output named no HEAD to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PorcelainFault {
    MissingHead,
    MissingCommit,
}

/// Reads the HEAD and the change markers from porcelain v2 output.
///
/// An ordinary, renamed, or unmerged entry's first status letter is its staged state and its
/// second the unstaged one, where `.` marks no change.
///
/// # Errors
///
/// Returns the fault where the output names no branch, or a detached HEAD with no commit.
///
/// # Panics
///
/// Where a branch head line names an empty branch, which its prefix match excludes.
pub fn parse_porcelain(output: &str) -> Result<GitState, PorcelainFault> {
    let mut branch = None;
    let mut commit = None;
    let mut state = GitState {
        head: Head::Branch(String::new()),
        staged: false,
        unstaged: false,
        untracked: false,
    };
    for line in output.lines() {
        if let Some(name) = line.strip_prefix("# branch.head ") {
            branch = Some(name);
        } else if let Some(name) = line.strip_prefix("# branch.oid ") {
            commit = Some(name);
        } else if line.starts_with("? ") {
            state.untracked = true;
        } else if let Some(codes) = entry_codes(line) {
            let mut letters = codes.chars();
            state.staged |= letters.next().is_some_and(|letter| letter != '.');
            state.unstaged |= letters.next().is_some_and(|letter| letter != '.');
        } else {
            // Headers and entry kinds the status line does not show, such as ignored files.
        }
    }
    state.head = match branch {
        None => return Err(PorcelainFault::MissingHead),
        Some("(detached)") => {
            let short = commit.and_then(|name| name.get(..SHORT_COMMIT_LENGTH));
            let Some(short) = short else {
                return Err(PorcelainFault::MissingCommit);
            };
            Head::Detached(short.to_owned())
        }
        Some(name) => Head::Branch(name.to_owned()),
    };
    assert!(
        !matches!(&state.head, Head::Branch(name) if name.is_empty()),
        "a branch head line names its branch"
    );
    Ok(state)
}

/// The two status letters of an ordinary, renamed, or unmerged entry.
fn entry_codes(line: &str) -> Option<&str> {
    let rest = line
        .strip_prefix("1 ")
        .or_else(|| line.strip_prefix("2 "))
        .or_else(|| line.strip_prefix("u "))?;
    rest.get(..2)
}
