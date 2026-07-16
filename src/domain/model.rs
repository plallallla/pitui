use std::{ffi::OsString, fmt, path::PathBuf};

/// A complete object id. Git-facing code accepts abbreviated ids too, while
/// models loaded from Git normally contain the full id.
#[derive(Clone, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CommitHash(pub String);

impl CommitHash {
    pub fn short(&self) -> &str {
        let end = self.0.len().min(8);
        &self.0[..end]
    }
}

impl fmt::Display for CommitHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BranchName(pub String);

impl fmt::Display for BranchName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// A repository-relative path as emitted by Git.
///
/// Git paths are byte strings on Unix and are not guaranteed to be UTF-8. The
/// raw representation is retained for subsequent Git argv while `display`
/// provides a lossy, safe-to-format view for the terminal.
#[derive(Clone, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct GitPath {
    raw: Vec<u8>,
    display: String,
}

impl GitPath {
    pub fn from_bytes(raw: impl Into<Vec<u8>>) -> Self {
        let raw = raw.into();
        let display = String::from_utf8_lossy(&raw).into_owned();
        Self { raw, display }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.raw
    }

    pub fn as_str(&self) -> &str {
        &self.display
    }

    #[cfg(unix)]
    pub fn to_os_string(&self) -> OsString {
        use std::os::unix::ffi::OsStringExt;
        OsString::from_vec(self.raw.clone())
    }

    #[cfg(not(unix))]
    pub fn to_os_string(&self) -> OsString {
        OsString::from(&self.display)
    }
}

impl From<String> for GitPath {
    fn from(value: String) -> Self {
        Self::from_bytes(value.into_bytes())
    }
}

impl From<&str> for GitPath {
    fn from(value: &str) -> Self {
        Self::from_bytes(value.as_bytes().to_vec())
    }
}

impl fmt::Display for GitPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.display.fmt(formatter)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WorkingTreeStatus {
    pub staged: usize,
    pub modified: usize,
    pub untracked: usize,
    pub conflicted: usize,
    pub ahead: usize,
    pub behind: usize,
}

impl WorkingTreeStatus {
    pub fn is_clean(&self) -> bool {
        self.staged == 0 && self.modified == 0 && self.untracked == 0 && self.conflicted == 0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Repository {
    pub root: PathBuf,
    pub name: String,
    pub current_branch: Option<BranchName>,
    pub head: CommitHash,
    pub status: WorkingTreeStatus,
}

/// A configured Git remote. `push_urls` contains Git's effective push URLs:
/// when no explicit `remote.<name>.pushurl` exists it is identical to
/// `fetch_urls`. Pitui intentionally exposes both sides so a split fetch/push
/// configuration can be detected and repaired before any network operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteInfo {
    pub name: String,
    pub fetch_urls: Vec<String>,
    pub push_urls: Vec<String>,
    /// The current branch reads/pulls from this remote.
    pub is_upstream: bool,
    /// A plain `git push` resolves to this remote for the current branch.
    pub is_push_target: bool,
}

impl RemoteInfo {
    pub fn urls_match(&self) -> bool {
        !self.fetch_urls.is_empty() && self.fetch_urls == self.push_urls
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BranchKind {
    Local,
    Remote,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Branch {
    pub name: BranchName,
    pub full_ref: String,
    pub kind: BranchKind,
    pub head: CommitHash,
    pub short_head: String,
    pub commit_date: String,
    pub subject: String,
    pub is_current: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Commit {
    pub hash: CommitHash,
    pub short_hash: String,
    pub author: String,
    pub authored_at: String,
    pub decorations: String,
    pub subject: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CommitList {
    pub viewing_branch: Option<BranchName>,
    pub items: Vec<Commit>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReflogEntry {
    pub hash: CommitHash,
    pub short_hash: String,
    pub selector: String,
    pub action: String,
    pub message: String,
    pub author: String,
    pub authored_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkingTreeChange {
    /// Porcelain v1 index status (`X` in `XY`). A space means unchanged.
    pub index_status: char,
    /// Porcelain v1 worktree status (`Y` in `XY`). A space means unchanged.
    pub worktree_status: char,
    pub path: GitPath,
    pub old_path: Option<GitPath>,
}

impl WorkingTreeChange {
    pub fn is_untracked(&self) -> bool {
        self.index_status == '?' && self.worktree_status == '?'
    }

    pub fn is_conflicted(&self) -> bool {
        matches!(
            (self.index_status, self.worktree_status),
            ('D', 'D')
                | ('A', 'U')
                | ('U', 'D')
                | ('U', 'A')
                | ('D', 'U')
                | ('A', 'A')
                | ('U', 'U')
        )
    }

    pub fn has_staged_changes(&self) -> bool {
        !matches!(self.index_status, ' ' | '?' | '!')
    }

    pub fn has_worktree_changes(&self) -> bool {
        !matches!(self.worktree_status, ' ' | '?' | '!')
    }

    pub fn status_code(&self) -> String {
        format!("{}{}", self.index_status, self.worktree_status)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkingTreeDiffKind {
    Staged,
    Worktree,
    Untracked,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkingTreeDiffSection {
    pub kind: WorkingTreeDiffKind,
    pub lines: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkingTreeDiff {
    pub path: GitPath,
    pub sections: Vec<WorkingTreeDiffSection>,
}

impl CommitList {
    pub fn empty() -> Self {
        Self::default()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FileChangeKind {
    Added,
    Copied { similarity: Option<u8> },
    Deleted,
    Modified,
    Renamed { similarity: Option<u8> },
    TypeChanged,
    Unmerged,
    Unknown(String),
}

impl FileChangeKind {
    pub fn marker(&self) -> &'static str {
        match self {
            Self::Added => "A",
            Self::Copied { .. } => "C",
            Self::Deleted => "D",
            Self::Modified => "M",
            Self::Renamed { .. } => "R",
            Self::TypeChanged => "T",
            Self::Unmerged => "U",
            Self::Unknown(_) => "?",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HunkSummary {
    pub header: String,
    pub additions: usize,
    pub deletions: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChangedFile {
    pub kind: FileChangeKind,
    pub path: GitPath,
    pub old_path: Option<GitPath>,
    pub additions: Option<usize>,
    pub deletions: Option<usize>,
    pub hunks: Vec<HunkSummary>,
    pub is_binary: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommitDetail {
    pub commit: Commit,
    pub author_email: String,
    pub committer: String,
    pub committer_email: String,
    pub committed_at: String,
    pub message: String,
    pub files: Vec<ChangedFile>,
}

impl CommitDetail {
    pub fn file(&self, index: Option<usize>) -> Option<&ChangedFile> {
        self.files.get(index?)
    }
}
