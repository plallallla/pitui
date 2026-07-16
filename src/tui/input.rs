use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::{
    app::{Action, AppState, GlobalMode, RemoteEditKind, RemoteInputField},
    config::{KeyResolution, KeyStroke},
};

pub fn map_key(app: &AppState, event: KeyEvent) -> Option<Action> {
    if !matches!(event.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
        return None;
    }
    if event.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(event.code, KeyCode::Char('c') | KeyCode::Char('C'))
        && !matches!(app.mode, GlobalMode::Normal | GlobalMode::Chord { .. })
    {
        return Some(Action::Quit);
    }

    match &app.mode {
        GlobalMode::Filtering { query, .. } => map_filtering(event, query),
        GlobalMode::Confirming {
            dialog: crate::app::ConfirmDialog::ResetModeChoice { .. },
        } => match event.code {
            KeyCode::Char('s') | KeyCode::Char('S') => Some(Action::ChooseResetSoft),
            KeyCode::Char('m') | KeyCode::Char('M') => Some(Action::ChooseResetMixed),
            KeyCode::Char('h') | KeyCode::Char('H') => Some(Action::ChooseResetHard),
            KeyCode::Esc | KeyCode::Char('q') => Some(Action::Cancel),
            _ => None,
        },
        GlobalMode::Confirming { .. } => match event.code {
            KeyCode::Enter => Some(Action::Confirm),
            KeyCode::Esc | KeyCode::Char('q') => Some(Action::Cancel),
            _ => None,
        },
        GlobalMode::TypingConfirmation { input, .. } => map_typed_confirmation(event, input),
        GlobalMode::EditingCommitMessage { input, .. } => map_commit_message(event, input),
        GlobalMode::EditingRemote {
            kind,
            field,
            name,
            url,
            ..
        } => map_remote_editor(event, kind, *field, name, url),
        GlobalMode::Chord { prefix, .. } => {
            if matches!(event.code, KeyCode::Esc) {
                return Some(Action::Cancel);
            }
            map_resolved_key(app, prefix, event).or(Some(Action::Cancel))
        }
        GlobalMode::Error => match event.code {
            KeyCode::Enter | KeyCode::Esc | KeyCode::Char('q') => Some(Action::DismissError),
            _ => None,
        },
        GlobalMode::Normal => map_resolved_key(app, &[], event),
    }
}

fn map_remote_editor(
    event: KeyEvent,
    kind: &RemoteEditKind,
    field: RemoteInputField,
    name: &str,
    url: &str,
) -> Option<Action> {
    match event.code {
        KeyCode::Char(character)
            if !event
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            match field {
                RemoteInputField::Name if matches!(kind, RemoteEditKind::Add) => {
                    let mut input = name.to_string();
                    input.push(character);
                    Some(Action::UpdateRemoteName(input))
                }
                RemoteInputField::Url => {
                    let mut input = url.to_string();
                    input.push(character);
                    Some(Action::UpdateRemoteUrl(input))
                }
                RemoteInputField::Name => None,
            }
        }
        KeyCode::Backspace => match field {
            RemoteInputField::Name if matches!(kind, RemoteEditKind::Add) => {
                let mut input = name.to_string();
                input.pop();
                Some(Action::UpdateRemoteName(input))
            }
            RemoteInputField::Url => {
                let mut input = url.to_string();
                input.pop();
                Some(Action::UpdateRemoteUrl(input))
            }
            RemoteInputField::Name => None,
        },
        KeyCode::Tab | KeyCode::BackTab if matches!(kind, RemoteEditKind::Add) => {
            Some(Action::FocusNextRemoteField)
        }
        KeyCode::Enter => Some(Action::SubmitRemoteEditor),
        KeyCode::Esc => Some(Action::Cancel),
        _ => None,
    }
}

fn map_commit_message(event: KeyEvent, input: &str) -> Option<Action> {
    match event.code {
        KeyCode::Char(character)
            if !event
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            let mut input = input.to_string();
            input.push(character);
            Some(Action::UpdateCommitMessage(input))
        }
        KeyCode::Backspace => {
            let mut input = input.to_string();
            input.pop();
            Some(Action::UpdateCommitMessage(input))
        }
        KeyCode::Enter => Some(Action::SubmitCommit),
        KeyCode::Esc => Some(Action::Cancel),
        _ => None,
    }
}

fn map_filtering(event: KeyEvent, query: &str) -> Option<Action> {
    match event.code {
        KeyCode::Char(character)
            if !event
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            let mut query = query.to_string();
            query.push(character);
            Some(Action::UpdateFilter(query))
        }
        KeyCode::Backspace => {
            let mut query = query.to_string();
            query.pop();
            Some(Action::UpdateFilter(query))
        }
        KeyCode::Enter => Some(Action::SubmitFilter),
        KeyCode::Esc => Some(Action::CancelFilter),
        _ => None,
    }
}

fn map_typed_confirmation(event: KeyEvent, input: &str) -> Option<Action> {
    match event.code {
        KeyCode::Char(character)
            if !event
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            let mut input = input.to_string();
            input.push(character);
            Some(Action::UpdateTypedConfirmation(input))
        }
        KeyCode::Backspace => {
            let mut input = input.to_string();
            input.pop();
            Some(Action::UpdateTypedConfirmation(input))
        }
        KeyCode::Enter => Some(Action::ConfirmReset),
        KeyCode::Esc => Some(Action::Cancel),
        _ => None,
    }
}

fn map_resolved_key(app: &AppState, prefix: &[KeyStroke], event: KeyEvent) -> Option<Action> {
    match app
        .config
        .keymap
        .resolve(app, prefix, KeyStroke::from_event(event))?
    {
        KeyResolution::Action(action) => Some(action),
        KeyResolution::Chord(prefix) => Some(Action::BeginChord(prefix)),
    }
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, time::Instant};

    use crossterm::event::KeyEvent;

    use super::*;
    use crate::{
        app::{ChangeGroup, ChangeSelection, FocusPanel, Screen},
        config::KeyStroke,
        domain::{GitPath, WorkingTreeChange},
    };

    #[test]
    fn maps_global_exit() {
        let state = AppState::default();
        assert_eq!(
            map_key(
                &state,
                KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)
            ),
            Some(Action::Quit)
        );
    }

    #[test]
    fn ctrl_r_refreshes_from_every_normal_screen_but_plain_r_is_unbound() {
        for screen in [
            Screen::BranchOverview,
            Screen::CommitDetail,
            Screen::FileDiffDetail,
            Screen::Reflog,
            Screen::Changes,
            Screen::Remotes,
        ] {
            let state = AppState {
                screen,
                ..AppState::with_repository_paths(vec![PathBuf::from("/repo")])
            };
            assert_eq!(
                map_key(
                    &state,
                    KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)
                ),
                Some(Action::RefreshRepository)
            );
            assert_eq!(
                map_key(
                    &state,
                    KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE)
                ),
                None
            );
        }
    }

    #[test]
    fn file_detail_navigation_is_not_exposed_without_actionable_content() {
        for (screen, focus) in [
            (Screen::CommitDetail, FocusPanel::CommitFileList),
            (Screen::FileDiffDetail, FocusPanel::FileList),
            (Screen::FileDiffDetail, FocusPanel::DiffView),
            (Screen::Changes, FocusPanel::ChangesDiff),
        ] {
            let state = AppState {
                screen,
                focus,
                ..AppState::default()
            };
            for (key, action) in [
                (KeyCode::Home, Action::Home),
                (KeyCode::End, Action::End),
                (KeyCode::PageUp, Action::PageUp),
                (KeyCode::PageDown, Action::PageDown),
            ] {
                assert_eq!(
                    map_key(&state, KeyEvent::new(key, KeyModifiers::NONE)),
                    None,
                    "inactive {key:?}/{action:?} should not map for empty {screen:?}/{focus:?}"
                );
            }
        }
    }

    #[test]
    fn confirmation_q_cancels_instead_of_quitting() {
        let state = AppState {
            mode: GlobalMode::Confirming {
                dialog: crate::app::ConfirmDialog::CherryPickQueue {
                    repository_index: 0,
                    commits: vec![],
                },
            },
            ..AppState::default()
        };
        assert_eq!(
            map_key(
                &state,
                KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)
            ),
            Some(Action::Cancel)
        );
    }

    #[test]
    fn ctrl_c_quits_outside_commit_context() {
        let state = AppState {
            mode: GlobalMode::Filtering {
                target: crate::app::FilterTarget::Branches,
                query: String::new(),
            },
            ..AppState::default()
        };
        assert_eq!(
            map_key(
                &state,
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
            ),
            Some(Action::Quit)
        );
    }

    #[test]
    fn repository_node_maps_fetch_pull_push_reflog_and_remotes() {
        let state = AppState::with_repository_paths(vec![PathBuf::from("/repo")]);
        assert_eq!(
            map_key(
                &state,
                KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE)
            ),
            Some(Action::OpenFetchRepositoryDialog)
        );
        assert_eq!(
            map_key(
                &state,
                KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE)
            ),
            Some(Action::OpenPullRebaseDialog)
        );
        assert_eq!(
            map_key(
                &state,
                KeyEvent::new(KeyCode::Char('P'), KeyModifiers::SHIFT)
            ),
            Some(Action::OpenPushDialog)
        );
        assert_eq!(
            map_key(
                &state,
                KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE)
            ),
            Some(Action::OpenReflog)
        );
        assert_eq!(
            map_key(
                &state,
                KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE)
            ),
            Some(Action::OpenRemotes)
        );
    }

    #[test]
    fn ctrl_g_opens_changes_from_every_normal_screen() {
        for screen in [
            Screen::BranchOverview,
            Screen::CommitDetail,
            Screen::FileDiffDetail,
            Screen::Reflog,
            Screen::Changes,
            Screen::Remotes,
        ] {
            let state = AppState {
                screen,
                ..AppState::with_repository_paths(vec![PathBuf::from("/repo")])
            };
            assert_eq!(
                map_key(
                    &state,
                    KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL)
                ),
                Some(Action::ToggleChanges)
            );
        }
    }

    #[test]
    fn remote_management_maps_add_edit_upstream_and_two_field_input() {
        let mut state = AppState {
            screen: Screen::Remotes,
            focus: FocusPanel::RemoteList,
            remotes_repository_index: Some(0),
            remotes: vec![crate::domain::RemoteInfo {
                name: "origin".into(),
                fetch_urls: vec!["ssh://example/repo.git".into()],
                push_urls: vec!["ssh://example/repo.git".into()],
                is_upstream: false,
                is_push_target: false,
            }],
            ..AppState::with_repository_paths(vec![PathBuf::from("/repo")])
        };
        state.ensure_valid_remote_selection();
        assert_eq!(
            map_key(
                &state,
                KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)
            ),
            Some(Action::OpenAddRemoteEditor)
        );
        assert_eq!(
            map_key(
                &state,
                KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE)
            ),
            Some(Action::OpenSetRemoteUrlEditor)
        );
        assert_eq!(
            map_key(
                &state,
                KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE)
            ),
            Some(Action::OpenSetUpstreamRemoteDialog)
        );

        state.mode = GlobalMode::EditingRemote {
            kind: RemoteEditKind::Add,
            field: RemoteInputField::Name,
            name: "origi".into(),
            url: String::new(),
            validation_error: None,
        };
        assert_eq!(
            map_key(
                &state,
                KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE)
            ),
            Some(Action::UpdateRemoteName("origin".into()))
        );
        assert_eq!(
            map_key(&state, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Some(Action::FocusNextRemoteField)
        );
        assert_eq!(
            map_key(&state, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Some(Action::SubmitRemoteEditor)
        );
    }

    #[test]
    fn commit_keys_toggle_selection_and_copy_hash_info_or_message() {
        let mut state = AppState {
            focus: FocusPanel::CommitList,
            ..AppState::default()
        };
        state.branch_commits.items.push(crate::domain::Commit {
            hash: crate::domain::CommitHash("0123456789abcdef".into()),
            short_hash: "01234567".into(),
            author: "Ada".into(),
            authored_at: "2026-07-16".into(),
            decorations: String::new(),
            subject: "copy me".into(),
        });
        state.ensure_valid_commit_selection();

        assert_eq!(
            map_key(
                &state,
                KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE)
            ),
            Some(Action::ToggleCommitCopySelection)
        );
        assert_eq!(
            map_key(
                &state,
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
            ),
            Some(Action::BeginChord(vec![
                KeyStroke::parse("Ctrl+C").unwrap()
            ]))
        );
        assert_eq!(
            map_key(
                &state,
                KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE)
            ),
            None
        );

        state.mode = GlobalMode::Chord {
            prefix: vec![KeyStroke::parse("Ctrl+C").unwrap()],
            started_at: Instant::now(),
        };
        assert_eq!(
            map_key(
                &state,
                KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE)
            ),
            Some(Action::CopySelectedCommitHashes)
        );
        assert_eq!(
            map_key(
                &state,
                KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE)
            ),
            Some(Action::CopyCurrentCommitInfo)
        );
        assert_eq!(
            map_key(
                &state,
                KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE)
            ),
            Some(Action::CopyCurrentCommitMessage)
        );
        assert_eq!(
            map_key(
                &state,
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
            ),
            Some(Action::Cancel)
        );
    }

    #[test]
    fn changes_keys_select_stage_unstage_and_open_commit() {
        let change = WorkingTreeChange {
            index_status: 'M',
            worktree_status: 'M',
            path: GitPath::from("both.txt"),
            old_path: None,
        };
        let mut state = AppState {
            screen: Screen::Changes,
            focus: FocusPanel::ChangesDiff,
            changes_repository_index: Some(0),
            changes: vec![change],
            ..AppState::with_repository_paths(vec![PathBuf::from("/repo")])
        };
        state.ensure_valid_changes_selection();
        state.change_selection.extend([
            ChangeSelection {
                group: ChangeGroup::Staged,
                path: GitPath::from("both.txt"),
            },
            ChangeSelection {
                group: ChangeGroup::Unstaged,
                path: GitPath::from("both.txt"),
            },
        ]);
        for (key, expected) in [
            (KeyCode::Char(' '), Action::ToggleChangeSelection),
            (KeyCode::Char('s'), Action::StageSelectedChanges),
            (KeyCode::Char('u'), Action::UnstageSelectedChanges),
            (KeyCode::Char('c'), Action::OpenCommitDialog),
        ] {
            assert_eq!(
                map_key(&state, KeyEvent::new(key, KeyModifiers::NONE)),
                Some(expected)
            );
        }
    }

    #[test]
    fn commit_message_editor_accepts_text_validates_on_submit_and_cancels() {
        let state = AppState {
            mode: GlobalMode::EditingCommitMessage {
                input: "subject".into(),
                validation_error: None,
            },
            ..AppState::default()
        };
        assert_eq!(
            map_key(
                &state,
                KeyEvent::new(KeyCode::Char('!'), KeyModifiers::NONE)
            ),
            Some(Action::UpdateCommitMessage("subject!".into()))
        );
        assert_eq!(
            map_key(&state, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Some(Action::SubmitCommit)
        );
        assert_eq!(
            map_key(&state, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            Some(Action::Cancel)
        );
    }

    #[test]
    fn reset_mode_dialog_maps_three_explicit_choices() {
        let state = AppState {
            mode: GlobalMode::Confirming {
                dialog: crate::app::ConfirmDialog::ResetModeChoice {
                    repository_index: 0,
                    commit: crate::domain::CommitHash("abcdef".into()),
                    short_hash: "abcdef".into(),
                },
            },
            ..AppState::default()
        };
        assert_eq!(
            map_key(
                &state,
                KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE)
            ),
            Some(Action::ChooseResetSoft)
        );
        assert_eq!(
            map_key(
                &state,
                KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE)
            ),
            Some(Action::ChooseResetMixed)
        );
        assert_eq!(
            map_key(
                &state,
                KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE)
            ),
            Some(Action::ChooseResetHard)
        );
    }
}
