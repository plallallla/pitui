use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::app::{
    Action, AppState, FocusPanel, GlobalMode, RemoteEditKind, RemoteInputField, Screen,
    ShortcutMenu,
};

pub fn map_key(app: &AppState, event: KeyEvent) -> Option<Action> {
    if !matches!(event.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
        return None;
    }
    if event.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(event.code, KeyCode::Char('g') | KeyCode::Char('G'))
        && matches!(app.mode, GlobalMode::Normal)
    {
        return Some(Action::ToggleChanges);
    }
    if event.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(event.code, KeyCode::Char('r') | KeyCode::Char('R'))
        && matches!(app.mode, GlobalMode::Normal)
    {
        return Some(Action::RefreshRepository);
    }
    if event.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(event.code, KeyCode::Char('c') | KeyCode::Char('C'))
    {
        if matches!(app.mode, GlobalMode::Shortcut { .. }) {
            return Some(Action::Cancel);
        }
        let can_copy_commit = matches!(app.mode, GlobalMode::Normal)
            && app.selected_commit().is_some()
            && matches!(
                app.screen,
                Screen::BranchOverview | Screen::CommitDetail | Screen::FileDiffDetail
            );
        if can_copy_commit {
            return Some(Action::OpenCommitCopyShortcuts);
        }
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
        GlobalMode::Shortcut {
            menu: ShortcutMenu::CommitCopy,
        } => map_commit_copy_shortcut(event),
        GlobalMode::Error => match event.code {
            KeyCode::Enter | KeyCode::Esc | KeyCode::Char('q') => Some(Action::DismissError),
            _ => None,
        },
        GlobalMode::Normal => map_normal(event, app),
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

fn map_commit_copy_shortcut(event: KeyEvent) -> Option<Action> {
    match event.code {
        KeyCode::Char('h' | 'H') => Some(Action::CopySelectedCommitHashes),
        KeyCode::Char('i' | 'I') => Some(Action::CopyCurrentCommitInfo),
        KeyCode::Char('m' | 'M') => Some(Action::CopyCurrentCommitMessage),
        KeyCode::Esc | KeyCode::Char('q' | 'Q') => Some(Action::Cancel),
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

fn map_normal(event: KeyEvent, app: &AppState) -> Option<Action> {
    let global = match event.code {
        KeyCode::Char('q') => Some(Action::Quit),
        KeyCode::Tab => Some(Action::FocusNext),
        KeyCode::BackTab => Some(Action::FocusPrev),
        KeyCode::Esc => Some(Action::Back),
        KeyCode::Up | KeyCode::Char('k') => Some(Action::MoveUp),
        KeyCode::Down | KeyCode::Char('j') => Some(Action::MoveDown),
        KeyCode::Left | KeyCode::Char('h') => Some(Action::MoveLeft),
        KeyCode::Right | KeyCode::Char('l') => Some(Action::MoveRight),
        KeyCode::PageUp => Some(Action::PageUp),
        KeyCode::PageDown => Some(Action::PageDown),
        KeyCode::Home => Some(Action::Home),
        KeyCode::End => Some(Action::End),
        _ => None,
    };
    if global.is_some() {
        return global;
    }

    match (app.screen, app.focus, event.code) {
        (Screen::BranchOverview, FocusPanel::BranchList, KeyCode::Enter) => {
            Some(Action::LoadCommitsForSelectedBranch)
        }
        (Screen::BranchOverview, FocusPanel::BranchList, KeyCode::Char('f'))
            if app.selected_repository_node_index().is_some() =>
        {
            Some(Action::OpenFetchRepositoryDialog)
        }
        (Screen::BranchOverview, FocusPanel::BranchList, KeyCode::Char('p'))
            if app.selected_repository_node_index().is_some() =>
        {
            Some(Action::OpenPullRebaseDialog)
        }
        (Screen::BranchOverview, FocusPanel::BranchList, KeyCode::Char('P'))
            if app.selected_repository_node_index().is_some() =>
        {
            Some(Action::OpenPushDialog)
        }
        (Screen::BranchOverview, FocusPanel::BranchList, KeyCode::Char('g'))
            if app.selected_repository_node_index().is_some() =>
        {
            Some(Action::OpenReflog)
        }
        (Screen::BranchOverview, FocusPanel::BranchList, KeyCode::Char('o')) => {
            Some(Action::OpenRemotes)
        }
        (Screen::BranchOverview, FocusPanel::BranchList, KeyCode::Char('s')) => {
            Some(Action::OpenSwitchBranchDialog)
        }
        (Screen::BranchOverview, FocusPanel::BranchList, KeyCode::Char('b')) => {
            Some(Action::OpenRebaseDialog)
        }
        (Screen::BranchOverview, FocusPanel::BranchList, KeyCode::Char('/')) => {
            Some(Action::StartFilter)
        }
        (Screen::BranchOverview, FocusPanel::CommitList, KeyCode::Enter)
        | (Screen::CommitDetail, FocusPanel::CommitList, KeyCode::Enter) => {
            Some(Action::OpenCommitDetail)
        }
        (Screen::BranchOverview, FocusPanel::CommitList, KeyCode::Char(' '))
        | (Screen::CommitDetail, FocusPanel::CommitList, KeyCode::Char(' ')) => {
            Some(Action::ToggleCommitCopySelection)
        }
        (Screen::BranchOverview, FocusPanel::CommitList, KeyCode::Char('/')) => {
            Some(Action::StartFilter)
        }
        (Screen::BranchOverview, FocusPanel::CommitList, KeyCode::Char('y'))
        | (Screen::CommitDetail, FocusPanel::CommitList, KeyCode::Char('y'))
        | (Screen::CommitDetail, FocusPanel::CommitFileList, KeyCode::Char('y')) => {
            Some(Action::QueueCherryPickSelectedCommit)
        }
        (Screen::BranchOverview, FocusPanel::CommitList, KeyCode::Char('Y'))
        | (Screen::CommitDetail, FocusPanel::CommitList, KeyCode::Char('Y')) => {
            Some(Action::OpenCherryPickQueueDialog)
        }
        (Screen::BranchOverview, FocusPanel::CommitList, KeyCode::Char('R'))
        | (Screen::CommitDetail, FocusPanel::CommitList, KeyCode::Char('R')) => {
            Some(Action::OpenResetDialog)
        }
        (Screen::CommitDetail, FocusPanel::CommitList, KeyCode::Char('/')) => {
            Some(Action::StartFilter)
        }
        (Screen::CommitDetail, FocusPanel::CommitFileList, KeyCode::Char(' ')) => {
            Some(Action::ToggleFileExpanded)
        }
        (Screen::CommitDetail, FocusPanel::CommitFileList, KeyCode::Enter)
        | (Screen::CommitDetail, FocusPanel::CommitFileList, KeyCode::Char('v'))
        | (Screen::FileDiffDetail, FocusPanel::FileList, KeyCode::Enter) => {
            Some(Action::OpenSelectedFileDiff)
        }
        (Screen::FileDiffDetail, _, KeyCode::Char('n')) => Some(Action::NextFile),
        (Screen::FileDiffDetail, _, KeyCode::Char('p')) => Some(Action::PrevFile),
        (Screen::FileDiffDetail, _, KeyCode::Char('v')) => Some(Action::ToggleDiffMode),
        (Screen::FileDiffDetail, _, KeyCode::Char('w')) => Some(Action::ToggleWrap),
        (Screen::Reflog, FocusPanel::ReflogList, KeyCode::Char('R')) => {
            Some(Action::OpenResetDialog)
        }
        (Screen::Changes, FocusPanel::ChangesTree, KeyCode::Enter) => {
            Some(Action::ActivateSelectedChange)
        }
        (Screen::Changes, _, KeyCode::Char(' ')) => Some(Action::ToggleChangeSelection),
        (Screen::Changes, _, KeyCode::Char('s')) => Some(Action::StageSelectedChanges),
        (Screen::Changes, _, KeyCode::Char('u')) => Some(Action::UnstageSelectedChanges),
        (Screen::Changes, _, KeyCode::Char('c')) => Some(Action::OpenCommitDialog),
        (Screen::Changes, _, KeyCode::Char('v')) => Some(Action::ToggleDiffMode),
        (Screen::Changes, _, KeyCode::Char('w')) => Some(Action::ToggleWrap),
        (Screen::Remotes, FocusPanel::RemoteList, KeyCode::Char('a')) => {
            Some(Action::OpenAddRemoteEditor)
        }
        (Screen::Remotes, FocusPanel::RemoteList, KeyCode::Char('e'))
            if app.selected_remote().is_some() =>
        {
            Some(Action::OpenSetRemoteUrlEditor)
        }
        (Screen::Remotes, FocusPanel::RemoteList, KeyCode::Char('u'))
            if app.selected_remote().is_some() =>
        {
            Some(Action::OpenSetUpstreamRemoteDialog)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crossterm::event::KeyEvent;

    use super::*;

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
                ..AppState::default()
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
    fn file_detail_panels_map_home_end_and_page_navigation() {
        for (screen, focus) in [
            (Screen::CommitDetail, FocusPanel::CommitFileList),
            (Screen::FileDiffDetail, FocusPanel::FileList),
            (Screen::FileDiffDetail, FocusPanel::DiffView),
            (Screen::Changes, FocusPanel::ChangesTree),
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
                    Some(action),
                    "missing {key:?} mapping for {screen:?}/{focus:?}"
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
                ..AppState::default()
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
            remotes: vec![crate::domain::RemoteInfo {
                name: "origin".into(),
                fetch_urls: vec!["ssh://example/repo.git".into()],
                push_urls: vec!["ssh://example/repo.git".into()],
                is_upstream: false,
                is_push_target: false,
            }],
            ..AppState::default()
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
            Some(Action::OpenCommitCopyShortcuts)
        );
        assert_eq!(
            map_key(
                &state,
                KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE)
            ),
            None
        );

        state.mode = GlobalMode::Shortcut {
            menu: ShortcutMenu::CommitCopy,
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
        let state = AppState {
            screen: Screen::Changes,
            focus: FocusPanel::ChangesDiff,
            ..AppState::default()
        };
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
