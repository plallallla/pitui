use bevy_ecs::prelude::Component;
use pitui_core::{
    Branch, ChangedFile, Commit, FileDiff, GitPath, ReflogEntry, RemoteInfo, Repository,
    WorkingTreeChange, WorkingTreeDiff,
};

use crate::RepositoryKey;

#[derive(Component, Clone, Debug, Eq, PartialEq)]
pub struct RepositoryMetadata(pub Repository);

#[derive(Component, Clone, Debug, Eq, PartialEq)]
pub struct BranchMetadata(pub Branch);

#[derive(Component, Clone, Debug, Eq, PartialEq)]
pub struct CommitMetadata {
    pub summary: Commit,
    pub message: Option<String>,
    pub tags: Vec<String>,
}

#[derive(Component, Clone, Debug, Eq, PartialEq)]
pub struct FileMetadata(pub ChangedFile);

/// A stable directory node in a commit or working-tree file hierarchy.
/// Scope and boundary remain part of `DatasetIdentity`; metadata only carries
/// the repository-relative directory path used by projection and copy.
#[derive(Component, Clone, Debug, Eq, PartialEq)]
pub struct FileTreeDirectoryMetadata(pub GitPath);

#[derive(Component, Clone, Debug, Eq, PartialEq)]
pub struct FileChangesMetadata(pub FileDiff);

#[derive(Component, Clone, Debug, Eq, PartialEq)]
pub struct WorkingTreeFileMetadata(pub WorkingTreeChange);

#[derive(Component, Clone, Debug, Eq, PartialEq)]
pub struct WorkingTreeFileChangesMetadata(pub WorkingTreeDiff);

/// Repository-scoped data for creating one commit from the staged snapshot
/// visible when the Dataset is opened. This is deliberately not encoded as a
/// generic Interaction TextInput: it owns its semantic state, Proxy and
/// Operation Set like every other extensible Dataset.
#[derive(Component, Clone, Debug, Eq, PartialEq)]
pub struct CommitCreationMetadata {
    pub repository: RepositoryKey,
    pub message: String,
    pub error: Option<String>,
    pub staged_revision: u64,
    pub staged_paths: Vec<GitPath>,
}

#[derive(Component, Clone, Debug, Eq, PartialEq)]
pub struct ReflogEntryMetadata(pub ReflogEntry);

#[derive(Component, Clone, Debug, Eq, PartialEq)]
pub struct RemoteMetadata(pub RemoteInfo);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GitOperationStatus {
    Success,
    Failure,
    ConflictAborted,
}

impl GitOperationStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::ConflictAborted => "conflict-aborted",
        }
    }
}

#[derive(Component, Clone, Debug, Eq, PartialEq)]
pub struct GitOperationLogEntryMetadata {
    pub sequence: u64,
    pub operation: String,
    pub repository: RepositoryKey,
    pub started_at_utc: String,
    pub duration_ms: u128,
    pub status: GitOperationStatus,
    pub message: String,
    pub abort_attempted: bool,
    pub abort_result: Option<String>,
}
