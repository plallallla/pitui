use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use crate::domain::{
    Branch, BranchKind, BranchName, ChangedFile, Commit, CommitDetail, CommitHash, CommitList,
    FileDiff, GitPath, ReflogEntry, RemoteInfo, Repository, WorkingTreeChange,
};

/// Stable identity of a repository for the lifetime of one application model.
/// The numeric value is never a filtered-list index.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RepositoryId(pub usize);

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BranchId {
    pub repository: RepositoryId,
    pub name: BranchName,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CommitId {
    pub repository: RepositoryId,
    pub hash: CommitHash,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct FileId {
    pub commit: CommitId,
    pub path: GitPath,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum Resource<T> {
    #[default]
    NotLoaded,
    Loading,
    Ready(T),
    Failed(String),
}

impl<T> Resource<T> {
    pub fn ready(&self) -> Option<&T> {
        match self {
            Self::Ready(value) => Some(value),
            Self::NotLoaded | Self::Loading | Self::Failed(_) => None,
        }
    }

    pub fn ready_mut(&mut self) -> Option<&mut T> {
        match self {
            Self::Ready(value) => Some(value),
            Self::NotLoaded | Self::Loading | Self::Failed(_) => None,
        }
    }

    pub fn is_loading(&self) -> bool {
        matches!(self, Self::Loading)
    }

    fn reset_loading(&mut self) {
        if self.is_loading() {
            *self = Self::NotLoaded;
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommitMetadata {
    pub author_email: String,
    pub committer: String,
    pub committer_email: String,
    pub committed_at: String,
    pub message: String,
}

impl From<&CommitDetail> for CommitMetadata {
    fn from(detail: &CommitDetail) -> Self {
        Self {
            author_email: detail.author_email.clone(),
            committer: detail.committer.clone(),
            committer_email: detail.committer_email.clone(),
            committed_at: detail.committed_at.clone(),
            message: detail.message.clone(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileNode {
    pub id: FileId,
    pub summary: ChangedFile,
    pub diff: Resource<FileDiff>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommitNode {
    pub id: CommitId,
    pub summary: Commit,
    pub metadata: Resource<CommitMetadata>,
    pub file_order: Vec<FileId>,
    pub files: HashMap<FileId, FileNode>,
}

impl CommitNode {
    fn summary(id: CommitId, summary: Commit) -> Self {
        Self {
            id,
            summary,
            metadata: Resource::NotLoaded,
            file_order: Vec::new(),
            files: HashMap::new(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BranchNode {
    pub id: BranchId,
    /// `None` is a virtual revision such as detached `HEAD`; normal local and
    /// remote branches always carry their parsed summary.
    pub summary: Option<Branch>,
    pub commits: Resource<Vec<CommitId>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryNode {
    pub id: RepositoryId,
    pub requested_path: PathBuf,
    pub summary: Option<Repository>,
    pub branch_order: Vec<BranchId>,
    pub branches: HashMap<BranchId, BranchNode>,
    /// Commits are normalized at repository scope because one commit can be
    /// reachable from multiple branches.
    pub commits: HashMap<CommitId, CommitNode>,
    pub working_tree: Resource<Vec<WorkingTreeChange>>,
    pub reflog: Resource<Vec<ReflogEntry>>,
    pub remotes: Resource<Vec<RemoteInfo>>,
}

impl RepositoryNode {
    fn new(id: RepositoryId, requested_path: PathBuf) -> Self {
        Self {
            id,
            requested_path,
            summary: None,
            branch_order: Vec::new(),
            branches: HashMap::new(),
            commits: HashMap::new(),
            working_tree: Resource::NotLoaded,
            reflog: Resource::NotLoaded,
            remotes: Resource::NotLoaded,
        }
    }

    pub fn branch(&self, name: &BranchName) -> Option<&BranchNode> {
        self.branches.get(&BranchId {
            repository: self.id,
            name: name.clone(),
        })
    }

    pub fn commit(&self, hash: &CommitHash) -> Option<&CommitNode> {
        self.commits.get(&CommitId {
            repository: self.id,
            hash: hash.clone(),
        })
    }

    pub fn git_cwd(&self) -> &Path {
        self.summary
            .as_ref()
            .map_or(self.requested_path.as_path(), |repository| {
                repository.root.as_path()
            })
    }

    pub fn display_name(&self) -> String {
        self.summary.as_ref().map_or_else(
            || {
                self.requested_path
                    .file_name()
                    .filter(|name| !name.is_empty())
                    .map_or_else(
                        || self.requested_path.display().to_string(),
                        |name| name.to_string_lossy().into_owned(),
                    )
            },
            |repository| repository.name.clone(),
        )
    }

    pub fn display_path(&self) -> &Path {
        self.summary
            .as_ref()
            .map_or(self.requested_path.as_path(), |repository| {
                repository.root.as_path()
            })
    }

    pub fn branch_at(&self, index: usize) -> Option<&Branch> {
        self.branches
            .get(self.branch_order.get(index)?)?
            .summary
            .as_ref()
    }

    pub fn branch_count(&self) -> usize {
        self.branch_order.len()
    }
}

/// Authoritative normalized Git data store. UI cursor, focus, expansion and
/// pending worker bookkeeping deliberately do not live here.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GitModel {
    repository_order: Vec<RepositoryId>,
    repositories: HashMap<RepositoryId, RepositoryNode>,
}

impl GitModel {
    pub fn from_paths(paths: impl IntoIterator<Item = PathBuf>) -> Self {
        let mut model = Self::default();
        for path in paths {
            let id = RepositoryId(model.repository_order.len());
            model.repository_order.push(id);
            model.repositories.insert(id, RepositoryNode::new(id, path));
        }
        model
    }

    pub fn repository_ids(&self) -> &[RepositoryId] {
        &self.repository_order
    }

    pub fn repository(&self, id: RepositoryId) -> Option<&RepositoryNode> {
        self.repositories.get(&id)
    }

    pub fn repository_mut(&mut self, id: RepositoryId) -> Option<&mut RepositoryNode> {
        self.repositories.get_mut(&id)
    }

    pub fn set_repository_summary(&mut self, id: RepositoryId, summary: Repository) {
        if let Some(repository) = self.repository_mut(id) {
            repository.summary = Some(summary);
        }
        self.ensure_current_branch_visible(id);
    }

    pub fn replace_branches(&mut self, id: RepositoryId, branches: Vec<Branch>) {
        let Some(repository) = self.repository_mut(id) else {
            return;
        };
        let mut previous = std::mem::take(&mut repository.branches);
        repository.branch_order.clear();
        for branch in branches {
            let branch_id = BranchId {
                repository: id,
                name: branch.name.clone(),
            };
            let commits = previous
                .remove(&branch_id)
                .map_or(Resource::NotLoaded, |node| node.commits);
            repository.branch_order.push(branch_id.clone());
            repository.branches.insert(
                branch_id.clone(),
                BranchNode {
                    id: branch_id,
                    summary: Some(branch),
                    commits,
                },
            );
        }
        self.ensure_current_branch_visible(id);
    }

    /// Keep an unborn current branch in the authoritative model even though
    /// `for-each-ref` cannot return a ref until its first commit is created.
    fn ensure_current_branch_visible(&mut self, id: RepositoryId) {
        let Some(repository) = self.repository_mut(id) else {
            return;
        };
        let Some(summary) = repository.summary.as_ref() else {
            return;
        };
        let head = summary.head.clone();
        let Some(current_name) = summary.current_branch.clone() else {
            for branch in repository.branches.values_mut() {
                if let Some(summary) = branch.summary.as_mut() {
                    summary.is_current = false;
                }
            }
            return;
        };

        let current_id = BranchId {
            repository: id,
            name: current_name.clone(),
        };
        if repository.branches.contains_key(&current_id) {
            for branch in repository.branches.values_mut() {
                if let Some(summary) = branch.summary.as_mut() {
                    summary.is_current = summary.name == current_name;
                }
            }
            if let Some(branch) = repository
                .branches
                .get_mut(&current_id)
                .and_then(|branch| branch.summary.as_mut())
                && branch.head.0.is_empty()
                && !head.0.is_empty()
            {
                branch.head = head.clone();
                branch.short_head = head.short().to_string();
            }
            return;
        }

        let short_head = if head.0.is_empty() {
            "unborn".to_string()
        } else {
            head.short().to_string()
        };
        repository.branch_order.insert(0, current_id.clone());
        repository.branches.insert(
            current_id.clone(),
            BranchNode {
                id: current_id,
                summary: Some(Branch {
                    name: current_name.clone(),
                    full_ref: format!("refs/heads/{}", current_name.0),
                    kind: BranchKind::Local,
                    head: head.clone(),
                    short_head,
                    commit_date: String::new(),
                    subject: if head.0.is_empty() {
                        "Unborn branch (no commits yet)".into()
                    } else {
                        String::new()
                    },
                    is_current: true,
                }),
                commits: Resource::NotLoaded,
            },
        );
    }

    pub fn mark_branch_commits_loading(&mut self, id: &BranchId) {
        let Some(repository) = self.repository_mut(id.repository) else {
            return;
        };
        let branch = repository
            .branches
            .entry(id.clone())
            .or_insert_with(|| BranchNode {
                id: id.clone(),
                summary: None,
                commits: Resource::NotLoaded,
            });
        branch.commits = Resource::Loading;
    }

    pub fn replace_branch_commits(&mut self, id: &BranchId, commits: Vec<Commit>) {
        let Some(repository) = self.repository_mut(id.repository) else {
            return;
        };
        let mut order = Vec::with_capacity(commits.len());
        for commit in commits {
            let commit_id = CommitId {
                repository: id.repository,
                hash: commit.hash.clone(),
            };
            order.push(commit_id.clone());
            repository
                .commits
                .entry(commit_id.clone())
                .and_modify(|node| node.summary = commit.clone())
                .or_insert_with(|| CommitNode::summary(commit_id, commit));
        }
        repository
            .branches
            .entry(id.clone())
            .or_insert_with(|| BranchNode {
                id: id.clone(),
                summary: None,
                commits: Resource::NotLoaded,
            })
            .commits = Resource::Ready(order);
    }

    pub fn mark_commit_loading(&mut self, id: &CommitId) {
        if let Some(commit) = self
            .repository_mut(id.repository)
            .and_then(|repository| repository.commits.get_mut(id))
        {
            commit.metadata = Resource::Loading;
        }
    }

    pub fn set_commit_detail(&mut self, id: RepositoryId, detail: CommitDetail) {
        let Some(repository) = self.repository_mut(id) else {
            return;
        };
        let commit_id = CommitId {
            repository: id,
            hash: detail.commit.hash.clone(),
        };
        let node = repository
            .commits
            .entry(commit_id.clone())
            .or_insert_with(|| CommitNode::summary(commit_id.clone(), detail.commit.clone()));
        node.summary = detail.commit.clone();
        node.metadata = Resource::Ready(CommitMetadata::from(&detail));

        let mut previous = std::mem::take(&mut node.files);
        node.file_order.clear();
        for file in detail.files {
            let file_id = FileId {
                commit: commit_id.clone(),
                path: file.path.clone(),
            };
            let diff = previous
                .remove(&file_id)
                .map_or(Resource::NotLoaded, |node| node.diff);
            node.file_order.push(file_id.clone());
            node.files.insert(
                file_id.clone(),
                FileNode {
                    id: file_id,
                    summary: file,
                    diff,
                },
            );
        }
    }

    pub fn mark_file_diff_loading(&mut self, id: &FileId) {
        if let Some(file) = self.file_mut(id) {
            file.diff = Resource::Loading;
        }
    }

    pub fn set_file_diff(&mut self, id: RepositoryId, diff: FileDiff) {
        let file_id = FileId {
            commit: CommitId {
                repository: id,
                hash: diff.commit.clone(),
            },
            path: diff.path.clone(),
        };
        if let Some(file) = self.file_mut(&file_id) {
            file.diff = Resource::Ready(diff);
        }
    }

    pub fn file(&self, id: &FileId) -> Option<&FileNode> {
        self.repository(id.commit.repository)?
            .commits
            .get(&id.commit)?
            .files
            .get(id)
    }

    pub fn commit(&self, id: &CommitId) -> Option<&CommitNode> {
        self.repository(id.repository)?.commits.get(id)
    }

    fn file_mut(&mut self, id: &FileId) -> Option<&mut FileNode> {
        self.repository_mut(id.commit.repository)?
            .commits
            .get_mut(&id.commit)?
            .files
            .get_mut(id)
    }

    pub fn set_working_tree(&mut self, id: RepositoryId, changes: Vec<WorkingTreeChange>) {
        if let Some(repository) = self.repository_mut(id) {
            repository.working_tree = Resource::Ready(changes);
        }
    }

    pub fn mark_working_tree_loading(&mut self, id: RepositoryId) {
        if let Some(repository) = self.repository_mut(id) {
            repository.working_tree = Resource::Loading;
        }
    }

    pub fn set_reflog(&mut self, id: RepositoryId, entries: Vec<ReflogEntry>) {
        if let Some(repository) = self.repository_mut(id) {
            repository.reflog = Resource::Ready(entries);
        }
    }

    pub fn mark_reflog_loading(&mut self, id: RepositoryId) {
        if let Some(repository) = self.repository_mut(id) {
            repository.reflog = Resource::Loading;
        }
    }

    pub fn set_remotes(&mut self, id: RepositoryId, remotes: Vec<RemoteInfo>) {
        if let Some(repository) = self.repository_mut(id) {
            repository.remotes = Resource::Ready(remotes);
        }
    }

    pub fn mark_remotes_loading(&mut self, id: RepositoryId) {
        if let Some(repository) = self.repository_mut(id) {
            repository.remotes = Resource::Loading;
        }
    }

    pub fn branch_commits(&self, id: &BranchId) -> Option<Vec<&CommitNode>> {
        let repository = self.repository(id.repository)?;
        let order = repository.branches.get(id)?.commits.ready()?;
        Some(
            order
                .iter()
                .filter_map(|commit| repository.commits.get(commit))
                .collect(),
        )
    }

    pub fn branch_commits_resource(&self, id: &BranchId) -> Option<&Resource<Vec<CommitId>>> {
        Some(&self.repository(id.repository)?.branches.get(id)?.commits)
    }

    pub fn commit_metadata_resource(&self, id: &CommitId) -> Option<&Resource<CommitMetadata>> {
        Some(&self.repository(id.repository)?.commits.get(id)?.metadata)
    }

    pub fn file_diff_resource(&self, id: &FileId) -> Option<&Resource<FileDiff>> {
        Some(&self.file(id)?.diff)
    }

    pub fn repository_summary(&self, id: RepositoryId) -> Option<&Repository> {
        self.repository(id)?.summary.as_ref()
    }

    pub fn branch_summaries(&self, id: RepositoryId) -> Vec<&Branch> {
        let Some(repository) = self.repository(id) else {
            return Vec::new();
        };
        repository
            .branch_order
            .iter()
            .filter_map(|branch| repository.branches.get(branch)?.summary.as_ref())
            .collect()
    }

    pub fn branch_commit_list(&self, id: &BranchId) -> Option<CommitList> {
        Some(CommitList {
            viewing_branch: Some(id.name.clone()),
            items: self
                .branch_commits(id)?
                .into_iter()
                .map(|commit| commit.summary.clone())
                .collect(),
        })
    }

    pub fn commit_detail(&self, id: &CommitId) -> Option<CommitDetail> {
        let repository = self.repository(id.repository)?;
        let commit = repository.commits.get(id)?;
        let metadata = commit.metadata.ready()?;
        Some(CommitDetail {
            commit: commit.summary.clone(),
            author_email: metadata.author_email.clone(),
            committer: metadata.committer.clone(),
            committer_email: metadata.committer_email.clone(),
            committed_at: metadata.committed_at.clone(),
            message: metadata.message.clone(),
            files: commit
                .file_order
                .iter()
                .filter_map(|file| commit.files.get(file).map(|file| file.summary.clone()))
                .collect(),
        })
    }

    pub fn file_diff(&self, id: &FileId) -> Option<&FileDiff> {
        self.file(id)?.diff.ready()
    }

    pub fn working_tree(&self, id: RepositoryId) -> Option<&[WorkingTreeChange]> {
        self.repository(id)?.working_tree.ready().map(Vec::as_slice)
    }

    pub fn reflog(&self, id: RepositoryId) -> Option<&[ReflogEntry]> {
        self.repository(id)?.reflog.ready().map(Vec::as_slice)
    }

    pub fn remotes(&self, id: RepositoryId) -> Option<&[RemoteInfo]> {
        self.repository(id)?.remotes.ready().map(Vec::as_slice)
    }

    pub fn mark_branch_commits_failed(&mut self, id: &BranchId, error: String) {
        if let Some(branch) = self
            .repository_mut(id.repository)
            .and_then(|repository| repository.branches.get_mut(id))
        {
            branch.commits = Resource::Failed(error);
        }
    }

    pub fn reset_branch_commits_loading(&mut self, id: &BranchId) {
        if let Some(branch) = self
            .repository_mut(id.repository)
            .and_then(|repository| repository.branches.get_mut(id))
        {
            branch.commits.reset_loading();
        }
    }

    pub fn reset_commit_loading(&mut self, id: &CommitId) {
        if let Some(commit) = self
            .repository_mut(id.repository)
            .and_then(|repository| repository.commits.get_mut(id))
        {
            commit.metadata.reset_loading();
        }
    }

    pub fn reset_file_diff_loading(&mut self, id: &FileId) {
        if let Some(file) = self.file_mut(id) {
            file.diff.reset_loading();
        }
    }

    pub fn reset_working_tree_loading(&mut self, id: RepositoryId) {
        if let Some(repository) = self.repository_mut(id) {
            repository.working_tree.reset_loading();
        }
    }

    pub fn reset_reflog_loading(&mut self, id: RepositoryId) {
        if let Some(repository) = self.repository_mut(id) {
            repository.reflog.reset_loading();
        }
    }

    pub fn reset_remotes_loading(&mut self, id: RepositoryId) {
        if let Some(repository) = self.repository_mut(id) {
            repository.remotes.reset_loading();
        }
    }

    pub fn mark_commit_failed(&mut self, id: &CommitId, error: String) {
        if let Some(commit) = self
            .repository_mut(id.repository)
            .and_then(|repository| repository.commits.get_mut(id))
        {
            commit.metadata = Resource::Failed(error);
        }
    }

    pub fn mark_file_diff_failed(&mut self, id: &FileId, error: String) {
        if let Some(file) = self.file_mut(id) {
            file.diff = Resource::Failed(error);
        }
    }

    pub fn mark_working_tree_failed(&mut self, id: RepositoryId, error: String) {
        if let Some(repository) = self.repository_mut(id) {
            repository.working_tree = Resource::Failed(error);
        }
    }

    pub fn mark_reflog_failed(&mut self, id: RepositoryId, error: String) {
        if let Some(repository) = self.repository_mut(id) {
            repository.reflog = Resource::Failed(error);
        }
    }

    pub fn mark_remotes_failed(&mut self, id: RepositoryId, error: String) {
        if let Some(repository) = self.repository_mut(id) {
            repository.remotes = Resource::Failed(error);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{BranchKind, FileChangeKind};

    fn branch(name: &str) -> Branch {
        Branch {
            name: BranchName(name.into()),
            full_ref: format!("refs/heads/{name}"),
            kind: BranchKind::Local,
            head: CommitHash("aaaaaaaa".into()),
            short_head: "aaaaaaaa".into(),
            commit_date: String::new(),
            subject: String::new(),
            is_current: name == "main",
        }
    }

    fn commit(hash: &str, subject: &str) -> Commit {
        Commit {
            hash: CommitHash(hash.into()),
            short_hash: hash.into(),
            author: "Ada".into(),
            authored_at: "2026-07-17".into(),
            decorations: String::new(),
            subject: subject.into(),
        }
    }

    #[test]
    fn normalizes_shared_commits_and_preserves_loaded_children() {
        let mut model = GitModel::from_paths([PathBuf::from("/repo")]);
        let repository = RepositoryId(0);
        model.replace_branches(repository, vec![branch("main"), branch("feature")]);
        let main = BranchId {
            repository,
            name: BranchName("main".into()),
        };
        let feature = BranchId {
            repository,
            name: BranchName("feature".into()),
        };
        let shared = commit("aaaaaaaa", "base");
        model.replace_branch_commits(&main, vec![shared.clone()]);
        model.replace_branch_commits(&feature, vec![commit("bbbbbbbb", "tip"), shared]);

        assert_eq!(model.repository(repository).unwrap().commits.len(), 2);
        assert_eq!(model.branch_commits(&main).unwrap().len(), 1);
        assert_eq!(model.branch_commits(&feature).unwrap().len(), 2);

        let detail = CommitDetail {
            commit: commit("aaaaaaaa", "base"),
            author_email: "ada@example.invalid".into(),
            committer: "Ada".into(),
            committer_email: "ada@example.invalid".into(),
            committed_at: "2026-07-17".into(),
            message: "base\n\nbody".into(),
            files: vec![ChangedFile {
                kind: FileChangeKind::Modified,
                path: GitPath::from("src/main.rs"),
                old_path: None,
                additions: Some(1),
                deletions: Some(1),
                hunks: Vec::new(),
                is_binary: false,
            }],
        };
        model.set_commit_detail(repository, detail);
        let file = FileId {
            commit: CommitId {
                repository,
                hash: CommitHash("aaaaaaaa".into()),
            },
            path: GitPath::from("src/main.rs"),
        };
        assert!(model.file(&file).is_some());

        // Reloading branch summaries must not discard already loaded commit
        // and file resources for a branch that still exists.
        model.replace_branches(repository, vec![branch("main"), branch("feature")]);
        assert_eq!(model.branch_commits(&main).unwrap().len(), 1);
        assert!(model.file(&file).is_some());
    }
}
