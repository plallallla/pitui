use super::{AppState, FocusKind, FocusRole};

/// Stable identifiers for the model-driven views.  A view is a projection of
/// semantic focus, not a navigation state of its own.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ViewId {
    #[default]
    History,
    Commit,
    FileDiff,
    Reflog,
    Changes,
    Remotes,
}

impl ViewId {
    pub const ALL: &'static [Self] = &[
        Self::History,
        Self::Commit,
        Self::FileDiff,
        Self::Reflog,
        Self::Changes,
        Self::Remotes,
    ];

    pub const fn id(self) -> &'static str {
        match self {
            Self::History => "history",
            Self::Commit => "commit",
            Self::FileDiff => "file-diff",
            Self::Reflog => "reflog",
            Self::Changes => "changes",
            Self::Remotes => "remotes",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|view| view.id() == value)
    }
}

/// Reusable panels are identified independently from the view in which they
/// appear.  This lets one commit/file component move from the right side to
/// the left side while preserving its model identity and cursor.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum PanelId {
    RepositoryBranches,
    Commits,
    Commit,
    FileDiff,
    Reflog,
    ReflogDetail,
    Changes,
    ChangesDiff,
    Remotes,
    RemoteDetail,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ViewProjection {
    pub view: ViewId,
    pub left: PanelId,
    pub right: PanelId,
    pub focused: PanelId,
}

impl ViewProjection {
    /// Project a two-column view from the semantic layer and its drill-down
    /// role. Collection means "the entity is still in its parent's right-hand
    /// collection"; Entity means it has advanced to the next view; Content is
    /// the entity's rendered detail.
    pub const fn from_focus(kind: FocusKind, role: FocusRole) -> Self {
        match (kind, role) {
            (FocusKind::Repository | FocusKind::Branch, _) => {
                Self::history(PanelId::RepositoryBranches)
            }
            (FocusKind::Commit, FocusRole::Collection) => Self::history(PanelId::Commits),
            (FocusKind::Commit, FocusRole::Entity | FocusRole::Content) => {
                Self::commit(PanelId::Commits)
            }
            (FocusKind::File, FocusRole::Collection) => Self::commit(PanelId::Commit),
            (FocusKind::File, FocusRole::Entity) => Self::file_diff(PanelId::Commit),
            (FocusKind::File, FocusRole::Content) | (FocusKind::Diff, _) => {
                Self::file_diff(PanelId::FileDiff)
            }
            (FocusKind::Reflog, _) => Self {
                view: ViewId::Reflog,
                left: PanelId::Reflog,
                right: PanelId::ReflogDetail,
                focused: PanelId::Reflog,
            },
            (FocusKind::Changes, _) => Self {
                view: ViewId::Changes,
                left: PanelId::Changes,
                right: PanelId::ChangesDiff,
                focused: PanelId::Changes,
            },
            (FocusKind::ChangesDiff, _) => Self {
                view: ViewId::Changes,
                left: PanelId::Changes,
                right: PanelId::ChangesDiff,
                focused: PanelId::ChangesDiff,
            },
            (FocusKind::Remote, _) => Self {
                view: ViewId::Remotes,
                left: PanelId::Remotes,
                right: PanelId::RemoteDetail,
                focused: PanelId::Remotes,
            },
        }
    }

    const fn history(focused: PanelId) -> Self {
        Self {
            view: ViewId::History,
            left: PanelId::RepositoryBranches,
            right: PanelId::Commits,
            focused,
        }
    }

    const fn commit(focused: PanelId) -> Self {
        Self {
            view: ViewId::Commit,
            left: PanelId::Commits,
            right: PanelId::Commit,
            focused,
        }
    }

    const fn file_diff(focused: PanelId) -> Self {
        Self {
            view: ViewId::FileDiff,
            left: PanelId::Commit,
            right: PanelId::FileDiff,
            focused,
        }
    }

    /// Empty/loading collections may not have a concrete `FocusTarget`, but
    /// their semantic `FocusKind + FocusRole` still fully determines the
    /// projection. Rendering never needs an independent page selector.
    pub fn from_state(state: &AppState) -> Self {
        let focus = state.focus_context();
        Self::from_focus(focus.kind, focus.role)
    }
}

impl AppState {
    pub fn view_projection(&self) -> ViewProjection {
        ViewProjection::from_state(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drill_down_moves_the_previous_right_panel_to_the_left() {
        let history = ViewProjection::from_focus(FocusKind::Commit, FocusRole::Collection);
        let commit = ViewProjection::from_focus(FocusKind::Commit, FocusRole::Entity);
        let file = ViewProjection::from_focus(FocusKind::File, FocusRole::Entity);

        assert_eq!(history.right, commit.left);
        assert_eq!(commit.right, file.left);
        assert_eq!(history.view, ViewId::History);
        assert_eq!(commit.view, ViewId::Commit);
        assert_eq!(file.view, ViewId::FileDiff);
    }

    #[test]
    fn projection_for_empty_semantic_collections_keeps_the_expected_view() {
        let mut state = AppState::default();
        state.set_focus_layer(FocusKind::File, FocusRole::Collection);
        assert_eq!(state.view_projection().view, ViewId::Commit);
    }
}
