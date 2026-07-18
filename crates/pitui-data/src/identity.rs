use std::path::{Path, PathBuf};

use pitui_core::{BranchName, CommitHash, GitPath};

/// Stable repository identity. Path discovery/canonicalization happens at the
/// composition boundary; ECS code never uses an `Entity` as persistent identity.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RepositoryKey(PathBuf);

impl RepositoryKey {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self(path.into())
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ChangeBoundary {
    Staged,
    Unstaged,
}

/// Stable identity for every Dataset entity that may survive projection or
/// be referred to by more than one parent collection.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum DatasetIdentity {
    GlobalRepositoriesBranches,
    Repository(RepositoryKey),
    Branch {
        repository: RepositoryKey,
        name: BranchName,
    },
    Commits {
        repository: RepositoryKey,
        branch: BranchName,
    },
    Commit {
        repository: RepositoryKey,
        hash: CommitHash,
    },
    Files {
        repository: RepositoryKey,
        commit: CommitHash,
    },
    File {
        repository: RepositoryKey,
        commit: CommitHash,
        path: GitPath,
    },
    FileChanges {
        repository: RepositoryKey,
        commit: CommitHash,
        path: GitPath,
    },
    Reflog(RepositoryKey),
    ReflogEntry {
        repository: RepositoryKey,
        selector: String,
    },
    Remotes(RepositoryKey),
    Remote {
        repository: RepositoryKey,
        name: String,
    },
    GlobalChanges,
    WorkingTreeFiles {
        repository: RepositoryKey,
        boundary: ChangeBoundary,
    },
    WorkingTreeFile {
        repository: RepositoryKey,
        boundary: ChangeBoundary,
        path: GitPath,
    },
    WorkingTreeFileChanges {
        repository: RepositoryKey,
        boundary: ChangeBoundary,
        path: GitPath,
    },
    CommitCreation(RepositoryKey),
    GlobalInteractionContext,
    GlobalGitOperationLog,
    GitOperationLogEntry(u64),
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum DatasetKind {
    RepositoriesBranches,
    Repository,
    Branch,
    Commits,
    Commit,
    Files,
    File,
    FileChanges,
    Changes,
    WorkingTreeFiles,
    WorkingTreeFile,
    WorkingTreeFileChanges,
    CommitCreation,
    Reflog,
    ReflogEntry,
    Remotes,
    Remote,
    InteractionContext,
    GitOperationLog,
    GitOperationLogEntry,
}

impl DatasetKind {
    /// Exhaustive semantic kind inventory used by strict configuration and
    /// default-template validation. Adding a new Dataset meaning requires
    /// extending this list, which makes missing Proxy/Operation contracts fail
    /// before the terminal starts.
    pub const ALL: [Self; 20] = [
        Self::RepositoriesBranches,
        Self::Repository,
        Self::Branch,
        Self::Commits,
        Self::Commit,
        Self::Files,
        Self::File,
        Self::FileChanges,
        Self::Changes,
        Self::WorkingTreeFiles,
        Self::WorkingTreeFile,
        Self::WorkingTreeFileChanges,
        Self::CommitCreation,
        Self::Reflog,
        Self::ReflogEntry,
        Self::Remotes,
        Self::Remote,
        Self::InteractionContext,
        Self::GitOperationLog,
        Self::GitOperationLogEntry,
    ];
}

impl DatasetIdentity {
    /// The semantic kind is part of stable identity, not a caller-selected
    /// hint. The ECS kernel checks this mapping whenever an Entity is ensured.
    pub const fn kind(&self) -> DatasetKind {
        match self {
            Self::GlobalRepositoriesBranches => DatasetKind::RepositoriesBranches,
            Self::Repository(_) => DatasetKind::Repository,
            Self::Branch { .. } => DatasetKind::Branch,
            Self::Commits { .. } => DatasetKind::Commits,
            Self::Commit { .. } => DatasetKind::Commit,
            Self::Files { .. } => DatasetKind::Files,
            Self::File { .. } => DatasetKind::File,
            Self::FileChanges { .. } => DatasetKind::FileChanges,
            Self::GlobalChanges => DatasetKind::Changes,
            Self::WorkingTreeFiles { .. } => DatasetKind::WorkingTreeFiles,
            Self::WorkingTreeFile { .. } => DatasetKind::WorkingTreeFile,
            Self::WorkingTreeFileChanges { .. } => DatasetKind::WorkingTreeFileChanges,
            Self::CommitCreation(_) => DatasetKind::CommitCreation,
            Self::Reflog(_) => DatasetKind::Reflog,
            Self::ReflogEntry { .. } => DatasetKind::ReflogEntry,
            Self::Remotes(_) => DatasetKind::Remotes,
            Self::Remote { .. } => DatasetKind::Remote,
            Self::GlobalInteractionContext => DatasetKind::InteractionContext,
            Self::GlobalGitOperationLog => DatasetKind::GitOperationLog,
            Self::GitOperationLogEntry(_) => DatasetKind::GitOperationLogEntry,
        }
    }
}
