use std::collections::HashMap;

use crate::domain::{BranchName, CommitHash, GitPath};

use super::{
    AppState, BranchId, BranchTreeNode, ChangeGroup, ChangesTreeNode, CommitId, FileId,
    RepositoryId,
};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub enum FocusKind {
    Repository,
    Branch,
    Commit,
    File,
    Diff,
    Reflog,
    Changes,
    ChangesDiff,
    Remote,
}

/// Position of a semantic entity in the drill-down flow. A collection role
/// keeps the current parent/child view; Entity advances that child to the left
/// side of the next projection; Content focuses its rendered contents.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum FocusRole {
    Collection,
    Entity,
    Content,
}

impl FocusKind {
    pub const ALL: &'static [Self] = &[
        Self::Repository,
        Self::Branch,
        Self::Commit,
        Self::File,
        Self::Diff,
        Self::Reflog,
        Self::Changes,
        Self::ChangesDiff,
        Self::Remote,
    ];

    pub const ALL_MASK: u16 = (1 << Self::ALL.len()) - 1;

    pub const fn mask(self) -> u16 {
        1 << self as u8
    }

    pub const fn id(self) -> &'static str {
        match self {
            Self::Repository => "repository",
            Self::Branch => "branch",
            Self::Commit => "commit",
            Self::File => "file",
            Self::Diff => "diff",
            Self::Reflog => "reflog.entry",
            Self::Changes => "working-tree.change",
            Self::ChangesDiff => "working-tree.diff",
            Self::Remote => "remote",
        }
    }

    pub const fn title(self) -> &'static str {
        match self {
            Self::Repository => "Repository",
            Self::Branch => "Branch",
            Self::Commit => "Commit",
            Self::File => "File",
            Self::Diff => "File diff",
            Self::Reflog => "Reflog entry",
            Self::Changes => "Working-tree change",
            Self::ChangesDiff => "Working-tree diff",
            Self::Remote => "Remote",
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ChangeTarget {
    Root,
    Group(ChangeGroup),
    File { group: ChangeGroup, path: GitPath },
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum FocusTarget {
    Repository(RepositoryId),
    Branch(BranchId),
    Commit(CommitId),
    File(FileId),
    Diff(FileId),
    ReflogEntry {
        repository: RepositoryId,
        selector: String,
        commit: CommitHash,
    },
    Changes {
        repository: RepositoryId,
        target: ChangeTarget,
    },
    ChangesDiff {
        repository: RepositoryId,
        group: ChangeGroup,
        path: GitPath,
    },
    Remote {
        repository: RepositoryId,
        name: Option<String>,
    },
}

impl FocusTarget {
    pub const fn kind(&self) -> FocusKind {
        match self {
            Self::Repository(_) => FocusKind::Repository,
            Self::Branch(_) => FocusKind::Branch,
            Self::Commit(_) => FocusKind::Commit,
            Self::File(_) => FocusKind::File,
            Self::Diff(_) => FocusKind::Diff,
            Self::ReflogEntry { .. } => FocusKind::Reflog,
            Self::Changes { .. } => FocusKind::Changes,
            Self::ChangesDiff { .. } => FocusKind::ChangesDiff,
            Self::Remote { .. } => FocusKind::Remote,
        }
    }

    pub const fn repository(&self) -> RepositoryId {
        match self {
            Self::Repository(repository)
            | Self::ReflogEntry { repository, .. }
            | Self::Changes { repository, .. }
            | Self::ChangesDiff { repository, .. }
            | Self::Remote { repository, .. } => *repository,
            Self::Branch(branch) => branch.repository,
            Self::Commit(commit) => commit.repository,
            Self::File(file) | Self::Diff(file) => file.commit.repository,
        }
    }
}

/// Semantic navigation identity. The target carries typed ancestry; optional
/// ancestor fields make projection code cheap and are validated on creation.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct FocusPath {
    pub target: FocusTarget,
    pub role: FocusRole,
    pub repository: RepositoryId,
    pub branch: Option<BranchId>,
    pub commit: Option<CommitId>,
    pub file: Option<FileId>,
}

/// Runtime focus for both concrete entities and empty collections. `path`
/// is present whenever the focused entity exists; `kind + role` remain valid
/// while a collection is empty/loading, so view and operation resolution do
/// not need a parallel page-state machine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FocusContext {
    pub kind: FocusKind,
    pub role: FocusRole,
    pub path: Option<FocusPath>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum CollectionId {
    RepositoryTree,
    Commits(BranchId),
    Files(CommitId),
    Reflog(RepositoryId),
    Changes(RepositoryId),
    Remotes(RepositoryId),
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum EntityId {
    Repository(RepositoryId),
    Branch(BranchId),
    Commit(CommitId),
    File(FileId),
    Reflog {
        repository: RepositoryId,
        selector: String,
    },
    Change {
        repository: RepositoryId,
        target: ChangeTarget,
    },
    Remote {
        repository: RepositoryId,
        name: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NavigationState {
    pub current: FocusContext,
    /// Stable IDs, never filtered-list offsets. SelectionState contains the
    /// presentation cursor offsets; this map preserves model identity across
    /// filtering and view projection changes.
    pub cursors: HashMap<CollectionId, EntityId>,
    pub history: Vec<FocusContext>,
}

impl Default for NavigationState {
    fn default() -> Self {
        Self {
            current: FocusContext {
                kind: FocusKind::Repository,
                role: FocusRole::Entity,
                path: None,
            },
            cursors: HashMap::new(),
            history: Vec::new(),
        }
    }
}

impl FocusPath {
    pub fn new(
        target: FocusTarget,
        role: FocusRole,
        branch: Option<BranchId>,
        commit: Option<CommitId>,
        file: Option<FileId>,
    ) -> Option<Self> {
        let repository = target.repository();
        if branch
            .as_ref()
            .is_some_and(|branch| branch.repository != repository)
            || commit
                .as_ref()
                .is_some_and(|commit| commit.repository != repository)
            || file
                .as_ref()
                .is_some_and(|file| file.commit.repository != repository)
        {
            return None;
        }
        if let (Some(commit), Some(file)) = (&commit, &file)
            && file.commit != *commit
        {
            return None;
        }

        // A FocusPath is an identity path, not a bag of convenient context.
        // Descendant fields are therefore forbidden on shallower targets and
        // the target entity must equal the matching ancestry field.
        let valid_shape = match &target {
            FocusTarget::Repository(_) => branch.is_none() && commit.is_none() && file.is_none(),
            FocusTarget::Branch(target_branch) => {
                branch.as_ref() == Some(target_branch) && commit.is_none() && file.is_none()
            }
            FocusTarget::Commit(target_commit) => {
                branch.is_some() && commit.as_ref() == Some(target_commit) && file.is_none()
            }
            FocusTarget::File(target_file) | FocusTarget::Diff(target_file) => {
                branch.is_some()
                    && commit.as_ref() == Some(&target_file.commit)
                    && file.as_ref() == Some(target_file)
            }
            FocusTarget::ReflogEntry { .. }
            | FocusTarget::Changes { .. }
            | FocusTarget::ChangesDiff { .. }
            | FocusTarget::Remote { .. } => branch.is_none() && commit.is_none() && file.is_none(),
        };
        if !valid_shape {
            return None;
        }
        Some(Self {
            target,
            role,
            repository,
            branch,
            commit,
            file,
        })
    }

    pub const fn kind(&self) -> FocusKind {
        self.target.kind()
    }
}

impl AppState {
    pub fn semantic_focus(&self) -> Option<FocusPath> {
        self.navigation.current.path.clone()
    }

    pub fn focus_context(&self) -> FocusContext {
        self.navigation.current.clone()
    }

    fn derive_semantic_focus(&self, kind: FocusKind, role: FocusRole) -> Option<FocusPath> {
        let active_repository = RepositoryId(
            self.active_repository_index
                .or_else(|| {
                    self.viewing_branch
                        .as_ref()
                        .map(|branch| branch.repository.0)
                })
                .or(self.reflog_repository_index)
                .or(self.changes_repository_index)
                .or(self.remotes_repository_index)?,
        );
        let branch_id = self.viewing_branch.clone();
        let commit_id = self.selected_commit().map(|commit| CommitId {
            repository: self
                .viewing_branch
                .as_ref()
                .map_or(active_repository, |branch| branch.repository),
            hash: commit.hash.clone(),
        });
        let file_id = self.selected_file().and_then(|file| {
            Some(FileId {
                commit: commit_id.clone()?,
                path: file.path.clone(),
            })
        });

        let target = match kind {
            FocusKind::Repository | FocusKind::Branch => match self.selected_tree_node()? {
                BranchTreeNode::Repository { repository_index } => {
                    FocusTarget::Repository(RepositoryId(repository_index))
                }
                BranchTreeNode::Branch {
                    repository_index,
                    branch_index,
                } => FocusTarget::Branch(BranchId {
                    repository: RepositoryId(repository_index),
                    name: self
                        .repository_branch(repository_index, branch_index)?
                        .name
                        .clone(),
                }),
            },
            FocusKind::Commit => FocusTarget::Commit(commit_id.clone()?),
            FocusKind::File => FocusTarget::File(file_id.clone()?),
            FocusKind::Diff => FocusTarget::Diff(file_id.clone()?),
            FocusKind::Reflog => {
                let entry = self.selected_reflog()?;
                FocusTarget::ReflogEntry {
                    repository: RepositoryId(
                        self.reflog_repository_index.unwrap_or(active_repository.0),
                    ),
                    selector: entry.selector.clone(),
                    commit: entry.hash.clone(),
                }
            }
            FocusKind::Changes => {
                let repository =
                    RepositoryId(self.changes_repository_index.unwrap_or(active_repository.0));
                let target = match self.selected_changes_node()? {
                    ChangesTreeNode::Root => ChangeTarget::Root,
                    ChangesTreeNode::Group(group) => ChangeTarget::Group(group),
                    ChangesTreeNode::File {
                        group,
                        change_index,
                    } => ChangeTarget::File {
                        group,
                        path: self.working_tree_changes().get(change_index)?.path.clone(),
                    },
                };
                FocusTarget::Changes { repository, target }
            }
            FocusKind::ChangesDiff => {
                let (group, change) = self.selected_change()?;
                FocusTarget::ChangesDiff {
                    repository: RepositoryId(
                        self.changes_repository_index.unwrap_or(active_repository.0),
                    ),
                    group,
                    path: change.path.clone(),
                }
            }
            FocusKind::Remote => FocusTarget::Remote {
                repository: RepositoryId(
                    self.remotes_repository_index.unwrap_or(active_repository.0),
                ),
                name: self.selected_remote().map(|remote| remote.name.clone()),
            },
        };

        let (branch, commit, file) = match &target {
            FocusTarget::Repository(_)
            | FocusTarget::ReflogEntry { .. }
            | FocusTarget::Changes { .. }
            | FocusTarget::ChangesDiff { .. }
            | FocusTarget::Remote { .. } => (None, None, None),
            FocusTarget::Branch(branch) => (Some(branch.clone()), None, None),
            FocusTarget::Commit(commit) => (branch_id, Some(commit.clone()), None),
            FocusTarget::File(file) | FocusTarget::Diff(file) => {
                (branch_id, Some(file.commit.clone()), Some(file.clone()))
            }
        };

        FocusPath::new(target, role, branch, commit, file)
    }

    pub fn semantic_focus_kind(&self) -> Option<FocusKind> {
        Some(self.focus_context().kind)
    }

    pub fn focused_branch_name(&self) -> Option<BranchName> {
        self.semantic_focus()?.branch.map(|branch| branch.name)
    }

    pub fn reconcile_focus(&mut self) {
        let current = self.navigation.current.clone();
        let next = self.derive_focus_context(current.kind, current.role);
        if self.navigation.current != next {
            if self.navigation.current.kind != next.kind
                || self.navigation.current.role != next.role
            {
                self.navigation
                    .history
                    .push(self.navigation.current.clone());
            }
            self.navigation.current = next;
        }
        if let Some((collection, entity)) = focus_cursor(&self.navigation.current) {
            self.navigation.cursors.insert(collection, entity);
        }
    }

    /// The sole navigation entry point. Views, operation mounts and data
    /// requirements all project from this semantic layer/role pair.
    pub fn set_focus_layer(&mut self, kind: FocusKind, role: FocusRole) {
        let previous = self.navigation.current.clone();
        let next = self.derive_focus_context(kind, role);
        if previous.kind != next.kind || previous.role != next.role {
            self.navigation.history.push(previous);
        }
        self.navigation.current = next;
        if let Some((collection, entity)) = focus_cursor(&self.navigation.current) {
            self.navigation.cursors.insert(collection, entity);
        }
    }

    pub fn restore_focus_context(&mut self, context: FocusContext) {
        self.set_focus_layer(context.kind, context.role);
    }

    fn derive_focus_context(&self, kind: FocusKind, role: FocusRole) -> FocusContext {
        let path = self.derive_semantic_focus(kind, role);
        FocusContext {
            // The repository tree is one collection whose selected entity may
            // be either a repository or a branch. Every other empty/loading
            // collection retains its declared semantic kind.
            kind: if matches!(kind, FocusKind::Repository | FocusKind::Branch) {
                path.as_ref().map_or(FocusKind::Repository, FocusPath::kind)
            } else {
                kind
            },
            role,
            path,
        }
    }
}

fn focus_cursor(focus: &FocusContext) -> Option<(CollectionId, EntityId)> {
    let path = focus.path.as_ref()?;
    match &path.target {
        FocusTarget::Repository(repository) => Some((
            CollectionId::RepositoryTree,
            EntityId::Repository(*repository),
        )),
        FocusTarget::Branch(branch) => Some((
            CollectionId::RepositoryTree,
            EntityId::Branch(branch.clone()),
        )),
        FocusTarget::Commit(commit) => Some((
            CollectionId::Commits(path.branch.clone()?),
            EntityId::Commit(commit.clone()),
        )),
        FocusTarget::File(file) | FocusTarget::Diff(file) => Some((
            CollectionId::Files(file.commit.clone()),
            EntityId::File(file.clone()),
        )),
        FocusTarget::ReflogEntry {
            repository,
            selector,
            ..
        } => Some((
            CollectionId::Reflog(*repository),
            EntityId::Reflog {
                repository: *repository,
                selector: selector.clone(),
            },
        )),
        FocusTarget::Changes { repository, target } => Some((
            CollectionId::Changes(*repository),
            EntityId::Change {
                repository: *repository,
                target: target.clone(),
            },
        )),
        FocusTarget::ChangesDiff {
            repository,
            group,
            path,
        } => Some((
            CollectionId::Changes(*repository),
            EntityId::Change {
                repository: *repository,
                target: ChangeTarget::File {
                    group: *group,
                    path: path.clone(),
                },
            },
        )),
        FocusTarget::Remote {
            repository,
            name: Some(name),
        } => Some((
            CollectionId::Remotes(*repository),
            EntityId::Remote {
                repository: *repository,
                name: name.clone(),
            },
        )),
        FocusTarget::Remote { name: None, .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_inconsistent_ancestor_paths() {
        let repository = RepositoryId(0);
        let other = RepositoryId(1);
        assert!(
            FocusPath::new(
                FocusTarget::Repository(repository),
                FocusRole::Entity,
                Some(BranchId {
                    repository: other,
                    name: BranchName("main".into()),
                }),
                None,
                None,
            )
            .is_none()
        );

        let branch = BranchId {
            repository,
            name: BranchName("main".into()),
        };
        let target = CommitId {
            repository,
            hash: CommitHash("a".repeat(40)),
        };
        let unrelated = CommitId {
            repository,
            hash: CommitHash("b".repeat(40)),
        };
        assert!(
            FocusPath::new(
                FocusTarget::Commit(target),
                FocusRole::Entity,
                Some(branch),
                Some(unrelated),
                None,
            )
            .is_none(),
            "the ancestry commit must be the target commit"
        );
        assert!(
            FocusPath::new(
                FocusTarget::Changes {
                    repository,
                    target: ChangeTarget::Root,
                },
                FocusRole::Entity,
                None,
                Some(CommitId {
                    repository,
                    hash: CommitHash("c".repeat(40)),
                }),
                None,
            )
            .is_none(),
            "repository facets cannot inherit an unrelated commit path"
        );
    }

    #[test]
    fn cursor_identity_is_typed_and_independent_from_list_offsets() {
        let repository = RepositoryId(0);
        let branch = BranchId {
            repository,
            name: BranchName("main".into()),
        };
        let commit = CommitId {
            repository,
            hash: CommitHash("a".repeat(40)),
        };
        let path = FocusPath::new(
            FocusTarget::Commit(commit.clone()),
            FocusRole::Collection,
            Some(branch.clone()),
            Some(commit.clone()),
            None,
        )
        .unwrap();
        assert_eq!(
            focus_cursor(&FocusContext {
                kind: FocusKind::Commit,
                role: FocusRole::Collection,
                path: Some(path),
            }),
            Some((CollectionId::Commits(branch), EntityId::Commit(commit)))
        );
    }
}
