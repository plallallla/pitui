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
        FilterTarget, GlobalMode, PROMPT_OPERATION_SPECS, PanelId, RemoteEditKind,
        RemoteInputField, RepositoryId, Resource, ViewId, modal_shortcut_set,
    },
    config::{CommitListDensityConfig, FooterMode, FooterOverflow},
    domain::{DiffCellKind, DiffLineKind, FileDiff, side_by_side_rows},
};

const FOCUSED_BORDER: Color = Color::Yellow;
const NORMAL_BORDER: Color = Color::DarkGray;

fn resource_message<T>(resource: Option<&Resource<T>>, loading: &str, empty: &str) -> String {
    match resource {
        Some(Resource::Loading) => loading.into(),
        Some(Resource::Failed(error)) => format!("Load failed: {error}"),
        Some(Resource::NotLoaded) => "Not loaded".into(),
        Some(Resource::Ready(_)) | None => empty.into(),
    }
}

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
    let footer_height = match app.mode {
        GlobalMode::Normal | GlobalMode::Chord { .. } => {
            if app.config.footer.mode != FooterMode::Hidden {
                app.config.footer.max_rows
            } else {
                0
            }
        }
        _ => 1,
    };
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(footer_height),
        ])
        .split(area);

    render_status_bar(frame, app, rows[0]);
    match app.view_projection().view {
        ViewId::History => render_branch_overview(frame, app, rows[1]),
        ViewId::Commit => render_commit_detail(frame, app, rows[1]),
        ViewId::FileDiff => render_file_diff(frame, app, rows[1], area.width),
        ViewId::Reflog => render_reflog(frame, app, rows[1]),
        ViewId::Changes => render_changes(frame, app, rows[1], area.width),
        ViewId::Remotes => render_remotes(frame, app, rows[1]),
    }
    if footer_height > 0 {
        render_hotkeys(frame, app, rows[2]);
    }
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
    let view = app.view_projection().view;
    let selected_commit = match view {
        ViewId::Reflog => app
            .selected_reflog()
            .map_or("—", |entry| entry.short_hash.as_str()),
        ViewId::Changes | ViewId::Remotes => "—",
        _ => app
            .selected_commit()
            .map_or("—", |commit| commit.short_hash.as_str()),
    };
    let selected_file = match view {
        ViewId::Changes => app
            .selected_change()
            .map_or("—", |(_, change)| change.path.as_str()),
        ViewId::Remotes => "—",
        _ => app.selected_file().map_or("—", |file| file.path.as_str()),
    };
    let mode = match app.mode {
        GlobalMode::Normal => "NORMAL",
        GlobalMode::Filtering { .. } => "FILTER",
        GlobalMode::Confirming { .. } => "CONFIRM",
        GlobalMode::TypingConfirmation { .. } => "TYPE",
        GlobalMode::EditingCommitMessage { .. } => "COMMIT",
        GlobalMode::EditingRemote { .. } => "REMOTE",
        GlobalMode::Chord { .. } => "SHORTCUT",
        GlobalMode::CommandPrompt { .. } => "COMMAND",
        GlobalMode::ShortcutHelp { .. } => "HELP",
        GlobalMode::OperationPalette { .. } => "OPERATIONS",
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
    let line = if area.width < 100 {
        format!(
            " {repo} [{}/{}] | {branch} | {mode}{spinner} | S{} M{} U{} C{} | ↑{}↓{} ",
            app.active_repository_index.map_or(0, |index| index + 1),
            app.repository_count(),
            active_repository.map_or(0, |repo| repo.status.staged),
            active_repository.map_or(0, |repo| repo.status.modified),
            active_repository.map_or(0, |repo| repo.status.untracked),
            active_repository.map_or(0, |repo| repo.status.conflicted),
            active_repository.map_or(0, |repo| repo.status.ahead),
            active_repository.map_or(0, |repo| repo.status.behind),
        )
    } else if area.width < 140 {
        format!(
            " repo={repo} ({}/{}) | branch={branch} | op={mode}{spinner} | {counts} | {tracking} | selected={} ",
            app.active_repository_index.map_or(0, |index| index + 1),
            app.repository_count(),
            app.commit_selection.len()
        )
    } else {
        format!(
            " repo={repo} ({}/{}) | branch={branch} | head={head} | commit={selected_commit} | file={selected_file} | op={mode}{spinner} | {counts} | {tracking} | selected={} ",
            app.active_repository_index.map_or(0, |index| index + 1),
            app.repository_count(),
            app.commit_selection.len()
        )
    };
    frame.render_widget(
        Paragraph::new(terminal_safe(&line))
            .style(Style::default().bg(Color::Blue).fg(Color::White)),
        area,
    );
}

fn columns(area: Rect, app: &AppState) -> [Rect; 2] {
    let left = app
        .config
        .views
        .view(app.view_projection().view)
        .left_width_percent;
    let split = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(left),
            Constraint::Percentage(100 - left),
        ])
        .split(area);
    [split[0], split[1]]
}

fn list_state(selected: Option<usize>) -> ListState {
    let mut state = ListState::default();
    state.select(selected);
    state
}

fn render_branch_overview(frame: &mut Frame<'_>, app: &AppState, area: Rect) {
    let [left, right] = columns(area, app);
    render_branches(frame, app, left);
    render_commits(
        frame,
        app,
        right,
        app.config.views.view(ViewId::History).commit_density,
    );
}

fn render_reflog(frame: &mut Frame<'_>, app: &AppState, area: Rect) {
    let [left, right] = columns(area, app);
    let entries = app.reflog_entries();
    let items = if entries.is_empty() {
        let message = resource_message(
            app.reflog_repository_index
                .and_then(|index| app.model.repository(RepositoryId(index)))
                .map(|repository| &repository.reflog),
            "Loading reflog…",
            "No reflog entries",
        );
        vec![ListItem::new(Line::styled(
            terminal_safe(&message),
            Style::default().fg(Color::DarkGray),
        ))]
    } else {
        entries
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
        .block(panel_block(
            "Reflog",
            app.view_projection().focused == PanelId::Reflog,
        ))
        .highlight_symbol("▶ ")
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        );
    let selected = (!entries.is_empty())
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

fn render_remotes(frame: &mut Frame<'_>, app: &AppState, area: Rect) {
    let [left, right] = columns(area, app);
    let remotes = app.remotes();
    let items = if remotes.is_empty() {
        let message = resource_message(
            app.remotes_repository_index
                .and_then(|index| app.model.repository(RepositoryId(index)))
                .map(|repository| &repository.remotes),
            "Loading remotes…",
            "No remotes configured — press A to add one",
        );
        vec![ListItem::new(Line::styled(
            terminal_safe(&message),
            Style::default().fg(Color::DarkGray),
        ))]
    } else {
        remotes
            .iter()
            .map(|remote| {
                let routing = match (remote.is_upstream, remote.is_push_target) {
                    (true, true) => "★",
                    (true, false) => "F",
                    (false, true) => "P",
                    (false, false) => " ",
                };
                let (policy, policy_style) = if remote.urls_match() {
                    ("✓", Style::default().fg(Color::Green))
                } else {
                    (
                        "!",
                        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                    )
                };
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{routing} "), Style::default().fg(Color::Yellow)),
                    Span::raw(terminal_safe(&remote.name)),
                    Span::raw("  "),
                    Span::styled(policy, policy_style),
                ]))
            })
            .collect()
    };
    let selected = (!remotes.is_empty())
        .then_some(app.selection.selected_remote_index)
        .flatten();
    let list = List::new(items)
        .block(panel_block(
            "Remotes  ★ upstream · F fetch · P push · ! blocked",
            app.view_projection().focused == PanelId::Remotes,
        ))
        .highlight_symbol("▶ ")
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        );
    frame.render_stateful_widget(list, left, &mut list_state(selected));

    let lines = app.selected_remote().map_or_else(
        || {
            vec![
                Line::styled("No remote selected", Style::default().fg(Color::DarkGray)),
                Line::raw(""),
                Line::raw("Press A to add a remote with one URL shared by fetch and push."),
            ]
        },
        |remote| {
            let branch = app.active_repository().and_then(|repository| {
                repository
                    .current_branch
                    .as_ref()
                    .map(|branch| branch.0.as_str())
            });
            let mut lines = vec![
                Line::from(vec![
                    Span::styled("Remote: ", Style::default().fg(Color::Cyan)),
                    Span::styled(
                        terminal_safe(&remote.name),
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("Current branch: ", Style::default().fg(Color::Cyan)),
                    Span::raw(terminal_safe(branch.unwrap_or("detached"))),
                ]),
                Line::from(vec![
                    Span::styled("Routing: ", Style::default().fg(Color::Cyan)),
                    Span::raw(match (remote.is_upstream, remote.is_push_target) {
                        (true, true) => "upstream for fetch/pull and push",
                        (true, false) => "fetch/pull upstream only — blocked until unified",
                        (false, true) => "push target only — blocked until unified",
                        (false, false) => "not selected for the current branch",
                    }),
                ]),
                Line::raw(""),
                Line::styled("Fetch URL(s):", Style::default().fg(Color::Cyan)),
            ];
            if remote.fetch_urls.is_empty() {
                lines.push(Line::styled("  (missing)", Style::default().fg(Color::Red)));
            } else {
                lines.extend(
                    remote
                        .fetch_urls
                        .iter()
                        .map(|url| Line::raw(terminal_safe(&format!("  {url}")))),
                );
            }
            lines.push(Line::styled(
                "Push URL(s):",
                Style::default().fg(Color::Cyan),
            ));
            if remote.push_urls.is_empty() {
                lines.push(Line::styled("  (missing)", Style::default().fg(Color::Red)));
            } else {
                lines.extend(
                    remote
                        .push_urls
                        .iter()
                        .map(|url| Line::raw(terminal_safe(&format!("  {url}")))),
                );
            }
            lines.push(Line::raw(""));
            lines.push(if remote.urls_match() {
                Line::styled(
                    "✓ Fetch and push URLs are identical.",
                    Style::default().fg(Color::Green),
                )
            } else {
                Line::styled(
                    "! BLOCKED: fetch/pull/push require identical URLs. Press e to repair.",
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                )
            });
            lines
        },
    );
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel_block("Remote details", false))
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
    let [left, right] = columns(area, app);
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
                let change = &app.working_tree_changes()[change_index];
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
            app.view_projection().focused == PanelId::Changes,
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
            format!(
                "{} — {}",
                group.title(),
                app.working_tree_changes()[change_index].path
            ),
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
        app.view_projection().focused == PanelId::ChangesDiff,
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
                    let repository = app
                        .repository_node(repository_index)
                        .expect("tree repository must exist in GitModel");
                    let ui = app
                        .repository_ui(RepositoryId(repository_index))
                        .expect("tree repository must have UI state");
                    let active = app.active_repository_index == Some(repository_index);
                    let disclosure = if ui.expanded { "▼" } else { "▶" };
                    let state_marker = if ui.last_error.is_some() {
                        " !"
                    } else if ui.latest_status_job.is_some() || ui.latest_branches_job.is_some() {
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
                            Style::default().fg(if ui.last_error.is_some() {
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
                    let branch = app
                        .repository_branch(repository_index, branch_index)
                        .expect("tree branch must exist in GitModel");
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
        .block(panel_block(
            title,
            app.view_projection().focused == PanelId::RepositoryBranches,
        ))
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

fn commit_tags(decorations: &str) -> String {
    decorations
        .split(',')
        .filter_map(|decoration| decoration.trim().strip_prefix("tag: "))
        .collect::<Vec<_>>()
        .join(", ")
}

fn commit_date_time(authored_at: &str) -> String {
    let Some((date, time)) = authored_at.split_once('T') else {
        return authored_at.to_string();
    };
    let Some(hours_and_minutes) = time.get(..5) else {
        return date.to_string();
    };
    let bytes = hours_and_minutes.as_bytes();
    if bytes.len() == 5
        && bytes[0].is_ascii_digit()
        && bytes[1].is_ascii_digit()
        && bytes[2] == b':'
        && bytes[3].is_ascii_digit()
        && bytes[4].is_ascii_digit()
    {
        format!("{date} {hours_and_minutes}")
    } else {
        date.to_string()
    }
}

fn commit_items(
    app: &AppState,
    density: CommitListDensityConfig,
) -> (Vec<ListItem<'static>>, bool) {
    let commits = app.visible_commits();
    if commits.is_empty() {
        let message = resource_message(
            app.viewing_branch_id()
                .as_ref()
                .and_then(|branch| app.model.branch_commits_resource(branch)),
            "Loading selected branch commits…",
            "No commits",
        );
        return (
            vec![ListItem::new(Line::styled(
                terminal_safe(&message),
                Style::default().fg(Color::DarkGray),
            ))],
            true,
        );
    }
    let items = commits
        .iter()
        .map(|commit| {
            let selected = app.commit_selection.contains(&commit.hash);
            let mut summary = vec![
                Span::styled(
                    if selected { "✓ " } else { "  " },
                    Style::default().fg(Color::Green),
                ),
                Span::styled(
                    terminal_safe(&commit.short_hash),
                    Style::default().fg(Color::Yellow),
                ),
                Span::raw(" "),
                Span::raw(terminal_safe(&commit.subject)),
            ];
            match density {
                CommitListDensityConfig::Compact => {
                    summary.push(Span::styled(
                        if commit.decorations.is_empty() {
                            String::new()
                        } else {
                            terminal_safe(&format!("  {}", commit.decorations))
                        },
                        Style::default().fg(Color::Cyan),
                    ));
                    ListItem::new(Line::from(summary))
                }
                CommitListDensityConfig::Detailed => {
                    let tags = commit_tags(&commit.decorations);
                    let view = app.config.views.view(app.view_projection().view);
                    let mut metadata = vec![Span::raw("  ")];
                    if view.show_commit_datetime {
                        metadata.extend([
                            Span::styled("Date: ", Style::default().fg(Color::Cyan)),
                            Span::styled(
                                terminal_safe(&commit_date_time(&commit.authored_at)),
                                Style::default().fg(Color::Gray),
                            ),
                        ]);
                    }
                    if view.show_commit_author {
                        metadata.extend([
                            Span::raw("  "),
                            Span::styled("Author: ", Style::default().fg(Color::Cyan)),
                            Span::styled(
                                terminal_safe(&commit.author),
                                Style::default().fg(Color::Green),
                            ),
                        ]);
                    }
                    if view.show_commit_tags && !tags.is_empty() {
                        metadata.extend([
                            Span::raw("  "),
                            Span::styled("Tags: ", Style::default().fg(Color::Cyan)),
                            Span::styled(terminal_safe(&tags), Style::default().fg(Color::Magenta)),
                        ]);
                    }
                    ListItem::new(vec![Line::from(summary), Line::from(metadata)])
                }
            }
        })
        .collect();
    (items, false)
}

fn render_commits(
    frame: &mut Frame<'_>,
    app: &AppState,
    area: Rect,
    density: CommitListDensityConfig,
) {
    let (items, empty) = commit_items(app, density);
    let selection_count = app.commit_selection.len();
    let title = if app.effective_commit_filter().is_empty() {
        format!("Commits · {selection_count} selected")
    } else {
        terminal_safe(&format!(
            "Commits /{} · {selection_count} selected",
            app.effective_commit_filter()
        ))
    };
    let list = List::new(items)
        .block(panel_block(
            title,
            app.view_projection().focused == PanelId::Commits,
        ))
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
    let [left, right] = columns(area, app);
    render_commits(
        frame,
        app,
        left,
        app.config.views.view(ViewId::Commit).commit_density,
    );
    render_commit_column(frame, app, right);
}

/// Shared Commit column used on both sides of the hierarchical transition:
/// `Commits | Commit` -> `Commit | Diff`.
fn render_commit_column(frame: &mut Frame<'_>, app: &AppState, area: Rect) {
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(8), Constraint::Min(3)])
        .split(area);
    render_commit_metadata(frame, app, sections[0]);
    render_changed_files(
        frame,
        app,
        sections[1],
        app.view_projection().focused == PanelId::Commit,
    );
}

fn render_commit_metadata(frame: &mut Frame<'_>, app: &AppState, area: Rect) {
    let detail = app.current_commit_detail();
    let lines = detail.as_ref().map_or_else(
        || {
            let message = resource_message(
                app.selected_commit_id()
                    .or_else(|| app.current_commit_id())
                    .as_ref()
                    .and_then(|commit| app.model.commit_metadata_resource(commit)),
                "Loading selected commit…",
                "No commit selected",
            );
            vec![Line::styled(
                terminal_safe(&message),
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

fn render_changed_files(frame: &mut Frame<'_>, app: &AppState, area: Rect, focused: bool) {
    let detail = app.current_commit_detail();
    let files = detail
        .as_ref()
        .map(|detail| detail.files.as_slice())
        .unwrap_or_default();
    let items = if files.is_empty() {
        let message = resource_message(
            app.selected_commit_id()
                .or_else(|| app.current_commit_id())
                .as_ref()
                .and_then(|commit| app.model.commit_metadata_resource(commit)),
            "Loading changed files…",
            "No changed files",
        );
        vec![ListItem::new(Line::styled(
            terminal_safe(&message),
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
        .block(panel_block("Files changed in commit", focused))
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
    let [left, right] = columns(area, app);
    render_commit_column(frame, app, left);
    let empty_message = resource_message(
        app.selected_file_id()
            .as_ref()
            .and_then(|file| app.model.file_diff_resource(file)),
        "Loading selected file diff…",
        "No diff loaded",
    );
    render_diff_panel(
        frame,
        app.current_file_diff(),
        app.diff_mode,
        app.wrap_diff,
        app.selection.diff_scroll,
        app.view_projection().focused == PanelId::FileDiff,
        right,
        terminal_width,
        "Changes",
        &empty_message,
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
    let modal_footer =
        || modal_shortcut_set(&app.mode).map_or(String::new(), |set| set.footer.to_string());
    let mut static_text = match &app.mode {
        GlobalMode::Filtering { target, query } => format!(
            " /{}: {}_ | {} ",
            match target {
                FilterTarget::Branches => "branches",
                FilterTarget::Commits => "commits",
            },
            query,
            modal_footer()
        ),
        GlobalMode::Confirming { .. }
        | GlobalMode::TypingConfirmation { .. }
        | GlobalMode::EditingCommitMessage { .. }
        | GlobalMode::CommandPrompt { .. }
        | GlobalMode::ShortcutHelp { .. }
        | GlobalMode::OperationPalette { .. }
        | GlobalMode::Error => format!(" {} ", modal_footer()),
        GlobalMode::EditingRemote { kind, field, .. } => match kind {
            RemoteEditKind::Add => format!(
                " Add remote: editing {} | {} ",
                match field {
                    RemoteInputField::Name => "name",
                    RemoteInputField::Url => "shared URL",
                },
                modal_footer()
            ),
            RemoteEditKind::SetUrl { .. } => {
                format!(" Set shared fetch/push URL | {} ", modal_footer())
            }
        },
        GlobalMode::Chord { .. } | GlobalMode::Normal => String::new(),
    };

    let text = if matches!(app.mode, GlobalMode::Normal | GlobalMode::Chord { .. }) {
        let mut items = app
            .config
            .keymap
            .footer_items(app, &app.config.footer)
            .into_iter()
            .map(|item| format!("{} {}", item.key, item.label))
            .collect::<Vec<_>>();
        if matches!(app.mode, GlobalMode::Chord { .. }) {
            items.push("Esc cancel".into());
        }
        if let Some(message) = app.last_message.as_ref() {
            items.push(format!("✓ {message}"));
        }
        footer_lines(
            &items,
            usize::from(area.width),
            usize::from(area.height),
            &app.config.footer.separator,
            app.config.footer.overflow,
        )
    } else {
        if let Some(message) = app.last_message.as_ref() {
            static_text.push_str(&format!(" | ✓ {message} "));
        }
        vec![terminal_safe(&static_text)]
    };
    frame.render_widget(
        Paragraph::new(Text::from(
            text.into_iter().map(Line::raw).collect::<Vec<_>>(),
        ))
        .style(Style::default().bg(Color::DarkGray).fg(Color::White)),
        area,
    );
}

fn footer_lines(
    items: &[String],
    width: usize,
    max_rows: usize,
    separator: &str,
    overflow: FooterOverflow,
) -> Vec<String> {
    if width == 0 || max_rows == 0 {
        return Vec::new();
    }
    let separator = terminal_safe(separator);
    let items = items
        .iter()
        .map(|item| terminal_safe(item))
        .collect::<Vec<_>>();
    let mut lines = vec![Vec::<String>::new()];
    let mut hidden = 0usize;
    for (index, item) in items.iter().enumerate() {
        let current = lines.last_mut().expect("at least one footer line");
        let current_width = current
            .iter()
            .map(|entry| UnicodeWidthStr::width(entry.as_str()))
            .sum::<usize>()
            + separator
                .width()
                .saturating_mul(current.len().saturating_sub(1));
        let item_width = UnicodeWidthStr::width(item.as_str());
        let separator_width = if current.is_empty() {
            0
        } else {
            separator.width()
        };
        let candidate_width = current_width + separator_width + item_width;
        if candidate_width <= width {
            current.push(item.clone());
        } else if item_width <= width && lines.len() < max_rows {
            lines.push(vec![item.clone()]);
        } else {
            hidden = items.len().saturating_sub(index);
            break;
        }
    }
    if hidden > 0 {
        loop {
            let mut suffix = match overflow {
                FooterOverflow::Count => format!("… +{hidden}"),
                FooterOverflow::Ellipsis => "…".into(),
            };
            if UnicodeWidthStr::width(suffix.as_str()) > width {
                suffix = "…".into();
            }
            let last = lines.last_mut().expect("at least one footer line");
            let current = last.join(&separator);
            let separator_width = if last.is_empty() {
                0
            } else {
                separator.width()
            };
            let candidate_width = UnicodeWidthStr::width(current.as_str())
                + separator_width
                + UnicodeWidthStr::width(suffix.as_str());
            if candidate_width <= width {
                last.push(suffix);
                break;
            }
            if last.pop().is_some() {
                hidden = hidden.saturating_add(1);
                continue;
            }
            // Only possible for a zero-column terminal (handled above) or an
            // exotic width implementation. Avoid looping even in that case.
            break;
        }
    }
    lines
        .into_iter()
        .map(|line| line.join(&separator))
        .collect()
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
        GlobalMode::OperationPalette {
            query, selected, ..
        } => {
            let operations = app.operation_palette_matches();
            let mut lines = vec![
                Line::from(vec![
                    Span::styled("Filter: ", Style::default().fg(Color::Cyan)),
                    Span::styled(
                        terminal_safe(&format!("> {query}_")),
                        Style::default().fg(Color::Yellow),
                    ),
                ]),
                Line::styled(
                    format!(
                        "{} available operation(s) for the originating focus",
                        operations.len()
                    ),
                    Style::default().fg(Color::DarkGray),
                ),
                Line::raw(""),
            ];
            if operations.is_empty() {
                lines.push(Line::styled(
                    "No operation matches this filter",
                    Style::default().fg(Color::DarkGray),
                ));
            } else {
                let window = 18usize;
                let start = selected
                    .saturating_sub(window / 2)
                    .min(operations.len().saturating_sub(window));
                for (index, operation) in operations.iter().enumerate().skip(start).take(window) {
                    let active = index == *selected;
                    lines.push(Line::from(vec![
                        Span::styled(
                            if active { "▶ " } else { "  " },
                            Style::default().fg(Color::Yellow),
                        ),
                        Span::styled(
                            format!(
                                "{:<22}",
                                terminal_safe(&app.config.keymap.display_bindings_for_view(
                                    app.view_projection().view,
                                    *operation,
                                ))
                            ),
                            Style::default().fg(Color::Green),
                        ),
                        Span::styled(
                            format!(
                                "{:<22}",
                                terminal_safe(&app.config.operation_label(*operation))
                            ),
                            if active {
                                Style::default()
                                    .fg(Color::White)
                                    .add_modifier(Modifier::BOLD)
                            } else {
                                Style::default().fg(Color::Gray)
                            },
                        ),
                        Span::styled(
                            terminal_safe(operation.as_str()),
                            Style::default().fg(Color::DarkGray),
                        ),
                    ]));
                }
            }
            lines.extend([
                Line::raw(""),
                Line::raw("↑/↓ select · Enter run · Esc cancel"),
            ]);
            ("Operations — resolved for the originating focus", lines, 72)
        }
        GlobalMode::CommandPrompt {
            input,
            validation_error,
        } => {
            let mut lines = vec![
                Line::raw("Command:"),
                Line::styled(
                    terminal_safe(&format!("> {input}_")),
                    Style::default().fg(Color::Yellow),
                ),
                Line::raw(""),
                Line::styled("Available commands:", Style::default().fg(Color::Cyan)),
            ];
            lines.extend(PROMPT_OPERATION_SPECS.iter().map(|command| {
                Line::from(vec![
                    Span::styled(
                        format!("  {:<12}", terminal_safe(command.name)),
                        Style::default().fg(Color::Green),
                    ),
                    Span::raw(terminal_safe(command.description)),
                ])
            }));
            if let Some(error) = validation_error {
                lines.push(Line::raw(""));
                lines.push(Line::styled(
                    terminal_safe(error),
                    Style::default().fg(Color::Red),
                ));
            }
            lines.push(Line::raw(""));
            lines.push(Line::raw("Enter run | Esc cancel"));
            ("Quick command", lines, 38)
        }
        GlobalMode::ShortcutHelp { .. } => (
            "Current shortcut reference — effective bindings for this focus",
            shortcut_help_lines(app),
            90,
        ),
        GlobalMode::Confirming { dialog } => match dialog {
            ConfirmDialog::FetchRepository { repository_index } => {
                let repository = app.repository_node(*repository_index);
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
                        Line::raw("Every remote must use identical fetch and push URLs."),
                        Line::raw("Enter confirm | Esc cancel"),
                    ],
                    46,
                )
            }
            ConfirmDialog::PullRebaseRepository {
                repository_index,
                branch,
            } => {
                let repository = app.repository_node(*repository_index);
                (
                    "Pull with rebase",
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
                        Line::raw(terminal_safe(&format!("Current branch: {}", branch.0))),
                        Line::raw(""),
                        Line::raw("About to run:"),
                        Line::styled("git pull --rebase", Style::default().fg(Color::Yellow)),
                        Line::raw(""),
                        Line::raw("The working tree and index must be clean."),
                        Line::raw("The upstream remote must use one shared fetch/push URL."),
                        Line::raw(
                            "If rebase conflicts occur, Pitui will automatically abort them.",
                        ),
                        Line::raw(""),
                        Line::raw("Enter confirm | Esc cancel"),
                    ],
                    58,
                )
            }
            ConfirmDialog::PushRepository {
                repository_index,
                branch,
            } => {
                let repository = app.repository_node(*repository_index);
                (
                    "Push current branch",
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
                        Line::raw(terminal_safe(&format!("Current branch: {}", branch.0))),
                        Line::raw(""),
                        Line::raw("About to run:"),
                        Line::styled("git push", Style::default().fg(Color::Yellow)),
                        Line::raw(""),
                        Line::raw("The configured upstream/default push target will be used."),
                        Line::raw("No upstream is created automatically."),
                        Line::raw(
                            "Fetch upstream and push target must resolve to the same remote.",
                        ),
                        Line::raw(""),
                        Line::raw("Enter confirm | Esc cancel"),
                    ],
                    56,
                )
            }
            ConfirmDialog::AddRemote { name, url, .. } => (
                "Add remote",
                vec![
                    Line::from(vec![
                        Span::styled("Remote: ", Style::default().fg(Color::Cyan)),
                        Span::raw(terminal_safe(name)),
                    ]),
                    Line::styled(
                        "The same URL will be used for fetch and push:",
                        Style::default().fg(Color::Green),
                    ),
                    Line::raw(terminal_safe(url)),
                    Line::raw(""),
                    Line::raw("About to run:"),
                    Line::styled(
                        terminal_safe(&format!("git remote add -- {name} <shared-url>")),
                        Style::default().fg(Color::Yellow),
                    ),
                    Line::raw(""),
                    Line::raw("Enter confirm | Esc cancel"),
                ],
                48,
            ),
            ConfirmDialog::SetRemoteUrl { name, url, .. } => (
                "Set shared remote URL",
                vec![
                    Line::from(vec![
                        Span::styled("Remote: ", Style::default().fg(Color::Cyan)),
                        Span::raw(terminal_safe(name)),
                    ]),
                    Line::raw("New fetch/push URL:"),
                    Line::raw(terminal_safe(url)),
                    Line::raw(""),
                    Line::styled(
                        "This replaces all fetch URLs and removes separate push URLs.",
                        Style::default().fg(Color::Yellow),
                    ),
                    Line::raw("Pitui then uses this one URL in both directions."),
                    Line::raw(""),
                    Line::raw("Enter confirm | Esc cancel"),
                ],
                50,
            ),
            ConfirmDialog::SetUpstreamRemote { name, branch, .. } => (
                "Set upstream remote",
                vec![
                    Line::from(vec![
                        Span::styled("Current branch: ", Style::default().fg(Color::Cyan)),
                        Span::raw(terminal_safe(&branch.0)),
                    ]),
                    Line::from(vec![
                        Span::styled("Remote: ", Style::default().fg(Color::Cyan)),
                        Span::raw(terminal_safe(name)),
                    ]),
                    Line::raw(""),
                    Line::raw("Pitui will configure:"),
                    Line::styled(
                        terminal_safe(&format!("fetch/pull: {name}/{}", branch.0)),
                        Style::default().fg(Color::Green),
                    ),
                    Line::styled(
                        terminal_safe(&format!("push:       {name}/{}", branch.0)),
                        Style::default().fg(Color::Green),
                    ),
                    Line::raw(""),
                    Line::raw("The remote branch may be created by the next push."),
                    Line::raw("Enter confirm | Esc cancel"),
                ],
                54,
            ),
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
            ConfirmDialog::CherryPickSelected { commits, .. } => {
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
                    Line::raw("Selected commits (oldest to newest):"),
                ];
                for (index, commit) in commits.iter().enumerate() {
                    let subject = app
                        .branch_commit_summaries()
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
                ("Cherry-pick selected commits", lines, 60)
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
                .and_then(|index| app.repository_node(index))
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
        GlobalMode::EditingRemote {
            kind,
            field,
            name,
            url,
            validation_error,
        } => {
            let adding = matches!(kind, RemoteEditKind::Add);
            let name_active = adding && *field == RemoteInputField::Name;
            let url_active = *field == RemoteInputField::Url;
            let mut lines = vec![
                Line::raw("Remote name:"),
                Line::styled(
                    terminal_safe(&format!("> {name}{}", if name_active { "_" } else { "" })),
                    Style::default().fg(if name_active {
                        Color::Yellow
                    } else {
                        Color::White
                    }),
                ),
                Line::raw(""),
                Line::raw("Shared fetch/push URL:"),
                Line::styled(
                    terminal_safe(&format!("> {url}{}", if url_active { "_" } else { "" })),
                    Style::default().fg(if url_active {
                        Color::Yellow
                    } else {
                        Color::White
                    }),
                ),
                Line::raw(""),
                Line::styled(
                    "Pitui does not allow a separate push URL.",
                    Style::default().fg(Color::Green),
                ),
            ];
            if let Some(error) = validation_error {
                lines.push(Line::styled(
                    terminal_safe(error),
                    Style::default().fg(Color::Red),
                ));
            }
            lines.push(Line::raw(if adding {
                "Tab switch field | Enter continue | Esc cancel"
            } else {
                "Enter continue | Esc cancel"
            }));
            (
                if adding {
                    "Add remote"
                } else {
                    "Set shared remote URL"
                },
                lines,
                54,
            )
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

    let help_popup = matches!(app.mode, GlobalMode::ShortcutHelp { .. });
    let palette_popup = matches!(app.mode, GlobalMode::OperationPalette { .. });
    let popup = centered_rect(
        if help_popup {
            94
        } else if palette_popup {
            88
        } else {
            72
        },
        height,
        area,
    );
    let popup_scroll = match app.mode {
        GlobalMode::ShortcutHelp { scroll } => {
            let viewport = usize::from(popup.height.saturating_sub(2)).max(1);
            scroll.min(
                lines
                    .len()
                    .saturating_sub(viewport)
                    .min(usize::from(u16::MAX)) as u16,
            )
        }
        _ => 0,
    };
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title(title)
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Yellow)),
            )
            .scroll((popup_scroll, 0))
            .wrap(Wrap { trim: false }),
        popup,
    );
}

fn shortcut_help_lines(app: &AppState) -> Vec<Line<'static>> {
    let current_context = Some(app.focus_context().kind);
    let mut lines = vec![
        Line::styled(
            "Only global commands and the originating focus are shown.",
            Style::default().fg(Color::DarkGray),
        ),
        Line::styled(
            "Bindings are resolved from the same configurable command tables used by input.",
            Style::default().fg(Color::DarkGray),
        ),
        Line::raw(""),
    ];
    for section in app
        .config
        .shortcut_help_sections_for_view(app.view_projection().view, current_context)
    {
        let current = section.context.is_some() && section.context == current_context;
        lines.push(Line::styled(
            terminal_safe(&format!(
                "{} {}",
                if current { "▶" } else { " " },
                section.title
            )),
            Style::default()
                .fg(if current { Color::Yellow } else { Color::Cyan })
                .add_modifier(Modifier::BOLD),
        ));
        for item in section.items {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {:<25}", terminal_safe(&item.key)),
                    Style::default().fg(Color::Green),
                ),
                Span::styled(
                    format!("{:<23}", terminal_safe(&item.label)),
                    Style::default().fg(Color::White),
                ),
                Span::styled(
                    terminal_safe(&item.operation),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
        }
        lines.push(Line::raw(""));
    }
    lines
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Instant};

    use ratatui::{Terminal, backend::TestBackend};

    use super::*;
    use crate::domain::{
        BranchName, ChangedFile, Commit, CommitDetail, CommitHash, DiffHunk, DiffLine,
        FileChangeKind, FileDiff, GitPath, ReflogEntry, RemoteInfo, Repository, WorkingTreeChange,
        WorkingTreeStatus,
    };
    use crate::{
        app::{BranchId, FocusKind, FocusRole, OperationId, RepositoryId},
        config::{KeyStroke, ResolvedConfig},
    };

    fn focus(state: &mut AppState, kind: FocusKind, role: FocusRole) {
        state.set_focus_layer(kind, role);
    }

    fn seed_commits(state: &mut AppState, commits: Vec<Commit>) {
        let branch = BranchId {
            repository: RepositoryId(0),
            name: BranchName("main".into()),
        };
        state.model.replace_branch_commits(&branch, commits);
        state.viewing_branch = Some(branch);
        state.ensure_valid_commit_selection();
    }

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
        assert!(!rendered.contains("Ctrl+R refresh"));
        assert!(!rendered.contains("Ctrl+G changes"));
    }

    #[test]
    fn status_bar_omits_view_focus_and_removed_queue_fields_at_every_width() {
        for width in [80, 120, 160] {
            let backend = TestBackend::new(width, 12);
            let mut terminal = Terminal::new(backend).unwrap();
            let state = AppState::default();
            terminal.draw(|frame| render(frame, &state)).unwrap();
            let rendered = buffer_text(&terminal);
            assert!(!rendered.contains("view="));
            assert!(!rendered.contains("viewing="));
            assert!(!rendered.contains("focus="));
            assert!(!rendered.contains("queue="));
            assert!(!rendered.contains(" V="));
            assert!(!rendered.contains(" F="));
        }
    }

    #[test]
    fn renders_detailed_commits_on_the_right_and_compact_commits_on_the_left() {
        let mut state = AppState::with_repository_paths(vec!["/repo".into()]);
        seed_commits(
            &mut state,
            vec![Commit {
                hash: CommitHash("0123456789abcdef".into()),
                short_hash: "0123456".into(),
                author: "Ada Lovelace".into(),
                authored_at: "2026-07-16T10:20:30+08:00".into(),
                decorations: "HEAD -> main, tag: v1.2.3, origin/main".into(),
                subject: "show rich commit metadata".into(),
            }],
        );

        let backend = TestBackend::new(160, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &state)).unwrap();
        let rendered = buffer_text(&terminal);
        assert!(rendered.contains("Date: 2026-07-16 10:20"));
        assert!(rendered.contains("Author: Ada Lovelace"));
        assert!(rendered.contains("Tags: v1.2.3"));

        focus(&mut state, FocusKind::Commit, FocusRole::Entity);
        terminal.draw(|frame| render(frame, &state)).unwrap();
        let rendered = buffer_text(&terminal);
        assert!(!rendered.contains("Date: 2026-07-16 10:20"));
        assert!(!rendered.contains("Author: Ada Lovelace"));
        assert!(!rendered.contains("Tags: v1.2.3"));
        assert!(rendered.contains("show rich commit metadata"));
    }

    #[test]
    fn detailed_commit_omits_the_tag_field_when_no_git_tag_exists() {
        let mut state = AppState::with_repository_paths(vec!["/repo".into()]);
        seed_commits(
            &mut state,
            vec![Commit {
                hash: CommitHash("0123456789abcdef".into()),
                short_hash: "0123456".into(),
                author: "Ada Lovelace".into(),
                authored_at: "2026-07-16T09:05:30Z".into(),
                decorations: "HEAD -> main, origin/main".into(),
                subject: "no tag on this commit".into(),
            }],
        );

        let backend = TestBackend::new(160, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &state)).unwrap();
        let rendered = buffer_text(&terminal);
        assert!(rendered.contains("Date: 2026-07-16 09:05"));
        assert!(rendered.contains("Author: Ada Lovelace"));
        assert!(!rendered.contains("Tags:"));
    }

    #[test]
    fn renders_only_the_current_chord_level_in_the_footer() {
        let mut state = AppState::with_repository_paths(vec!["/repo".into()]);
        focus(&mut state, FocusKind::Commit, FocusRole::Collection);
        seed_commits(
            &mut state,
            vec![Commit {
                hash: CommitHash("0123456789abcdef".into()),
                short_hash: "01234567".into(),
                author: "Ada".into(),
                authored_at: "2026-07-16".into(),
                decorations: String::new(),
                subject: "copy this commit".into(),
            }],
        );

        let backend = TestBackend::new(200, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &state)).unwrap();
        let rendered = buffer_text(&terminal);
        assert!(rendered.contains("Ctrl+C copy…"));
        assert!(!rendered.contains("h hash"));
        assert!(!rendered.contains("i info"));
        assert!(!rendered.contains("m message"));

        state.mode = GlobalMode::Chord {
            prefix: vec![KeyStroke::parse("Ctrl+C").unwrap()],
            started_at: Instant::now(),
        };
        terminal.draw(|frame| render(frame, &state)).unwrap();
        let rendered = buffer_text(&terminal);
        assert!(rendered.contains("h hash"));
        assert!(rendered.contains("i info"));
        assert!(rendered.contains("m message"));
        assert!(rendered.contains("Esc cancel"));
        assert!(!rendered.contains("Ctrl+C copy…"));
    }

    #[test]
    fn renders_current_focus_shortcut_reference_from_effective_operation_tables() {
        let mut state = AppState::default();
        focus(&mut state, FocusKind::Commit, FocusRole::Collection);
        state.mode = GlobalMode::ShortcutHelp { scroll: 0 };
        let backend = TestBackend::new(180, 80);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &state)).unwrap();
        let rendered = buffer_text(&terminal);
        assert!(rendered.contains("Current shortcut reference"));
        assert!(rendered.contains("app.shortcuts"));
        assert!(rendered.contains("app.command_prompt"));
        assert!(!rendered.contains("Ctrl+?"));
        assert!(rendered.contains("[commit]"));
        assert!(rendered.contains("commit.copy.message"));
        assert!(rendered.contains("▶ Commit"));
        assert!(!rendered.contains("[branch]"));
        assert!(!rendered.contains("file.copy.absolute_path"));
        assert!(!rendered.contains("commit submission"));

        let mut state = AppState::default();
        focus(&mut state, FocusKind::Commit, FocusRole::Collection);
        state.mode = GlobalMode::ShortcutHelp { scroll: u16::MAX };
        let backend = TestBackend::new(180, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &state)).unwrap();
        let rendered = buffer_text(&terminal);
        assert!(rendered.contains("shortcut reference"));
        assert!(rendered.contains("h/Enter/Esc/q close"));
        assert!(!rendered.contains("Quick command · accepted commands"));
    }

    #[test]
    fn renders_quick_command_prompt_and_validation() {
        let backend = TestBackend::new(120, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let state = AppState {
            mode: GlobalMode::CommandPrompt {
                input: "unknown".into(),
                validation_error: Some(
                    "Unknown command `unknown`. Type `help` for the shortcut guide.".into(),
                ),
            },
            ..AppState::default()
        };
        terminal.draw(|frame| render(frame, &state)).unwrap();
        let rendered = buffer_text(&terminal);
        assert!(rendered.contains("Quick command"));
        assert!(rendered.contains("> unknown_"));
        assert!(rendered.contains("help"));
        assert!(rendered.contains("open the shortcut guide"));
        assert!(rendered.contains("Unknown command `unknown`"));
    }

    #[test]
    fn renders_searchable_operation_palette_from_registry_ids() {
        let backend = TestBackend::new(150, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        let state = AppState {
            mode: GlobalMode::OperationPalette {
                query: "help".into(),
                selected: 0,
                operations: vec![
                    OperationId::AppShortcutHelp,
                    OperationId::AppRefresh,
                    OperationId::NavigationDown,
                ],
            },
            ..AppState::default()
        };
        terminal.draw(|frame| render(frame, &state)).unwrap();
        let rendered = buffer_text(&terminal);
        assert!(rendered.contains("Operations"));
        assert!(rendered.contains("> help_"));
        assert!(rendered.contains("app.shortcuts"));
        assert!(rendered.contains("h"));
        assert!(!rendered.contains("app.refresh"));
    }

    #[test]
    fn configured_footer_rows_do_not_replace_the_status_bar() {
        let mut config = ResolvedConfig::default();
        config.footer.max_rows = 2;
        let mut state = AppState::with_config(vec!["/repo".into()], Arc::new(config));
        focus(&mut state, FocusKind::Commit, FocusRole::Collection);
        seed_commits(
            &mut state,
            vec![Commit {
                hash: CommitHash("0123456789abcdef".into()),
                short_hash: "01234567".into(),
                author: "Ada".into(),
                authored_at: "2026-07-16".into(),
                decorations: String::new(),
                subject: "many footer actions".into(),
            }],
        );

        let backend = TestBackend::new(42, 15);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &state)).unwrap();
        let buffer = terminal.backend().buffer();
        let row = |y: u16| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        };
        assert!(
            !row(0).trim().is_empty(),
            "status bar must stay on the first row"
        );
        assert!(!row(13).trim().is_empty(), "first footer row must be used");
        assert!(!row(14).trim().is_empty(), "second footer row must be used");
    }

    #[test]
    fn hidden_footer_keeps_the_status_and_main_view_visible() {
        let mut config = ResolvedConfig::default();
        config.footer.mode = FooterMode::Hidden;
        let state = AppState::with_config(Vec::new(), Arc::new(config));
        let backend = TestBackend::new(100, 15);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &state)).unwrap();
        let rendered = buffer_text(&terminal);
        assert!(rendered.contains("Branches"));
        assert!(rendered.contains("repo=—"));
        assert!(!rendered.contains("q quit"));
    }

    #[test]
    fn footer_overflow_removes_whole_items() {
        let lines = footer_lines(
            &["alpha one".into(), "beta two".into(), "gamma three".into()],
            16,
            1,
            " | ",
            FooterOverflow::Count,
        );
        assert_eq!(lines, vec!["alpha one | … +2"]);

        let lines = footer_lines(
            &["an item wider than the terminal".into(), "second".into()],
            8,
            1,
            " | ",
            FooterOverflow::Count,
        );
        assert_eq!(lines, vec!["… +2"]);
    }

    #[test]
    fn renders_reflog_list_and_selected_entry() {
        let backend = TestBackend::new(120, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState {
            reflog_repository_index: Some(0),
            ..AppState::with_repository_paths(vec!["/repo".into()])
        };
        state.model.set_reflog(
            RepositoryId(0),
            vec![ReflogEntry {
                hash: CommitHash("0123456789abcdef".into()),
                short_hash: "0123456".into(),
                selector: "HEAD@{0}".into(),
                action: "commit".into(),
                message: "add reflog view".into(),
                author: "Ada".into(),
                authored_at: "2026-07-16T00:00:00Z".into(),
            }],
        );
        state.ensure_valid_reflog_selection();
        focus(&mut state, FocusKind::Reflog, FocusRole::Entity);
        terminal.draw(|frame| render(frame, &state)).unwrap();
        let rendered = buffer_text(&terminal);
        assert!(rendered.contains("HEAD@{0}"));
        assert!(rendered.contains("add reflog view"));
        assert!(rendered.contains("Reflog entry"));
        assert!(rendered.contains("R reset"));
    }

    #[test]
    fn renders_remote_routing_and_blocks_split_urls() {
        let backend = TestBackend::new(160, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState {
            remotes_repository_index: Some(0),
            ..AppState::with_repository_paths(vec!["/repo".into()])
        };
        state.model.set_remotes(
            RepositoryId(0),
            vec![RemoteInfo {
                name: "origin".into(),
                fetch_urls: vec!["ssh://fetch.example/repo.git".into()],
                push_urls: vec!["ssh://push.example/repo.git".into()],
                is_upstream: true,
                is_push_target: false,
            }],
        );
        state.ensure_valid_remote_selection();
        focus(&mut state, FocusKind::Remote, FocusRole::Entity);
        terminal.draw(|frame| render(frame, &state)).unwrap();
        let rendered = buffer_text(&terminal);
        assert!(rendered.contains("Remotes"));
        assert!(rendered.contains("ssh://fetch.example/repo.git"));
        assert!(rendered.contains("ssh://push.example/repo.git"));
        assert!(rendered.contains("BLOCKED"));
        assert!(rendered.contains("A add remote"));
        assert!(rendered.contains("u set upstream"));
    }

    #[test]
    fn renders_three_level_changes_tree_and_reused_staged_diff() {
        let backend = TestBackend::new(140, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let path = GitPath::from("src/main.rs");
        let mut state = AppState {
            changes_repository_index: Some(0),
            current_changes_diff: Some(FileDiff {
                commit: CommitHash("INDEX".into()),
                path: path.clone(),
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
                path: path.clone(),
            }]),
            ..AppState::with_repository_paths(vec!["/repo".into()])
        };
        state.model.set_working_tree(
            RepositoryId(0),
            vec![WorkingTreeChange {
                index_status: 'M',
                worktree_status: ' ',
                path,
                old_path: None,
            }],
        );
        state.ensure_valid_changes_selection();
        focus(&mut state, FocusKind::Changes, FocusRole::Entity);
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
        let mut state = AppState {
            changes_repository_index: Some(0),
            mode: GlobalMode::EditingCommitMessage {
                input: "add selected files".into(),
                validation_error: Some("example validation".into()),
            },
            ..AppState::with_repository_paths(vec!["/repo".into()])
        };
        state.model.set_working_tree(
            RepositoryId(0),
            vec![WorkingTreeChange {
                index_status: 'A',
                worktree_status: ' ',
                path: GitPath::from("new.txt"),
                old_path: None,
            }],
        );
        focus(&mut state, FocusKind::Changes, FocusRole::Entity);
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
        state.model.set_repository_summary(
            RepositoryId(0),
            Repository {
                root: "/tmp/example".into(),
                name: "example".into(),
                current_branch: Some(crate::domain::BranchName("main".into())),
                head: CommitHash(String::new()),
                status: WorkingTreeStatus::default(),
            },
        );
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
            let help = AppState {
                mode: GlobalMode::ShortcutHelp { scroll: u16::MAX },
                ..AppState::default()
            };
            terminal.draw(|frame| render(frame, &help)).unwrap();
        }
    }

    #[test]
    fn renders_file_diff_with_reused_commit_column_and_side_by_side_diff() {
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
        let detail = CommitDetail {
            commit: commit.clone(),
            author_email: "test@example.invalid".into(),
            committer: "Test".into(),
            committer_email: "test@example.invalid".into(),
            committed_at: "2026-07-16T00:00:00Z".into(),
            message: "change".into(),
            files: vec![file],
        };
        let diff = FileDiff {
            commit: commit.hash.clone(),
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
        };
        let mut state = AppState::with_repository_paths(vec!["/repo".into()]);
        state.diff_mode = DiffViewMode::SideBySide;
        seed_commits(&mut state, vec![commit]);
        state.model.set_commit_detail(RepositoryId(0), detail);
        state.model.set_file_diff(RepositoryId(0), diff);
        state.selection.selected_file_index = Some(0);
        focus(&mut state, FocusKind::Diff, FocusRole::Content);

        let backend = TestBackend::new(160, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &state)).unwrap();
        let rendered = buffer_text(&terminal);
        assert!(rendered.contains("Commit"));
        assert!(rendered.contains("0123456789abcdef"));
        assert!(rendered.contains("Files changed in commit"));
        assert!(rendered.contains("file.rs"));
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
