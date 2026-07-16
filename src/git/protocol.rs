use crate::domain::{
    Branch, BranchName, CommitDetail, CommitHash, FileDiff, GitPath, ReflogEntry, Repository,
    WorkingTreeChange, WorkingTreeDiff,
};

pub type GitJobId = u64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResetMode {
    Soft,
    Mixed,
    Hard,
}

impl ResetMode {
    pub fn flag(self) -> &'static str {
        match self {
            Self::Soft => "--soft",
            Self::Mixed => "--mixed",
            Self::Hard => "--hard",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GitRequest {
    LoadRepositoryStatus,
    LoadBranches,
    LoadCommits {
        branch: BranchName,
        limit: usize,
    },
    LoadCommitDetail {
        commit: CommitHash,
    },
    LoadFileDiff {
        commit: CommitHash,
        path: GitPath,
        old_path: Option<GitPath>,
    },
    LoadReflog {
        limit: usize,
    },
    LoadWorkingTree,
    LoadWorkingTreeDiff {
        path: GitPath,
        old_path: Option<GitPath>,
        include_staged: bool,
        include_worktree: bool,
        untracked: bool,
    },
    StagePaths {
        paths: Vec<GitPath>,
    },
    UnstagePaths {
        paths: Vec<GitPath>,
    },
    Commit {
        message: String,
    },
    Fetch,
    SwitchBranch {
        branch: BranchName,
    },
    CherryPick {
        commits: Vec<CommitHash>,
    },
    Reset {
        commit: CommitHash,
        mode: ResetMode,
    },
    Rebase {
        upstream: BranchName,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GitResponse {
    RepositoryStatusLoaded(Repository),
    BranchesLoaded(Vec<Branch>),
    CommitsLoaded {
        branch: BranchName,
        commits: Vec<crate::domain::Commit>,
    },
    CommitDetailLoaded(CommitDetail),
    FileDiffLoaded(FileDiff),
    ReflogLoaded(Vec<ReflogEntry>),
    WorkingTreeLoaded(Vec<WorkingTreeChange>),
    WorkingTreeDiffLoaded(WorkingTreeDiff),
    CommandSucceeded {
        message: String,
    },
    CommandFailed {
        command: String,
        stderr: String,
    },
    RebaseConflictAborted {
        command: String,
        stderr: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitResponseEnvelope {
    pub id: GitJobId,
    pub response: GitResponse,
}
