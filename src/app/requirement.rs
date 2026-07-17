use super::{AppState, BranchId, CommitId, FileId, RepositoryId, Resource, ViewId};

/// Declarative data dependencies derived from the current model focus/view.
/// The controller translates these values to GitRequest effects; key handlers
/// no longer need to know which preview request a cursor move implies.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum DataRequirement {
    BranchCommits(BranchId),
    CommitDetail(CommitId),
    FileDiff(FileId),
    Reflog(RepositoryId),
    WorkingTree(RepositoryId),
    Remotes(RepositoryId),
}

impl AppState {
    pub fn missing_data_requirements(&self) -> Vec<DataRequirement> {
        let mut requirements = Vec::new();
        match self.view_projection().view {
            ViewId::History => {
                if let Some(branch) = self.selected_branch_id()
                    && self
                        .model
                        .repository(branch.repository)
                        .and_then(|repository| repository.branches.get(&branch))
                        .is_some_and(|branch| matches!(branch.commits, Resource::NotLoaded))
                {
                    requirements.push(DataRequirement::BranchCommits(branch));
                }
            }
            ViewId::Commit => {
                if let Some(commit) = self.selected_commit_id()
                    && self
                        .model
                        .repository(commit.repository)
                        .and_then(|repository| repository.commits.get(&commit))
                        .is_some_and(|commit| matches!(commit.metadata, Resource::NotLoaded))
                {
                    requirements.push(DataRequirement::CommitDetail(commit));
                }
            }
            ViewId::FileDiff => {
                if let Some(file) = self.selected_file_id()
                    && self
                        .model
                        .file(&file)
                        .is_some_and(|file| matches!(file.diff, Resource::NotLoaded))
                {
                    requirements.push(DataRequirement::FileDiff(file));
                }
            }
            ViewId::Reflog => {
                if let Some(repository) = self.reflog_repository_index.map(RepositoryId)
                    && self
                        .model
                        .repository(repository)
                        .is_some_and(|repository| matches!(repository.reflog, Resource::NotLoaded))
                {
                    requirements.push(DataRequirement::Reflog(repository));
                }
            }
            ViewId::Changes => {
                if let Some(repository) = self.changes_repository_index.map(RepositoryId)
                    && self.model.repository(repository).is_some_and(|repository| {
                        matches!(repository.working_tree, Resource::NotLoaded)
                    })
                {
                    requirements.push(DataRequirement::WorkingTree(repository));
                }
            }
            ViewId::Remotes => {
                if let Some(repository) = self.remotes_repository_index.map(RepositoryId)
                    && self
                        .model
                        .repository(repository)
                        .is_some_and(|repository| matches!(repository.remotes, Resource::NotLoaded))
                {
                    requirements.push(DataRequirement::Remotes(repository));
                }
            }
        }
        requirements
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::domain::{Branch, BranchKind, BranchName, CommitHash};

    use super::*;

    #[test]
    fn history_projection_declares_missing_commits_for_the_focused_branch() {
        let mut state = AppState::with_repository_paths(vec![PathBuf::from("/repo")]);
        state.model.replace_branches(
            RepositoryId(0),
            vec![Branch {
                name: BranchName("main".into()),
                full_ref: "refs/heads/main".into(),
                kind: BranchKind::Local,
                head: CommitHash("a".repeat(40)),
                short_head: "aaaaaaaa".into(),
                commit_date: String::new(),
                subject: String::new(),
                is_current: true,
            }],
        );
        state.selection.selected_branch_index = Some(1);

        assert_eq!(
            state.missing_data_requirements(),
            vec![DataRequirement::BranchCommits(BranchId {
                repository: RepositoryId(0),
                name: BranchName("main".into()),
            })]
        );
    }
}
