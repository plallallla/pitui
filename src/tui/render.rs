use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{
    app::{
        AppState, BranchTreeNode, ChangeGroup, ChangesTreeNode, ConfirmDialog, DiffViewMode,
        FilterTarget, FocusPanel, GlobalMode, Screen,
    },
    domain::{DiffCellKind, DiffLineKind, FileDiff, side_by_side_rows},
};

const FOCUSED_BORDER: Color = Color::Yellow;
const NORMAL_BORDER: Color = Color::DarkGray;

/// Git metadata and file contents are untrusted terminal input. Never let C0
/// controls, escape sequences, or bidi overrides reach the backend verbatim.
fn terminal_safe(value: &str) -> String {
    let mut safe = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\t' => safe.push_str("    "),
            '\n' => safe.push_str(" ⏎ "),
            '\r' => {}
            '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' => safe.push('�'),
            character if character.is_control() => safe.push('�'),
            character => safe.push(character),
        }
    }
    safe
}

pub fn render(frame: &mut Frame<'_>, app: &AppState) {
    let area = frame.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(area);

    render_status_bar(frame, app, rows[0]);
    match app.screen {
        Screen::BranchOverview => render_branch_overview(frame, app, rows[1]),
        Screen::CommitDetail => render_commit_detail(frame, app, rows[1]),
        Screen::FileDiffDetail => render_file_diff(frame, app, rows[1], area.width),
        Screen::Reflog => render_reflog(frame, app, rows[1]),
        Screen::Changes => render_changes(frame, app, rows[1], area.width),
    }
    render_hotkeys(frame, app, rows[2]);
    render_popup(frame, app, area);
}

fn panel_block(title: impl Into<String>, focused: bool) -> Block<'static> {
    Block::default()
        .title(title.into())
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if focused {
            FOCUSED_BORDER
        } else {
            NORMAL_BORDER
        }))
}

fn render_status_bar(frame: &mut Frame<'_>, app: &AppState, area: Rect) {
    let active_repository = app.active_repository();
    let (repo, branch, head, counts) = active_repository.map_or_else(
        || {
            (
                "—".to_string(),
                "—".to_string(),
                "—".to_string(),
                "S=0 M=0 U=0 C=0".to_string(),
            )
        },
        |repo| {
            let branch = repo
                .current_branch
                .as_ref()
                .map_or_else(|| "detached".to_string(), |branch| branch.0.clone());
            (
                repo.name.clone(),
                branch,
                if repo.head.0.is_empty() {
                    "unborn".to_string()
                } else {
                    repo.head.short().to_string()
                },
                format!(
                    "S={} M={} U={} C={}",
                    repo.status.staged,
                    repo.status.modified,
                    repo.status.untracked,
                    repo.status.conflicted
                ),
            )
        },
    );
    let viewing = app
        .branch_commits
        .viewing_branch
        .as_ref()
        .map_or("—", |branch| branch.0.as_str());
    let selected_commit = match app.screen {
        Screen::Reflog => app
            .selected_reflog()
            .map_or("—", |entry| entry.short_hash.as_str()),
        Screen::Changes => "—",
        _ => app
            .selected_commit()
            .map_or("—", |commit| commit.short_hash.as_str()),
    };
    let selected_file = if app.screen == Screen::Changes {
        app.selected_change()
            .map_or("—", |(_, change)| change.path.as_str())
    } else {
        app.selected_file().map_or("—", |file| file.path.as_str())
    };
    let mode = match app.mode {
        GlobalMode::Normal => "NORMAL",
        GlobalMode::Filtering { .. } => "FILTER",
        GlobalMode::Confirming { .. } => "CONFIRM",
        GlobalMode::TypingConfirmation { .. } => "TYPE",
        GlobalMode::EditingCommitMessage { .. } => "COMMIT",
        GlobalMode::Error => "ERROR",
    };
    let spinner = if app.is_loading() {
        ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"][(app.tick_count as usize) % 10]
    } else {
        ""
    };
    let tracking = active_repository.map_or(String::new(), |repo| {
        format!("↑{} ↓{}", repo.status.ahead, repo.status.behind)
    });
    let view = match app.screen {
        Screen::BranchOverview => "BRANCH",
        Screen::CommitDetail => "COMMIT",
        Screen::FileDiffDetail => "DIFF",
        Screen::Reflog => "REFLOG",
        Screen::Changes => "CHANGES",
    };
    let focus = match app.focus {
        FocusPanel::BranchList => "BRANCHES",
        FocusPanel::CommitList => "COMMITS",
        FocusPanel::CommitFileList => "CHANGED_FILES",
        FocusPanel::FileList => "FILES",
        FocusPanel::DiffView => "DIFF",
        FocusPanel::ReflogList => "REFLOG",
        FocusPanel::ChangesTree => "CHANGES_TREE",
        FocusPanel::ChangesDiff => "CHANGES_DIFF",
        FocusPanel::Popup => "POPUP",
    };
    let line = if area.width < 100 {
        format!(
            " {repo} [{}/{}] | {branch} | viewing={viewing} | V={} F={} | {mode}{spinner} | S{} M{} U{} C{} | ↑{}↓{} ",
            app.active_repository_index.map_or(0, |index| index + 1),
            app.repositories.len(),
            &view[..1],
            &focus[..1],
            active_repository.map_or(0, |repo| repo.status.staged),
            active_repository.map_or(0, |repo| repo.status.modified),
            active_repository.map_or(0, |repo| repo.status.untracked),
            active_repository.map_or(0, |repo| repo.status.conflicted),
            active_repository.map_or(0, |repo| repo.status.ahead),
            active_repository.map_or(0, |repo| repo.status.behind),
        )
    } else if area.width < 140 {
        format!(
            " repo={repo} ({}/{}) | branch={branch} | viewing={viewing} | view={view} | focus={focus} | op={mode}{spinner} | {counts} | {tracking} | queue={} selected={} ",
            app.active_repository_index.map_or(0, |index| index + 1),
            app.repositories.len(),
            app.cherry_pick_queue.len(),
            app.commit_copy_selection.len()
        )
    } else {
        format!(
            " repo={repo} ({}/{}) | branch={branch} | head={head} | viewing={viewing} | commit={selected_commit} | file={selected_file} | view={view} | focus={focus} | op={mode}{spinner} | {counts} | {tracking} | queue={} selected={} ",
            app.active_repository_index.map_or(0, |index| index + 1),
            app.repositories.len(),
            app.cherry_pick_queue.len(),
            app.commit_copy_selection.len()
        )
    };
    frame.render_widget(
        Paragraph::new(terminal_safe(&line))
            .style(Style::default().bg(Color::Blue).fg(Color::White)),
        area,
    );
}

fn columns(area: Rect) -> [Rect; 2] {
    let split = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(36), Constraint::Percentage(64)])
        .split(area);
    [split[0], split[1]]
}

fn list_state(selected: Option<usize>) -> ListState {
    let mut state = ListState::default();
    state.select(selected);
    state
}

fn render_branch_overview(frame: &mut Frame<'_>, app: &AppState, area: Rect) {
    let [left, right] = columns(area);
    render_branches(frame, app, left);
    render_commits(frame, app, right);
}

fn render_reflog(frame: &mut Frame<'_>, app: &AppState, area: Rect) {
    let [left, right] = columns(area);
    let items = if app.reflog_entries.is_empty() {
        vec![ListItem::new(Line::styled(
            "No reflog entries",
            Style::default().fg(Color::DarkGray),
        ))]
    } else {
        app.reflog_entries
            .iter()
            .map(|entry| {
                ListItem::new(Line::from(vec![
                    Span::styled(
                        terminal_safe(&entry.short_hash),
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::raw(" "),
                    Span::styled(
                        terminal_safe(&entry.selector),
                        Style::default().fg(Color::Cyan),
                    ),
                    Span::raw(" "),
                    Span::styled(
                        terminal_safe(&entry.action),
                        Style::default().fg(Color::Magenta),
                    ),
                    Span::raw(terminal_safe(&format!("  {}", entry.message))),
                ]))
            })
            .collect()
    };
    let list = List::new(items)
        .block(panel_block("Reflog", app.focus == FocusPanel::ReflogList))
        .highlight_symbol("▶ ")
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        );
    let selected = (!app.reflog_entries.is_empty())
        .then_some(app.selection.selected_reflog_index)
        .flatten();
    frame.render_stateful_widget(list, left, &mut list_state(selected));

    let lines = app.selected_reflog().map_or_else(
        || {
            vec![Line::styled(
                "No reflog entry selected",
                Style::default().fg(Color::DarkGray),
            )]
        },
        |entry| {
            vec![
                Line::from(vec![
                    Span::styled("Selector: ", Style::default().fg(Color::Cyan)),
                    Span::raw(terminal_safe(&entry.selector)),
                ]),
                Line::from(vec![
                    Span::styled("Commit:   ", Style::default().fg(Color::Cyan)),
                    Span::raw(terminal_safe(&entry.hash.0)),
                ]),
                Line::from(vec![
                    Span::styled("Action:   ", Style::default().fg(Color::Cyan)),
                    Span::raw(terminal_safe(&entry.action)),
                ]),
                Line::from(vec![
                    Span::styled("Author:   ", Style::default().fg(Color::Cyan)),
                    Span::raw(terminal_safe(&entry.author)),
                ]),
                Line::from(vec![
                    Span::styled("Commit date: ", Style::default().fg(Color::Cyan)),
                    Span::raw(terminal_safe(&entry.authored_at)),
                ]),
                Line::raw(""),
                Line::styled(
                    terminal_safe(&entry.message),
                    Style::default().fg(Color::White),
                ),
            ]
        },
    );
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel_block("Reflog entry", false))
            .wrap(Wrap { trim: false }),
        right,
    );
}

fn change_status_style(change: &crate::domain::WorkingTreeChange, group: ChangeGroup) -> Style {
    if change.is_conflicted() {
        Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD)
    } else if group == ChangeGroup::Staged {
        Style::default().fg(Color::Green)
    } else if change.is_untracked() {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::Yellow)
    }
}

fn change_marker(change: &crate::domain::WorkingTreeChange, group: ChangeGroup) -> char {
    if change.is_conflicted() {
        'U'
    } else if change.is_untracked() {
        '?'
    } else {
        match group {
            ChangeGroup::Staged => change.index_status,
            ChangeGroup::Unstaged => change.worktree_status,
        }
    }
}

fn change_checkbox(selected: usize, total: usize) -> &'static str {
    if selected == 0 || total == 0 {
        "[ ]"
    } else if selected == total {
        "[x]"
    } else {
        "[-]"
    }
}

fn render_changes(frame: &mut Frame<'_>, app: &AppState, area: Rect, terminal_width: u16) {
    let [left, right] = columns(area);
    let nodes = app.visible_changes_nodes();
    let staged_count = app.change_group_count(ChangeGroup::Staged);
    let unstaged_count = app.change_group_count(ChangeGroup::Unstaged);
    let selected_staged = app.selected_change_count(ChangeGroup::Staged);
    let selected_unstaged = app.selected_change_count(ChangeGroup::Unstaged);
    let items = nodes
        .iter()
        .enumerate()
        .map(|(position, node)| match *node {
            ChangesTreeNode::Root => ListItem::new(Line::from(vec![
                Span::styled(
                    if app.expansion.changes_root_expanded {
                        "▼ "
                    } else {
                        "▶ "
                    },
                    Style::default().fg(Color::Cyan),
                ),
                Span::styled(
                    format!(
                        "{} ",
                        change_checkbox(
                            selected_staged + selected_unstaged,
                            staged_count + unstaged_count
                        )
                    ),
                    Style::default().fg(Color::Green),
                ),
                Span::styled(
                    "Changes",
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("  {staged_count} staged · {unstaged_count} unstaged"),
                    Style::default().fg(Color::DarkGray),
                ),
            ])),
            ChangesTreeNode::Group(group) => {
                let (connector, expanded, count, selected, style) = match group {
                    ChangeGroup::Staged => (
                        "  ├─",
                        app.expansion.staged_changes_expanded,
                        staged_count,
                        selected_staged,
                        Style::default().fg(Color::Green),
                    ),
                    ChangeGroup::Unstaged => (
                        "  └─",
                        app.expansion.unstaged_changes_expanded,
                        unstaged_count,
                        selected_unstaged,
                        Style::default().fg(Color::Yellow),
                    ),
                };
                ListItem::new(Line::from(vec![
                    Span::styled(connector, Style::default().fg(Color::DarkGray)),
                    Span::styled(if expanded { "▼ " } else { "▶ " }, style),
                    Span::styled(
                        format!("{} ", change_checkbox(selected, count)),
                        Style::default().fg(Color::Green),
                    ),
                    Span::styled(group.title(), style.add_modifier(Modifier::BOLD)),
                    Span::styled(format!(" ({count})"), Style::default().fg(Color::DarkGray)),
                ]))
            }
            ChangesTreeNode::File {
                group,
                change_index,
            } => {
                let change = &app.changes[change_index];
                let is_last = !nodes.iter().skip(position + 1).any(|candidate| {
                    matches!(candidate, ChangesTreeNode::File { group: candidate_group, .. } if *candidate_group == group)
                });
                let prefix = match group {
                    ChangeGroup::Staged => {
                        format!("  │  {} ", if is_last { "└─" } else { "├─" })
                    }
                    ChangeGroup::Unstaged => {
                        format!("     {} ", if is_last { "└─" } else { "├─" })
                    }
                };
                let path = change.old_path.as_ref().map_or_else(
                    || terminal_safe(change.path.as_str()),
                    |old_path| {
                        terminal_safe(&format!("{} → {}", old_path.as_str(), change.path.as_str()))
                    },
                );
                ListItem::new(Line::from(vec![
                    Span::styled(prefix, Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        if app.is_change_selected(group, change) {
                            "[x] "
                        } else {
                            "[ ] "
                        },
                        Style::default().fg(Color::Green),
                    ),
                    Span::styled(
                        format!("{} ", change_marker(change, group)),
                        change_status_style(change, group),
                    ),
                    Span::raw(path),
                    Span::styled(
                        if change.is_conflicted() {
                            "  conflict"
                        } else if change.is_untracked() {
                            "  untracked"
                        } else {
                            ""
                        },
                        change_status_style(change, group),
                    ),
                ]))
            }
        })
        .collect::<Vec<_>>();
    let list = List::new(items)
        .block(panel_block(
            format!(
                "Changes  S:{staged_count} U:{unstaged_count} · {} selected",
                app.change_selection.len()
            ),
            app.focus == FocusPanel::ChangesTree,
        ))
        .highlight_symbol("▶ ")
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        );
    frame.render_stateful_widget(
        list,
        left,
        &mut list_state(app.selection.selected_changes_index),
    );

    let (title, empty_message, selected_group) = match app.selected_changes_node() {
        Some(ChangesTreeNode::Root) => (
            "Changes overview".to_string(),
            format!(
                "{staged_count} staged and {unstaged_count} unstaged file entries. Expand a group and select a file to inspect its diff."
            ),
            None,
        ),
        Some(ChangesTreeNode::Group(group)) => (
            group.title().to_string(),
            format!(
                "{} file entries. Expand this group and select a file to inspect its diff.",
                app.change_group_count(group)
            ),
            None,
        ),
        Some(ChangesTreeNode::File {
            group,
            change_index,
        }) => (
            format!("{} — {}", group.title(), app.changes[change_index].path),
            "Loading selected file diff…".to_string(),
            Some(group),
        ),
        None => (
            "Changes overview".to_string(),
            "No changes available".to_string(),
            None,
        ),
    };
    let diff = selected_group
        .filter(|group| app.current_changes_diff_group == Some(*group))
        .and(app.current_changes_diff.as_ref());
    render_diff_panel(
        frame,
        diff,
        app.diff_mode,
        app.wrap_diff,
        app.selection.changes_diff_scroll,
        app.focus == FocusPanel::ChangesDiff,
        right,
        terminal_width,
        &title,
        &empty_message,
    );
}

fn render_branches(frame: &mut Frame<'_>, app: &AppState, area: Rect) {
    let nodes = app.visible_tree_nodes();
    let items = if nodes.is_empty() {
        vec![ListItem::new(Line::styled(
            "No repositories",
            Style::default().fg(Color::DarkGray),
        ))]
    } else {
        nodes
            .iter()
            .enumerate()
            .map(|(node_position, node)| match *node {
                BranchTreeNode::Repository { repository_index } => {
                    let repository = &app.repositories[repository_index];
                    let active = app.active_repository_index == Some(repository_index);
                    let disclosure = if repository.expanded { "▼" } else { "▶" };
                    let state_marker = if repository.last_error.is_some() {
                        " !"
                    } else if repository.latest_status_job.is_some()
                        || repository.latest_branches_job.is_some()
                    {
                        " …"
                    } else {
                        ""
                    };
                    ListItem::new(Line::from(vec![
                        Span::styled(
                            format!("{disclosure} {} ", if active { "●" } else { "○" }),
                            Style::default().fg(if active {
                                Color::Green
                            } else {
                                Color::DarkGray
                            }),
                        ),
                        Span::styled(
                            terminal_safe(&repository.display_name()),
                            Style::default()
                                .fg(if active {
                                    Color::Yellow
                                } else {
                                    Color::Magenta
                                })
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            terminal_safe(&format!(
                                "  {}{state_marker}",
                                repository.display_path().display()
                            )),
                            Style::default().fg(if repository.last_error.is_some() {
                                Color::Red
                            } else {
                                Color::DarkGray
                            }),
                        ),
                    ]))
                }
                BranchTreeNode::Branch {
                    repository_index,
                    branch_index,
                } => {
                    let branch = &app.repositories[repository_index].branches[branch_index];
                    let is_last_child = !matches!(
                        nodes.get(node_position + 1),
                        Some(BranchTreeNode::Branch {
                            repository_index: next_repository,
                            ..
                        }) if *next_repository == repository_index
                    );
                    let style = if matches!(branch.kind, crate::domain::BranchKind::Remote) {
                        Style::default().fg(Color::Cyan)
                    } else {
                        Style::default()
                    };
                    ListItem::new(Line::from(vec![
                        Span::styled(
                            format!(
                                "  {} {} ",
                                if is_last_child { "└─" } else { "├─" },
                                if branch.is_current { "*" } else { " " }
                            ),
                            Style::default().fg(Color::Green),
                        ),
                        Span::styled(terminal_safe(&branch.name.0), style),
                        Span::styled(
                            terminal_safe(&format!("  {}", branch.short_head)),
                            Style::default().fg(Color::DarkGray),
                        ),
                    ]))
                }
            })
            .collect()
    };
    let title = if app.effective_branch_filter().is_empty() {
        "Repositories / Branches".to_string()
    } else {
        terminal_safe(&format!(
            "Repositories / Branches /{}",
            app.effective_branch_filter()
        ))
    };
    let list = List::new(items)
        .block(panel_block(title, app.focus == FocusPanel::BranchList))
        .highlight_symbol("▶ ")
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        );
    let selected = (!nodes.is_empty())
        .then_some(app.selection.selected_branch_index)
        .flatten();
    frame.render_stateful_widget(list, area, &mut list_state(selected));
}

fn commit_items(app: &AppState) -> (Vec<ListItem<'static>>, bool) {
    let commits = app.visible_commits();
    if commits.is_empty() {
        return (
            vec![ListItem::new(Line::styled(
                "No commits",
                Style::default().fg(Color::DarkGray),
            ))],
            true,
        );
    }
    let items = commits
        .iter()
        .map(|commit| {
            let queued = app.cherry_pick_queue.contains(&commit.hash);
            let copy_selected = app.commit_copy_selection.contains(&commit.hash);
            ListItem::new(Line::from(vec![
                Span::styled(
                    if queued { "● " } else { "  " },
                    Style::default().fg(Color::Magenta),
                ),
                Span::styled(
                    if copy_selected { "✓ " } else { "  " },
                    Style::default().fg(Color::Green),
                ),
                Span::styled(
                    terminal_safe(&commit.short_hash),
                    Style::default().fg(Color::Yellow),
                ),
                Span::raw(" "),
                Span::raw(terminal_safe(&commit.subject)),
                Span::styled(
                    if commit.decorations.is_empty() {
                        String::new()
                    } else {
                        terminal_safe(&format!("  {}", commit.decorations))
                    },
                    Style::default().fg(Color::Cyan),
                ),
            ]))
        })
        .collect();
    (items, false)
}

fn render_commits(frame: &mut Frame<'_>, app: &AppState, area: Rect) {
    let (items, empty) = commit_items(app);
    let selection_count = app.commit_copy_selection.len();
    let title = if app.effective_commit_filter().is_empty() {
        format!("Commits · {selection_count} selected")
    } else {
        terminal_safe(&format!(
            "Commits /{} · {selection_count} selected",
            app.effective_commit_filter()
        ))
    };
    let list = List::new(items)
        .block(panel_block(title, app.focus == FocusPanel::CommitList))
        .highlight_symbol("▶ ")
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        );
    let selected = (!empty)
        .then_some(app.selection.selected_commit_index)
        .flatten();
    frame.render_stateful_widget(list, area, &mut list_state(selected));
}

fn render_commit_detail(frame: &mut Frame<'_>, app: &AppState, area: Rect) {
    let [left, right] = columns(area);
    render_commits(frame, app, left);
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(8), Constraint::Min(3)])
        .split(right);
    render_commit_metadata(frame, app, sections[0]);
    render_changed_files(frame, app, sections[1]);
}

fn render_commit_metadata(frame: &mut Frame<'_>, app: &AppState, area: Rect) {
    let lines = app.current_commit_detail.as_ref().map_or_else(
        || {
            vec![Line::styled(
                "No commit selected",
                Style::default().fg(Color::DarkGray),
            )]
        },
        |detail| {
            vec![
                Line::from(vec![
                    Span::styled("Commit: ", Style::default().fg(Color::Cyan)),
                    Span::raw(terminal_safe(&detail.commit.hash.0)),
                ]),
                Line::from(vec![
                    Span::styled("Author: ", Style::default().fg(Color::Cyan)),
                    Span::raw(terminal_safe(&format!(
                        "{} <{}>",
                        detail.commit.author, detail.author_email
                    ))),
                ]),
                Line::from(vec![
                    Span::styled("Date:   ", Style::default().fg(Color::Cyan)),
                    Span::raw(terminal_safe(&detail.commit.authored_at)),
                ]),
                Line::from(vec![
                    Span::styled("Message:", Style::default().fg(Color::Cyan)),
                    Span::raw(terminal_safe(&format!(" {}", detail.commit.subject))),
                ]),
                Line::styled(
                    terminal_safe(detail.message.lines().nth(1).unwrap_or_default()),
                    Style::default().fg(Color::Gray),
                ),
            ]
        },
    );
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel_block("Commit", false))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_changed_files(frame: &mut Frame<'_>, app: &AppState, area: Rect) {
    let files = app
        .current_commit_detail
        .as_ref()
        .map(|detail| detail.files.as_slice())
        .unwrap_or_default();
    let items = if files.is_empty() {
        vec![ListItem::new(Line::styled(
            "No changed files",
            Style::default().fg(Color::DarkGray),
        ))]
    } else {
        files
            .iter()
            .map(|file| {
                let expanded = app.expansion.expanded_files.contains(&file.path);
                let additions = file
                    .additions
                    .map_or_else(|| "-".into(), |value| value.to_string());
                let deletions = file
                    .deletions
                    .map_or_else(|| "-".into(), |value| value.to_string());
                let mut lines = vec![Line::from(vec![
                    Span::styled(
                        if expanded { "▼ " } else { "▶ " },
                        Style::default().fg(Color::Cyan),
                    ),
                    Span::styled(
                        format!("{} ", file.kind.marker()),
                        file_kind_style(file.kind.marker()),
                    ),
                    Span::raw(terminal_safe(file.path.as_str())),
                    Span::styled(
                        format!("  +{additions} -{deletions}"),
                        Style::default().fg(Color::DarkGray),
                    ),
                ])];
                if expanded {
                    if file.hunks.is_empty() {
                        lines.push(Line::styled(
                            "    (no textual hunks)",
                            Style::default().fg(Color::DarkGray),
                        ));
                    } else {
                        lines.extend(file.hunks.iter().map(|hunk| {
                            Line::styled(
                                terminal_safe(&format!(
                                    "    {}  +{} -{}",
                                    hunk.header, hunk.additions, hunk.deletions
                                )),
                                Style::default().fg(Color::Cyan),
                            )
                        }));
                    }
                }
                ListItem::new(lines)
            })
            .collect()
    };
    let list = List::new(items)
        .block(panel_block(
            "Files changed in commit",
            app.focus == FocusPanel::CommitFileList,
        ))
        .highlight_symbol("▶ ")
        .highlight_style(Style::default().bg(Color::DarkGray).fg(Color::White));
    let selected = (!files.is_empty())
        .then_some(app.selection.selected_file_index)
        .flatten();
    frame.render_stateful_widget(list, area, &mut list_state(selected));
}

fn file_kind_style(marker: &str) -> Style {
    match marker {
        "A" => Style::default().fg(Color::Green),
        "D" => Style::default().fg(Color::Red),
        "R" | "C" => Style::default().fg(Color::Cyan),
        "U" => Style::default().fg(Color::Magenta),
        _ => Style::default().fg(Color::Yellow),
    }
}

fn render_file_diff(frame: &mut Frame<'_>, app: &AppState, area: Rect, terminal_width: u16) {
    let [left, right] = columns(area);
    render_file_list(frame, app, left);
    render_diff_panel(
        frame,
        app.current_file_diff.as_ref(),
        app.diff_mode,
        app.wrap_diff,
        app.selection.diff_scroll,
        app.focus == FocusPanel::DiffView,
        right,
        terminal_width,
        "Changes",
        "No diff loaded",
    );
}

fn render_file_list(frame: &mut Frame<'_>, app: &AppState, area: Rect) {
    let files = app
        .current_commit_detail
        .as_ref()
        .map(|detail| detail.files.as_slice())
        .unwrap_or_default();
    let items = files
        .iter()
        .map(|file| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{} ", file.kind.marker()),
                    file_kind_style(file.kind.marker()),
                ),
                Span::raw(terminal_safe(file.path.as_str())),
            ]))
        })
        .collect::<Vec<_>>();
    let list = List::new(items)
        .block(panel_block("Files", app.focus == FocusPanel::FileList))
        .highlight_symbol("▶ ")
        .highlight_style(Style::default().bg(Color::DarkGray).fg(Color::White));
    frame.render_stateful_widget(
        list,
        area,
        &mut list_state(app.selection.selected_file_index),
    );
}

#[allow(clippy::too_many_arguments)]
fn render_diff_panel(
    frame: &mut Frame<'_>,
    diff: Option<&FileDiff>,
    mode: DiffViewMode,
    wrap: bool,
    scroll: u16,
    focused: bool,
    area: Rect,
    terminal_width: u16,
    base_title: &str,
    empty_message: &str,
) {
    let side_by_side = mode == DiffViewMode::SideBySide && terminal_width >= 140;
    let mode_name = if side_by_side {
        "side-by-side"
    } else {
        "unified"
    };
    let title = format!(
        "{base_title} ({mode_name}{})",
        if wrap { ", wrap" } else { "" }
    );
    let text = if side_by_side {
        side_by_side_text(diff, area.width, empty_message)
    } else {
        unified_text(
            diff,
            terminal_width < 140 && mode == DiffViewMode::SideBySide,
            empty_message,
        )
    };
    let mut paragraph = Paragraph::new(text)
        .block(panel_block(title, focused))
        .scroll((scroll, 0));
    if wrap {
        paragraph = paragraph.wrap(Wrap { trim: false });
    }
    frame.render_widget(paragraph, area);
}

fn unified_text(
    diff: Option<&FileDiff>,
    narrow_warning: bool,
    empty_message: &str,
) -> Text<'static> {
    let Some(diff) = diff else {
        return Text::from(Line::styled(
            terminal_safe(empty_message),
            Style::default().fg(Color::DarkGray),
        ));
    };
    let mut lines = Vec::new();
    if narrow_warning {
        lines.push(Line::styled(
            "side-by-side requires terminal width >= 140; showing unified",
            Style::default().fg(Color::Yellow),
        ));
    }
    lines.extend(
        diff.header
            .iter()
            .map(|line| Line::styled(terminal_safe(line), Style::default().fg(Color::DarkGray))),
    );
    if diff.is_binary {
        lines.push(Line::styled(
            "Binary file changed; textual diff is unavailable",
            Style::default().fg(Color::Yellow),
        ));
    }
    for hunk in &diff.hunks {
        lines.push(Line::styled(
            terminal_safe(&hunk.header),
            Style::default().fg(Color::Cyan),
        ));
        for line in &hunk.lines {
            let (marker, style) = match line.kind {
                DiffLineKind::Context => (" ", Style::default()),
                DiffLineKind::Addition => ("+", Style::default().fg(Color::Green)),
                DiffLineKind::Deletion => ("-", Style::default().fg(Color::Red)),
                DiffLineKind::Metadata => ("\\", Style::default().fg(Color::DarkGray)),
            };
            let old = line
                .old_line_no
                .map_or_else(String::new, |value| value.to_string());
            let new = line
                .new_line_no
                .map_or_else(String::new, |value| value.to_string());
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{old:>5} {new:>5} "),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(terminal_safe(&format!("{marker}{}", line.text)), style),
            ]));
        }
    }
    if lines.is_empty() {
        lines.push(Line::styled(
            "No textual diff; the file may be empty or binary",
            Style::default().fg(Color::DarkGray),
        ));
    }
    Text::from(lines)
}

fn truncate_width(value: &str, maximum: usize) -> String {
    if UnicodeWidthStr::width(value) <= maximum {
        return value.to_string();
    }
    if maximum == 0 {
        return String::new();
    }
    let content_width = maximum - 1;
    let mut result = String::new();
    let mut used = 0;
    for character in value.chars() {
        let width = UnicodeWidthChar::width(character).unwrap_or(0);
        if used + width > content_width {
            break;
        }
        result.push(character);
        used += width;
    }
    result.push('…');
    result
}

fn cell_style(kind: DiffCellKind) -> Style {
    match kind {
        DiffCellKind::Empty | DiffCellKind::Context => Style::default(),
        DiffCellKind::Added => Style::default().fg(Color::Green),
        DiffCellKind::Deleted => Style::default().fg(Color::Red),
        DiffCellKind::Modified => Style::default().fg(Color::Yellow),
    }
}

fn side_by_side_text(diff: Option<&FileDiff>, width: u16, empty_message: &str) -> Text<'static> {
    let Some(diff) = diff else {
        return Text::from(Line::styled(
            terminal_safe(empty_message),
            Style::default().fg(Color::DarkGray),
        ));
    };
    let mut lines = diff
        .header
        .iter()
        .map(|line| Line::styled(terminal_safe(line), Style::default().fg(Color::DarkGray)))
        .collect::<Vec<_>>();
    if diff.is_binary {
        lines.push(Line::styled(
            "Binary file changed; textual diff is unavailable",
            Style::default().fg(Color::Yellow),
        ));
    }
    let cell_width = width.saturating_sub(17) as usize / 2;
    for hunk in &diff.hunks {
        lines.push(Line::styled(
            terminal_safe(&hunk.header),
            Style::default().fg(Color::Cyan),
        ));
        for row in side_by_side_rows(hunk) {
            let left_no = row
                .left_line_no
                .map_or_else(String::new, |value| value.to_string());
            let right_no = row
                .right_line_no
                .map_or_else(String::new, |value| value.to_string());
            let left = truncate_width(
                &terminal_safe(row.left_text.as_deref().unwrap_or("")),
                cell_width,
            );
            let right = truncate_width(
                &terminal_safe(row.right_text.as_deref().unwrap_or("")),
                cell_width,
            );
            let left_padding = cell_width.saturating_sub(UnicodeWidthStr::width(left.as_str()));
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{left_no:>5} "),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    format!("{left}{}", " ".repeat(left_padding)),
                    cell_style(row.left_kind),
                ),
                Span::styled(" │ ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{right_no:>5} "),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(right, cell_style(row.right_kind)),
            ]));
        }
    }
    if lines.is_empty() {
        lines.push(Line::styled(
            "No textual diff; the file may be empty or binary",
            Style::default().fg(Color::DarkGray),
        ));
    }
    Text::from(lines)
}

fn render_hotkeys(frame: &mut Frame<'_>, app: &AppState, area: Rect) {
    let mut text = match &app.mode {
        GlobalMode::Filtering { target, query } => format!(
            " /{}: {}_ | Enter apply | Esc cancel ",
            match target {
                FilterTarget::Branches => "branches",
                FilterTarget::Commits => "commits",
            },
            query
        ),
        GlobalMode::Confirming {
            dialog: ConfirmDialog::ResetModeChoice { .. },
        } => " s soft | m mixed | h hard | Esc cancel ".into(),
        GlobalMode::Confirming {
            dialog: ConfirmDialog::HardResetWarning { .. },
        } => " Enter continue to hash confirmation | Esc cancel ".into(),
        GlobalMode::Confirming { .. } => " Enter confirm | Esc cancel ".into(),
        GlobalMode::TypingConfirmation { .. } => {
            " Type short hash | Enter final confirm | Esc cancel ".into()
        }
        GlobalMode::EditingCommitMessage { .. } => {
            " Type commit message | Enter create commit | Esc cancel ".into()
        }
        GlobalMode::Error => " Enter / Esc dismiss ".into(),
        GlobalMode::Normal => match (app.screen, app.focus) {
            (Screen::BranchOverview, FocusPanel::BranchList) => {
                let mut keys = vec!["Tab focus"];
                match app.selected_tree_node() {
                    Some(BranchTreeNode::Repository { .. }) => {
                        keys.extend(["Enter expand/collapse", "f fetch", "g reflog"]);
                    }
                    Some(BranchTreeNode::Branch { .. }) => {
                        keys.extend(["Enter view commits", "s switch", "b rebase"]);
                    }
                    None => {}
                }
                keys.extend(["/ filter", "r refresh", "q quit"]);
                format!(" {} ", keys.join(" | "))
            }
            (Screen::BranchOverview, FocusPanel::CommitList) => {
                let mut keys = vec!["Tab focus"];
                if app.selected_commit().is_some() {
                    keys.extend([
                        "Enter detail",
                        "Space select",
                        "c copy hashes",
                        "i copy info",
                        "y queue",
                        "R reset",
                    ]);
                }
                if !app.cherry_pick_queue.is_empty() {
                    keys.push("Y cherry-pick");
                }
                keys.extend(["/ filter", "q quit"]);
                format!(" {} ", keys.join(" | "))
            }
            (Screen::CommitDetail, FocusPanel::CommitList) => {
                let mut keys = Vec::new();
                if app.selected_commit().is_some() {
                    keys.extend([
                        "Enter detail",
                        "Space select",
                        "c copy hashes",
                        "i copy info",
                        "y queue",
                        "R reset",
                    ]);
                }
                if !app.cherry_pick_queue.is_empty() {
                    keys.push("Y cherry-pick");
                }
                keys.extend(["/ filter", "Esc back"]);
                format!(" {} ", keys.join(" | "))
            }
            (Screen::CommitDetail, FocusPanel::CommitFileList) => {
                let mut keys = Vec::new();
                if app.selected_file().is_some() {
                    keys.extend(["Space expand", "Enter file diff"]);
                }
                if app.selected_commit().is_some() {
                    keys.extend(["i copy commit info", "y queue"]);
                }
                keys.extend(["Tab focus", "Esc back", "q quit"]);
                format!(" {} ", keys.join(" | "))
            }
            (Screen::FileDiffDetail, _) => {
                let mut keys = vec!["v mode", "Ctrl+C hash", "Ctrl+Shift+C info"];
                if app.selected_file().is_some() {
                    keys.extend(["n next", "p prev"]);
                }
                keys.extend(["w wrap", "Tab focus", "Esc back", "q quit"]);
                format!(" {} ", keys.join(" | "))
            }
            (Screen::Reflog, FocusPanel::ReflogList) => {
                let mut keys = Vec::new();
                if app.selected_reflog().is_some() {
                    keys.push("R reset");
                }
                keys.extend(["Esc back", "q quit"]);
                format!(" {} ", keys.join(" | "))
            }
            (Screen::Changes, FocusPanel::ChangesTree) => {
                let mut keys = Vec::new();
                match app.selected_changes_node() {
                    Some(ChangesTreeNode::File { .. }) => keys.push("Enter open diff"),
                    Some(_) => keys.push("Enter expand/collapse"),
                    None => {}
                }
                keys.extend([
                    "Space select",
                    "s stage",
                    "u unstage",
                    "c commit",
                    "←/→ collapse/expand",
                    "v diff mode",
                    "w wrap",
                    "Tab focus",
                    "r refresh",
                    "Esc back",
                    "q quit",
                ]);
                format!(" {} ", keys.join(" | "))
            }
            (Screen::Changes, FocusPanel::ChangesDiff) => {
                " Space select file | s stage | u unstage | c commit | ↑/↓ scroll | v mode | w wrap | Tab tree | r refresh | Esc back "
                    .into()
            }
            _ => " q quit ".into(),
        },
    };
    if matches!(app.mode, GlobalMode::Normal) {
        text = format!(" Ctrl+G changes | {} ", text.trim());
    }
    if let Some(message) = app.last_message.as_ref() {
        text.push_str(&format!(" | ✓ {message} "));
    }
    frame.render_widget(
        Paragraph::new(terminal_safe(&text))
            .style(Style::default().bg(Color::DarkGray).fg(Color::White)),
        area,
    );
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

fn render_popup(frame: &mut Frame<'_>, app: &AppState, area: Rect) {
    let (title, lines, height) = match &app.mode {
        GlobalMode::Confirming { dialog } => match dialog {
            ConfirmDialog::FetchRepository { repository_index } => {
                let repository = app.repositories.get(*repository_index);
                (
                    "Fetch repository",
                    vec![
                        Line::raw("Repository:"),
                        Line::styled(
                            terminal_safe(&repository.map_or_else(
                                || "—".to_string(),
                                |repository| {
                                    format!(
                                        "{}  {}",
                                        repository.display_name(),
                                        repository.display_path().display()
                                    )
                                },
                            )),
                            Style::default().fg(Color::Cyan),
                        ),
                        Line::raw(""),
                        Line::raw("About to run:"),
                        Line::styled(
                            "git fetch --all --prune",
                            Style::default().fg(Color::Yellow),
                        ),
                        Line::raw(""),
                        Line::raw("Enter confirm | Esc cancel"),
                    ],
                    46,
                )
            }
            ConfirmDialog::SwitchBranch {
                repository_index,
                branch,
            } => {
                let status = app
                    .repository(*repository_index)
                    .map(|repository| &repository.status);
                (
                    "Switch branch",
                    vec![
                        Line::raw("About to run:"),
                        Line::styled(
                            terminal_safe(&format!("git switch {}", branch.0)),
                            Style::default().fg(Color::Yellow),
                        ),
                        Line::raw(""),
                        Line::raw(format!(
                            "Working tree: staged={} modified={} untracked={} conflicted={}",
                            status.map_or(0, |status| status.staged),
                            status.map_or(0, |status| status.modified),
                            status.map_or(0, |status| status.untracked),
                            status.map_or(0, |status| status.conflicted)
                        )),
                        Line::raw(""),
                        Line::raw("Enter confirm | Esc cancel"),
                    ],
                    42,
                )
            }
            ConfirmDialog::CherryPickQueue { commits, .. } => {
                let mut lines = vec![
                    Line::raw("About to run:"),
                    Line::styled(
                        format!(
                            "git cherry-pick {}",
                            commits
                                .iter()
                                .map(|commit| commit.short())
                                .collect::<Vec<_>>()
                                .join(" ")
                        ),
                        Style::default().fg(Color::Yellow),
                    ),
                    Line::raw(""),
                    Line::raw("Queue:"),
                ];
                for (index, commit) in commits.iter().enumerate() {
                    let subject = app
                        .branch_commits
                        .items
                        .iter()
                        .find(|item| item.hash == *commit)
                        .map_or("", |item| item.subject.as_str());
                    lines.push(Line::raw(terminal_safe(&format!(
                        "{}. {} {}",
                        index + 1,
                        commit.short(),
                        subject
                    ))));
                }
                lines.extend([Line::raw(""), Line::raw("Enter confirm | Esc cancel")]);
                ("Cherry-pick queue", lines, 60)
            }
            ConfirmDialog::ResetModeChoice { commit, .. } => (
                "Choose reset mode",
                vec![
                    Line::raw(terminal_safe(&format!(
                        "Target: {} ({})",
                        commit.short(),
                        commit.0
                    ))),
                    Line::raw(""),
                    Line::styled("s  --soft", Style::default().fg(Color::Green)),
                    Line::raw("   Move HEAD; keep index and working tree."),
                    Line::styled("m  --mixed", Style::default().fg(Color::Yellow)),
                    Line::raw("   Move HEAD and reset index; keep working tree."),
                    Line::styled("h  --hard", Style::default().fg(Color::Red)),
                    Line::raw("   Move HEAD and discard index/working tree changes."),
                    Line::raw(""),
                    Line::raw("s / m / h choose | Esc cancel"),
                ],
                56,
            ),
            ConfirmDialog::Reset { commit, mode, .. } => (
                "Confirm reset",
                vec![
                    Line::raw("About to run:"),
                    Line::styled(
                        format!("git reset {} {}", mode.flag(), commit.short()),
                        Style::default().fg(Color::Yellow),
                    ),
                    Line::raw(""),
                    Line::raw(if *mode == crate::git::ResetMode::Soft {
                        "Index and working tree will be preserved."
                    } else {
                        "Working tree will be preserved; index will be reset."
                    }),
                    Line::raw(""),
                    Line::raw("Enter confirm | Esc cancel"),
                ],
                42,
            ),
            ConfirmDialog::HardResetWarning {
                commit, expected, ..
            } => (
                "Hard reset — confirmation 1/2",
                vec![
                    Line::raw("About to run:"),
                    Line::styled(
                        format!("git reset --hard {}", commit.short()),
                        Style::default().fg(Color::Red),
                    ),
                    Line::raw(""),
                    Line::styled(
                        "This permanently discards tracked index and working tree changes.",
                        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                    ),
                    Line::raw(terminal_safe(&format!(
                        "Enter continues to a second confirmation requiring {expected}."
                    ))),
                    Line::raw(""),
                    Line::raw("Enter continue | Esc cancel"),
                ],
                48,
            ),
            ConfirmDialog::Rebase {
                current_branch,
                upstream,
                ..
            } => (
                "Safe rebase",
                vec![
                    Line::raw("About to run:"),
                    Line::styled(
                        terminal_safe(&format!("git rebase {}", upstream.0)),
                        Style::default().fg(Color::Yellow),
                    ),
                    Line::raw(""),
                    Line::raw(terminal_safe(&format!(
                        "Current branch `{}` will be rebased onto `{}`.",
                        current_branch.0, upstream.0
                    ))),
                    Line::raw("The working tree must be clean."),
                    Line::raw(
                        "If conflicts occur, Pitui will automatically run git rebase --abort.",
                    ),
                    Line::raw(""),
                    Line::raw("Enter confirm | Esc cancel"),
                ],
                52,
            ),
        },
        GlobalMode::TypingConfirmation {
            dialog: ConfirmDialog::HardResetWarning { commit, .. },
            expected,
            input,
            validation_error,
        } => {
            let mut lines = vec![
                Line::raw("About to run:"),
                Line::styled(
                    format!("git reset --hard {}", commit.short()),
                    Style::default().fg(Color::Red),
                ),
                Line::raw(""),
                Line::styled(
                    "This will discard working tree changes.",
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ),
                Line::raw(terminal_safe(&format!(
                    "Confirmation 2/2 — type {expected} to confirm:"
                ))),
                Line::styled(
                    terminal_safe(&format!("> {input}_")),
                    Style::default().fg(Color::Yellow),
                ),
            ];
            if let Some(error) = validation_error {
                lines.push(Line::styled(
                    terminal_safe(error),
                    Style::default().fg(Color::Red),
                ));
            }
            lines.push(Line::raw("Esc cancel"));
            ("Hard reset — confirmation 2/2", lines, 50)
        }
        GlobalMode::EditingCommitMessage {
            input,
            validation_error,
        } => {
            let repository = app
                .changes_repository_index
                .and_then(|index| app.repositories.get(index))
                .map_or_else(|| "—".to_string(), |repository| repository.display_name());
            let mut lines = vec![
                Line::from(vec![
                    Span::styled("Repository: ", Style::default().fg(Color::Cyan)),
                    Span::raw(terminal_safe(&repository)),
                ]),
                Line::from(vec![
                    Span::styled("Staged files: ", Style::default().fg(Color::Cyan)),
                    Span::raw(app.change_group_count(ChangeGroup::Staged).to_string()),
                ]),
                Line::raw(""),
                Line::raw("Commit message:"),
                Line::styled(
                    terminal_safe(&format!("> {input}_")),
                    Style::default().fg(Color::Yellow),
                ),
                Line::raw(""),
                Line::styled(
                    "git commit -m <message>",
                    Style::default().fg(Color::DarkGray),
                ),
            ];
            if let Some(error) = validation_error {
                lines.push(Line::styled(
                    terminal_safe(error),
                    Style::default().fg(Color::Red),
                ));
            }
            lines.push(Line::raw("Enter create commit | Esc cancel"));
            ("Create commit", lines, 46)
        }
        GlobalMode::Error => {
            let Some(error) = app.last_error.as_ref() else {
                return;
            };
            (
                "Command failed",
                vec![
                    Line::styled(
                        terminal_safe(&error.command),
                        Style::default().fg(Color::Yellow),
                    ),
                    Line::raw(""),
                    Line::raw(terminal_safe(&error.message)),
                    Line::raw(""),
                    Line::raw("Enter / Esc dismiss"),
                ],
                50,
            )
        }
        _ => return,
    };

    let popup = centered_rect(72, height, area);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title(title)
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Yellow)),
            )
            .wrap(Wrap { trim: false }),
        popup,
    );
}

#[cfg(test)]
mod tests {
    use ratatui::{Terminal, backend::TestBackend};

    use super::*;
    use crate::domain::{
        ChangedFile, Commit, CommitDetail, CommitHash, DiffHunk, DiffLine, FileChangeKind,
        FileDiff, GitPath, ReflogEntry, Repository, WorkingTreeChange, WorkingTreeStatus,
    };

    fn buffer_text(terminal: &Terminal<TestBackend>) -> String {
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    }

    #[test]
    fn renders_empty_overview() {
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let state = AppState::default();
        terminal.draw(|frame| render(frame, &state)).unwrap();
        let rendered = buffer_text(&terminal);
        assert!(rendered.contains("Branches"));
        assert!(rendered.contains("Commits"));
        assert!(rendered.contains("q quit"));
    }

    #[test]
    fn renders_reflog_list_and_selected_entry() {
        let backend = TestBackend::new(120, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState {
            screen: Screen::Reflog,
            focus: FocusPanel::ReflogList,
            reflog_repository_index: Some(0),
            reflog_entries: vec![ReflogEntry {
                hash: CommitHash("0123456789abcdef".into()),
                short_hash: "0123456".into(),
                selector: "HEAD@{0}".into(),
                action: "commit".into(),
                message: "add reflog view".into(),
                author: "Ada".into(),
                authored_at: "2026-07-16T00:00:00Z".into(),
            }],
            ..AppState::default()
        };
        state.ensure_valid_reflog_selection();
        terminal.draw(|frame| render(frame, &state)).unwrap();
        let rendered = buffer_text(&terminal);
        assert!(rendered.contains("HEAD@{0}"));
        assert!(rendered.contains("add reflog view"));
        assert!(rendered.contains("Reflog entry"));
        assert!(rendered.contains("R reset"));
    }

    #[test]
    fn renders_three_level_changes_tree_and_reused_staged_diff() {
        let backend = TestBackend::new(140, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let path = GitPath::from("src/main.rs");
        let mut state = AppState {
            screen: Screen::Changes,
            focus: FocusPanel::ChangesTree,
            changes_repository_index: Some(0),
            changes: vec![WorkingTreeChange {
                index_status: 'M',
                worktree_status: ' ',
                path: path.clone(),
                old_path: None,
            }],
            current_changes_diff: Some(FileDiff {
                commit: CommitHash("INDEX".into()),
                path,
                old_path: None,
                header: Vec::new(),
                hunks: vec![DiffHunk {
                    header: "@@ -1 +1 @@".into(),
                    old_start: 1,
                    old_count: 1,
                    new_start: 1,
                    new_count: 1,
                    lines: vec![
                        DiffLine {
                            old_line_no: Some(1),
                            new_line_no: None,
                            kind: DiffLineKind::Deletion,
                            text: "old".into(),
                        },
                        DiffLine {
                            old_line_no: None,
                            new_line_no: Some(1),
                            kind: DiffLineKind::Addition,
                            text: "new".into(),
                        },
                    ],
                }],
                is_binary: false,
            }),
            current_changes_diff_group: Some(ChangeGroup::Staged),
            change_selection: std::collections::HashSet::from([crate::app::ChangeSelection {
                group: ChangeGroup::Staged,
                path: GitPath::from("src/main.rs"),
            }]),
            ..AppState::default()
        };
        state.ensure_valid_changes_selection();
        terminal.draw(|frame| render(frame, &state)).unwrap();
        let rendered = buffer_text(&terminal);
        assert!(rendered.contains("Changes  S:1 U:0"));
        assert!(rendered.contains("1 selected"));
        assert!(rendered.contains("[x]"));
        assert!(rendered.contains("Staged Changes (1)"));
        assert!(rendered.contains("Unstaged Changes (0)"));
        assert!(rendered.contains("src/main.rs"));
        assert!(rendered.contains("Staged Changes — src/main.rs"));
        assert!(rendered.contains("+new"));

        state.diff_mode = DiffViewMode::SideBySide;
        terminal.draw(|frame| render(frame, &state)).unwrap();
        let rendered = buffer_text(&terminal);
        assert!(rendered.contains("Staged Changes — src/main.rs (side-by-side)"));
        assert!(rendered.contains("old"));
        assert!(rendered.contains("new"));
    }

    #[test]
    fn renders_commit_message_editor_and_validation() {
        let backend = TestBackend::new(120, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let state = AppState {
            screen: Screen::Changes,
            focus: FocusPanel::Popup,
            changes: vec![WorkingTreeChange {
                index_status: 'A',
                worktree_status: ' ',
                path: GitPath::from("new.txt"),
                old_path: None,
            }],
            mode: GlobalMode::EditingCommitMessage {
                input: "add selected files".into(),
                validation_error: Some("example validation".into()),
            },
            ..AppState::default()
        };
        terminal.draw(|frame| render(frame, &state)).unwrap();
        let rendered = buffer_text(&terminal);
        assert!(rendered.contains("Create commit"));
        assert!(rendered.contains("Staged files: 1"));
        assert!(rendered.contains("> add selected files_"));
        assert!(rendered.contains("example validation"));
        assert!(rendered.contains("Enter create commit"));
    }

    #[test]
    fn renders_repository_and_unborn_branch_on_separate_tree_levels() {
        let backend = TestBackend::new(140, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::with_repository_paths(vec!["/tmp/example".into()]);
        state.repositories[0].repository = Some(Repository {
            root: "/tmp/example".into(),
            name: "example".into(),
            current_branch: Some(crate::domain::BranchName("main".into())),
            head: CommitHash(String::new()),
            status: WorkingTreeStatus::default(),
        });
        state.repositories[0].ensure_current_branch_visible();
        terminal.draw(|frame| render(frame, &state)).unwrap();
        let rendered = buffer_text(&terminal);
        assert!(rendered.contains("example"));
        assert!(rendered.contains("└─ * main"));
        assert!(rendered.contains("unborn"));
    }

    #[test]
    fn renders_safely_at_tiny_terminal_sizes() {
        for (width, height) in [(1, 1), (20, 3), (40, 5), (80, 10)] {
            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).unwrap();
            let state = AppState::default();
            terminal.draw(|frame| render(frame, &state)).unwrap();
        }
    }

    #[test]
    fn renders_wide_side_by_side_diff() {
        let commit = Commit {
            hash: CommitHash("0123456789abcdef".into()),
            short_hash: "0123456".into(),
            author: "Test".into(),
            authored_at: "2026-07-16T00:00:00Z".into(),
            decorations: String::new(),
            subject: "change".into(),
        };
        let path = GitPath::from("file.rs");
        let file = ChangedFile {
            kind: FileChangeKind::Modified,
            path: path.clone(),
            old_path: None,
            additions: Some(1),
            deletions: Some(1),
            hunks: Vec::new(),
            is_binary: false,
        };
        let mut state = AppState {
            screen: Screen::FileDiffDetail,
            focus: FocusPanel::DiffView,
            diff_mode: DiffViewMode::SideBySide,
            current_commit_detail: Some(CommitDetail {
                commit: commit.clone(),
                author_email: "test@example.invalid".into(),
                committer: "Test".into(),
                committer_email: "test@example.invalid".into(),
                committed_at: "2026-07-16T00:00:00Z".into(),
                message: "change".into(),
                files: vec![file],
            }),
            current_file_diff: Some(FileDiff {
                commit: commit.hash,
                path,
                old_path: None,
                header: vec!["diff --git a/file.rs b/file.rs".into()],
                hunks: vec![DiffHunk {
                    header: "@@ -1 +1 @@".into(),
                    old_start: 1,
                    old_count: 1,
                    new_start: 1,
                    new_count: 1,
                    lines: vec![
                        DiffLine {
                            old_line_no: Some(1),
                            new_line_no: None,
                            kind: DiffLineKind::Deletion,
                            text: "old".into(),
                        },
                        DiffLine {
                            old_line_no: None,
                            new_line_no: Some(1),
                            kind: DiffLineKind::Addition,
                            text: "new".into(),
                        },
                    ],
                }],
                is_binary: false,
            }),
            ..AppState::default()
        };
        state.selection.selected_file_index = Some(0);

        let backend = TestBackend::new(160, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &state)).unwrap();
        let rendered = buffer_text(&terminal);
        assert!(rendered.contains("side-by-side"));
        assert!(rendered.contains("old"));
        assert!(rendered.contains("new"));
    }

    #[test]
    fn sanitizes_terminal_control_and_bidi_sequences() {
        let safe = terminal_safe("before\x1b[31m\tafter\u{202e}");
        assert!(!safe.contains('\x1b'));
        assert!(!safe.contains('\u{202e}'));
        assert!(safe.contains("    after"));
    }
}
