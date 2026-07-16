use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::app::{Action, AppState, FocusPanel, GlobalMode, Screen};

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
        && matches!(event.code, KeyCode::Char('c') | KeyCode::Char('C'))
    {
        let can_copy_commit = matches!(app.mode, GlobalMode::Normal)
            && app.selected_commit().is_some()
            && matches!(
                app.screen,
                Screen::BranchOverview | Screen::CommitDetail | Screen::FileDiffDetail
            );
        if can_copy_commit {
            return Some(
                if event.modifiers.contains(KeyModifiers::SHIFT)
                    || matches!(event.code, KeyCode::Char('C'))
                {
                    Action::CopyCurrentCommitInfo
                } else {
                    Action::CopySelectedCommitHashes
                },
            );
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
        GlobalMode::Error => match event.code {
            KeyCode::Enter | KeyCode::Esc | KeyCode::Char('q') => Some(Action::DismissError),
            _ => None,
        },
        GlobalMode::Normal => map_normal(event, app),
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
        KeyCode::Char('r') => Some(Action::RefreshRepository),
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
        (Screen::BranchOverview, FocusPanel::BranchList, KeyCode::Char('g'))
            if app.selected_repository_node_index().is_some() =>
        {
            Some(Action::OpenReflog)
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
        (Screen::BranchOverview, FocusPanel::CommitList, KeyCode::Char('c'))
        | (Screen::CommitDetail, FocusPanel::CommitList, KeyCode::Char('c')) => {
            Some(Action::CopySelectedCommitHashes)
        }
        (Screen::BranchOverview, FocusPanel::CommitList, KeyCode::Char('i'))
        | (Screen::CommitDetail, FocusPanel::CommitList, KeyCode::Char('i')) => {
            Some(Action::CopyCurrentCommitInfo)
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
        (Screen::CommitDetail, FocusPanel::CommitFileList, KeyCode::Char('i')) => {
            Some(Action::CopyCurrentCommitInfo)
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
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crossterm::event::KeyEvent;

    use super::*;

    #[test]
    fn maps_global_exit_and_refresh() {
        let state = AppState::default();
        assert_eq!(
            map_key(
                &state,
                KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)
            ),
            Some(Action::Quit)
        );
        assert_eq!(
            map_key(
                &state,
                KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE)
            ),
            Some(Action::RefreshRepository)
        );
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
    fn repository_node_maps_fetch_and_reflog() {
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
                KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE)
            ),
            Some(Action::OpenReflog)
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
    fn commit_keys_toggle_selection_and_copy_hash_or_info() {
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
            Some(Action::CopySelectedCommitHashes)
        );
        assert_eq!(
            map_key(
                &state,
                KeyEvent::new(
                    KeyCode::Char('C'),
                    KeyModifiers::CONTROL | KeyModifiers::SHIFT
                )
            ),
            Some(Action::CopyCurrentCommitInfo)
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
