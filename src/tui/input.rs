use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::{
    app::{
        Action, AppState, GlobalMode, ModalShortcutSetId, RemoteEditKind, RemoteInputField,
        modal_shortcut_set_id,
    },
    config::{KeyResolution, KeyStroke},
};

type ModeKeyHandler = fn(&AppState, KeyEvent) -> Option<Action>;

/// Modal modes own text and safety keys, so they use a separate callable jump
/// table instead of falling through to the focused normal-mode command table.
/// `ModalShortcutSetId` also selects the shared footer/help operation set.
#[derive(Clone, Copy)]
struct ModeKeyTable {
    id: ModalShortcutSetId,
    handle: ModeKeyHandler,
}

static MODE_KEY_TABLES: &[ModeKeyTable] = &[
    ModeKeyTable {
        id: ModalShortcutSetId::Filter,
        handle: map_filter_table,
    },
    ModeKeyTable {
        id: ModalShortcutSetId::Confirmation,
        handle: map_confirmation_table,
    },
    ModeKeyTable {
        id: ModalShortcutSetId::ResetMode,
        handle: map_reset_mode_table,
    },
    ModeKeyTable {
        id: ModalShortcutSetId::TypedConfirmation,
        handle: map_typed_confirmation_table,
    },
    ModeKeyTable {
        id: ModalShortcutSetId::CommitMessage,
        handle: map_commit_message_table,
    },
    ModeKeyTable {
        id: ModalShortcutSetId::RemoteAdd,
        handle: map_remote_table,
    },
    ModeKeyTable {
        id: ModalShortcutSetId::RemoteUrl,
        handle: map_remote_table,
    },
    ModeKeyTable {
        id: ModalShortcutSetId::Error,
        handle: map_error_table,
    },
    ModeKeyTable {
        id: ModalShortcutSetId::CommandPrompt,
        handle: map_command_prompt_table,
    },
    ModeKeyTable {
        id: ModalShortcutSetId::ShortcutHelp,
        handle: map_shortcut_help_table,
    },
];

pub fn map_key(app: &AppState, event: KeyEvent) -> Option<Action> {
    if !matches!(event.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
        return None;
    }
    match &app.mode {
        GlobalMode::Chord { prefix, .. } => {
            if matches!(event.code, KeyCode::Esc) {
                return Some(Action::Cancel);
            }
            map_resolved_key(app, prefix, event).or(Some(Action::Cancel))
        }
        GlobalMode::Normal => map_resolved_key(app, &[], event),
        _ => {
            if event.modifiers.contains(KeyModifiers::CONTROL)
                && matches!(event.code, KeyCode::Char('c') | KeyCode::Char('C'))
            {
                return Some(Action::Quit);
            }
            let id = modal_shortcut_set_id(&app.mode)?;
            let table = MODE_KEY_TABLES.iter().find(|table| table.id == id)?;
            (table.handle)(app, event)
        }
    }
}

fn map_filter_table(app: &AppState, event: KeyEvent) -> Option<Action> {
    let GlobalMode::Filtering { query, .. } = &app.mode else {
        return None;
    };
    map_filtering(event, query)
}

fn map_confirmation_table(_app: &AppState, event: KeyEvent) -> Option<Action> {
    match event.code {
        KeyCode::Enter => Some(Action::Confirm),
        KeyCode::Esc | KeyCode::Char('q') => Some(Action::Cancel),
        _ => None,
    }
}

fn map_reset_mode_table(_app: &AppState, event: KeyEvent) -> Option<Action> {
    match event.code {
        KeyCode::Char('s') | KeyCode::Char('S') => Some(Action::ChooseResetSoft),
        KeyCode::Char('m') | KeyCode::Char('M') => Some(Action::ChooseResetMixed),
        KeyCode::Char('h') | KeyCode::Char('H') => Some(Action::ChooseResetHard),
        KeyCode::Esc | KeyCode::Char('q') => Some(Action::Cancel),
        _ => None,
    }
}

fn map_typed_confirmation_table(app: &AppState, event: KeyEvent) -> Option<Action> {
    let GlobalMode::TypingConfirmation { input, .. } = &app.mode else {
        return None;
    };
    map_typed_confirmation(event, input)
}

fn map_commit_message_table(app: &AppState, event: KeyEvent) -> Option<Action> {
    let GlobalMode::EditingCommitMessage { input, .. } = &app.mode else {
        return None;
    };
    map_commit_message(event, input)
}

fn map_remote_table(app: &AppState, event: KeyEvent) -> Option<Action> {
    let GlobalMode::EditingRemote {
        kind,
        field,
        name,
        url,
        ..
    } = &app.mode
    else {
        return None;
    };
    map_remote_editor(event, kind, *field, name, url)
}

fn map_error_table(_app: &AppState, event: KeyEvent) -> Option<Action> {
    match event.code {
        KeyCode::Enter | KeyCode::Esc | KeyCode::Char('q') => Some(Action::DismissError),
        _ => None,
    }
}

fn map_command_prompt_table(app: &AppState, event: KeyEvent) -> Option<Action> {
    let GlobalMode::CommandPrompt { input, .. } = &app.mode else {
        return None;
    };
    match event.code {
        KeyCode::Char(character)
            if !event
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            let mut next = input.clone();
            next.push(character);
            Some(Action::UpdateCommandPrompt(next))
        }
        KeyCode::Backspace => {
            let mut next = input.clone();
            next.pop();
            Some(Action::UpdateCommandPrompt(next))
        }
        KeyCode::Enter => Some(Action::SubmitCommandPrompt),
        KeyCode::Esc => Some(Action::Cancel),
        _ => None,
    }
}

fn map_shortcut_help_table(_app: &AppState, event: KeyEvent) -> Option<Action> {
    match event.code {
        KeyCode::Up | KeyCode::Char('w' | 'k') => Some(Action::MoveUp),
        KeyCode::Down | KeyCode::Char('s' | 'j') => Some(Action::MoveDown),
        KeyCode::PageUp => Some(Action::PageUp),
        KeyCode::PageDown => Some(Action::PageDown),
        KeyCode::Home => Some(Action::Home),
        KeyCode::End => Some(Action::End),
        KeyCode::Enter
        | KeyCode::Esc
        | KeyCode::Char('q')
        | KeyCode::Char('h')
        | KeyCode::Char('H') => Some(Action::Cancel),
        _ => None,
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
        app::{ChangeGroup, ChangeSelection, FocusPanel, ModalShortcutSetId, Screen},
        config::KeyStroke,
        domain::{
            ChangedFile, Commit, CommitDetail, CommitHash, FileChangeKind, GitPath,
            WorkingTreeChange,
        },
    };

    #[test]
    fn modal_input_jump_table_covers_every_documented_operation_set() {
        assert_eq!(MODE_KEY_TABLES.len(), ModalShortcutSetId::ALL.len());
        for (index, id) in ModalShortcutSetId::ALL.iter().copied().enumerate() {
            assert_eq!(MODE_KEY_TABLES[index].id, id);
        }
    }

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
    fn horizontal_keys_follow_the_column_hierarchy_without_wrapping() {
        let commit = Commit {
            hash: CommitHash("0123456789abcdef".into()),
            short_hash: "0123456".into(),
            author: "Ada".into(),
            authored_at: "2026-07-16".into(),
            decorations: String::new(),
            subject: "navigation".into(),
        };
        let mut state = AppState::default();
        state.branch_commits.items.push(commit.clone());
        state.ensure_valid_commit_selection();

        let left = KeyEvent::new(KeyCode::Left, KeyModifiers::NONE);
        let right = KeyEvent::new(KeyCode::Right, KeyModifiers::NONE);
        assert_eq!(map_key(&state, left), None);
        assert_eq!(map_key(&state, right), Some(Action::MoveRight));

        state.focus = FocusPanel::CommitList;
        assert_eq!(map_key(&state, left), Some(Action::MoveLeft));
        assert_eq!(map_key(&state, right), Some(Action::MoveRight));

        state.screen = Screen::CommitDetail;
        state.current_commit_detail = Some(CommitDetail {
            commit,
            author_email: "ada@example.invalid".into(),
            committer: "Ada".into(),
            committer_email: "ada@example.invalid".into(),
            committed_at: "2026-07-16".into(),
            message: "navigation".into(),
            files: vec![ChangedFile {
                kind: FileChangeKind::Modified,
                path: GitPath::from("src/main.rs"),
                old_path: None,
                additions: Some(1),
                deletions: Some(1),
                hunks: Vec::new(),
                is_binary: false,
            }],
        });
        state.ensure_valid_file_selection();
        state.focus = FocusPanel::CommitFileList;
        assert_eq!(map_key(&state, left), Some(Action::MoveLeft));
        assert_eq!(map_key(&state, right), Some(Action::MoveRight));

        state.screen = Screen::FileDiffDetail;
        state.focus = FocusPanel::FileList;
        assert_eq!(map_key(&state, left), Some(Action::MoveLeft));
        assert_eq!(map_key(&state, right), Some(Action::MoveRight));

        state.focus = FocusPanel::DiffView;
        assert_eq!(map_key(&state, left), Some(Action::MoveLeft));
        assert_eq!(map_key(&state, right), None);
    }

    #[test]
    fn wasd_maps_to_up_left_down_right_in_the_current_focus() {
        let mut state = AppState {
            focus: FocusPanel::CommitList,
            ..AppState::default()
        };
        for index in 0..3 {
            state.branch_commits.items.push(Commit {
                hash: CommitHash(format!("{index:040x}")),
                short_hash: format!("{index:07x}"),
                author: "Ada".into(),
                authored_at: "2026-07-16".into(),
                decorations: String::new(),
                subject: format!("commit {index}"),
            });
        }
        state.selection.selected_commit_index = Some(1);

        for (key, action) in [
            ('w', Action::MoveUp),
            ('a', Action::MoveLeft),
            ('s', Action::MoveDown),
            ('d', Action::MoveRight),
        ] {
            assert_eq!(
                map_key(
                    &state,
                    KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE)
                ),
                Some(action.clone()),
                "{key} should map to {action:?}"
            );
        }
    }

    #[test]
    fn confirmation_q_cancels_instead_of_quitting() {
        let state = AppState {
            mode: GlobalMode::Confirming {
                dialog: crate::app::ConfirmDialog::CherryPickSelected {
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
                KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT)
            ),
            Some(Action::OpenAddRemoteEditor)
        );
        assert_eq!(
            map_key(
                &state,
                KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)
            ),
            None
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
            Some(Action::ToggleCommitSelection)
        );
        assert_eq!(
            map_key(
                &state,
                KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE)
            ),
            None,
            "cherry-pick must stay unavailable until a commit is explicitly selected"
        );
        state.branch_commits_repository_index = Some(0);
        state.commit_selection_repository_index = Some(0);
        state
            .commit_selection
            .insert(crate::domain::CommitHash("0123456789abcdef".into()));
        assert_eq!(
            map_key(
                &state,
                KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE)
            ),
            Some(Action::OpenCherryPickSelectedDialog)
        );
        assert_eq!(
            map_key(
                &state,
                KeyEvent::new(KeyCode::Char('Y'), KeyModifiers::SHIFT)
            ),
            None,
            "the former queue-apply shortcut must stay removed"
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
    fn copy_chord_mounts_commit_operations_only_on_commits_and_file_paths_only_on_files() {
        let commit = Commit {
            hash: CommitHash("0123456789abcdef".into()),
            short_hash: "01234567".into(),
            author: "Ada".into(),
            authored_at: "2026-07-16".into(),
            decorations: String::new(),
            subject: "context tables".into(),
        };
        let mut state = AppState {
            screen: Screen::CommitDetail,
            focus: FocusPanel::CommitFileList,
            ..AppState::default()
        };
        state.branch_commits.items.push(commit.clone());
        state.ensure_valid_commit_selection();
        state.current_commit_detail = Some(CommitDetail {
            commit,
            author_email: "ada@example.invalid".into(),
            committer: "Ada".into(),
            committer_email: "ada@example.invalid".into(),
            committed_at: "2026-07-16".into(),
            message: "context tables".into(),
            files: vec![ChangedFile {
                kind: FileChangeKind::Modified,
                path: GitPath::from("src/nested/main.rs"),
                old_path: None,
                additions: Some(1),
                deletions: Some(1),
                hunks: Vec::new(),
                is_binary: false,
            }],
        });
        state.ensure_valid_file_selection();

        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(
            map_key(&state, ctrl_c),
            Some(Action::BeginChord(vec![
                KeyStroke::parse("Ctrl+C").unwrap()
            ]))
        );
        let prefix = vec![KeyStroke::parse("Ctrl+C").unwrap()];
        for (key, action) in [
            ('n', Action::CopySelectedFileName),
            ('a', Action::CopySelectedFileAbsolutePath),
            ('r', Action::CopySelectedFileRelativePath),
        ] {
            state.mode = GlobalMode::Chord {
                prefix: prefix.clone(),
                started_at: Instant::now(),
            };
            assert_eq!(
                map_key(
                    &state,
                    KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE)
                ),
                Some(action)
            );
        }

        state.mode = GlobalMode::Normal;
        state.screen = Screen::FileDiffDetail;
        state.focus = FocusPanel::DiffView;
        assert_eq!(map_key(&state, ctrl_c), Some(Action::Quit));
    }

    #[test]
    fn global_shortcut_reference_opens_and_uses_its_own_modal_table() {
        let state = AppState {
            screen: Screen::CommitDetail,
            focus: FocusPanel::CommitFileList,
            ..AppState::default()
        };
        let ctrl_question = KeyEvent::new(KeyCode::Char('?'), KeyModifiers::CONTROL);
        assert_eq!(
            map_key(
                &state,
                KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE)
            ),
            Some(Action::OpenShortcutHelp)
        );
        assert_eq!(map_key(&state, ctrl_question), None);
        assert_eq!(
            map_key(&state, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE)),
            Some(Action::MoveLeft)
        );

        let help = AppState {
            mode: GlobalMode::ShortcutHelp { scroll: 0 },
            focus: FocusPanel::Popup,
            previous_focus: Some(FocusPanel::BranchList),
            ..AppState::default()
        };
        assert_eq!(
            map_key(&help, KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE)),
            Some(Action::PageDown)
        );
        assert_eq!(
            map_key(&help, KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE)),
            Some(Action::MoveUp)
        );
        assert_eq!(
            map_key(&help, KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE)),
            Some(Action::MoveDown)
        );
        assert_eq!(map_key(&help, ctrl_question), None);
        assert_eq!(
            map_key(&help, KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE)),
            Some(Action::Cancel)
        );
    }

    #[test]
    fn quick_command_prompt_opens_with_ctrl_backtick_and_accepts_text() {
        let state = AppState::default();
        assert_eq!(
            map_key(
                &state,
                KeyEvent::new(KeyCode::Char('`'), KeyModifiers::CONTROL)
            ),
            Some(Action::OpenCommandPrompt)
        );
        // Ctrl+Space is intentionally unbound; only the requested Ctrl+`
        // sequence opens the command prompt.
        assert_eq!(
            map_key(
                &state,
                KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL)
            ),
            None
        );

        let prompt = AppState {
            mode: GlobalMode::CommandPrompt {
                input: "hel".into(),
                validation_error: Some("old error".into()),
            },
            focus: FocusPanel::Popup,
            ..AppState::default()
        };
        assert_eq!(
            map_key(
                &prompt,
                KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE)
            ),
            Some(Action::UpdateCommandPrompt("help".into()))
        );
        assert_eq!(
            map_key(
                &prompt,
                KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)
            ),
            Some(Action::UpdateCommandPrompt("he".into()))
        );
        assert_eq!(
            map_key(&prompt, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Some(Action::SubmitCommandPrompt)
        );
        assert_eq!(
            map_key(&prompt, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
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
            (KeyCode::Char('S'), Action::StageSelectedChanges),
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
