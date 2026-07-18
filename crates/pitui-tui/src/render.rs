use pitui_core::{DiffCellKind, DiffLineKind};
use pitui_data::{
    CellProjection, KeyCode, KeyStroke, LayoutConstraint, RenderContentProjection,
    RenderProxyProjection, ResolvedKeyAction, RowProjection, UiFrame, UiLayoutProjection,
    ViewportMeasurement, ViewportProjection,
};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub fn render(frame: &mut Frame<'_>, ui: &UiFrame) -> Vec<ViewportMeasurement> {
    let area = frame.area();
    let status_height = u16::from(!ui.status.items.is_empty() && area.height > 0);
    let footer_height = u16::from(!ui.footer.bindings.is_empty() && area.height > status_height);
    let main_height = area
        .height
        .saturating_sub(status_height)
        .saturating_sub(footer_height);
    let main = Rect::new(area.x, area.y, area.width, main_height);
    let status = Rect::new(area.x, area.y + main_height, area.width, status_height);
    let footer = Rect::new(
        area.x,
        area.y + main_height + status_height,
        area.width,
        footer_height,
    );

    let mut measurements = Vec::new();
    render_layout(frame, &ui.layout, main, &mut measurements);
    if status.height > 0 {
        frame.render_widget(
            Paragraph::new(terminal_safe(&ui.status.items.join(" | ")))
                .style(Style::default().bg(Color::Blue).fg(Color::White)),
            status,
        );
    }
    if footer.height > 0 {
        let text = ui
            .footer
            .bindings
            .iter()
            .map(|binding| {
                let suffix = if matches!(binding.action, ResolvedKeyAction::EnterChord(_)) {
                    "…"
                } else {
                    ""
                };
                format!(
                    "{} {}{}",
                    format_key(&binding.stroke),
                    terminal_safe(&binding.label),
                    suffix
                )
            })
            .collect::<Vec<_>>()
            .join("  ");
        frame.render_widget(
            Paragraph::new(text).style(Style::default().fg(Color::DarkGray)),
            footer,
        );
    }
    measurements
}

fn render_layout(
    frame: &mut Frame<'_>,
    layout: &UiLayoutProjection,
    area: Rect,
    measurements: &mut Vec<ViewportMeasurement>,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    match layout {
        UiLayoutProjection::Empty => {}
        UiLayoutProjection::Row(children) => {
            render_split(frame, children, area, Direction::Horizontal, measurements);
        }
        UiLayoutProjection::Column(children) => {
            render_split(frame, children, area, Direction::Vertical, measurements);
        }
        UiLayoutProjection::Overlay(children) => {
            for (index, child) in children.iter().enumerate() {
                let child_area = if index == 0 {
                    area
                } else {
                    centered_overlay_area(child, area)
                };
                if index > 0 {
                    frame.render_widget(Clear, child_area);
                }
                render_layout(frame, child, child_area, measurements);
            }
        }
        UiLayoutProjection::Dataset { panel, .. } => {
            render_panel(frame, panel, area, measurements);
        }
    }
}

fn render_split(
    frame: &mut Frame<'_>,
    children: &[UiLayoutProjection],
    area: Rect,
    direction: Direction,
    measurements: &mut Vec<ViewportMeasurement>,
) {
    if children.is_empty() {
        return;
    }
    let constraints = children.iter().map(layout_constraint).collect::<Vec<_>>();
    let areas = Layout::default()
        .direction(direction)
        .constraints(constraints)
        .split(area);
    for (child, area) in children.iter().zip(areas.iter()) {
        render_layout(frame, child, *area, measurements);
    }
}

fn layout_constraint(layout: &UiLayoutProjection) -> Constraint {
    match layout {
        UiLayoutProjection::Dataset { constraint, .. } => match constraint {
            LayoutConstraint::Minimum(size) => Constraint::Min(*size),
            LayoutConstraint::Percentage(percent) => Constraint::Percentage(*percent),
            LayoutConstraint::Fill(weight) => Constraint::Fill(*weight),
        },
        _ => Constraint::Fill(1),
    }
}

fn render_panel(
    frame: &mut Frame<'_>,
    panel: &RenderProxyProjection,
    area: Rect,
    measurements: &mut Vec<ViewportMeasurement>,
) {
    let border = if panel.active {
        Color::Yellow
    } else {
        Color::DarkGray
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(terminal_safe(&panel.title))
        .border_style(Style::default().fg(border));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    measurements.push(ViewportMeasurement {
        dataset: panel.dataset,
        page_size: usize::from(inner.height.max(1)),
    });

    match &panel.content {
        RenderContentProjection::Empty => {}
        RenderContentProjection::Rows(rows) => {
            let items = visible_rows(&rows.rows, rows.viewport, inner.height)
                .map(render_row)
                .collect::<Vec<_>>();
            frame.render_widget(List::new(items), inner);
        }
        RenderContentProjection::Detail(detail) => {
            let lines = detail_lines(&detail.fields);
            frame.render_widget(
                Paragraph::new(visible_lines(lines, detail.viewport, inner.height)),
                inner,
            );
        }
        RenderContentProjection::UnifiedDiff(diff) => {
            let mut lines = diff
                .header
                .iter()
                .map(|line| Line::styled(terminal_safe(line), Style::default().fg(Color::Cyan)))
                .collect::<Vec<_>>();
            for hunk in &diff.hunks {
                lines.push(Line::styled(
                    terminal_safe(&hunk.header),
                    Style::default().fg(Color::Cyan),
                ));
                for line in &hunk.lines {
                    let (marker, color) = match line.kind {
                        DiffLineKind::Addition => ('+', Color::Green),
                        DiffLineKind::Deletion => ('-', Color::Red),
                        DiffLineKind::Context => (' ', Color::Reset),
                        DiffLineKind::Metadata => ('\\', Color::DarkGray),
                    };
                    lines.push(Line::styled(
                        format!(
                            "{:>5} {:>5} {}{}",
                            line.old_line_no
                                .map_or(String::new(), |line| line.to_string()),
                            line.new_line_no
                                .map_or(String::new(), |line| line.to_string()),
                            marker,
                            terminal_safe(&line.text)
                        ),
                        Style::default().fg(color),
                    ));
                }
            }
            frame.render_widget(
                Paragraph::new(visible_lines(lines, diff.viewport, inner.height)),
                inner,
            );
        }
        RenderContentProjection::SideBySideDiff(diff) => {
            let mut lines = diff
                .header
                .iter()
                .map(|line| Line::styled(terminal_safe(line), Style::default().fg(Color::Cyan)))
                .collect::<Vec<_>>();
            let half = usize::from(inner.width.saturating_sub(3) / 2);
            for hunk in &diff.hunks {
                lines.push(Line::styled(
                    terminal_safe(&hunk.header),
                    Style::default().fg(Color::Cyan),
                ));
                for row in &hunk.rows {
                    let left = format_diff_cell(row.left_line_no, row.left_text.as_deref(), half);
                    let right =
                        format_diff_cell(row.right_line_no, row.right_text.as_deref(), half);
                    let color = match (row.left_kind, row.right_kind) {
                        (DiffCellKind::Deleted | DiffCellKind::Modified, _) => Color::Red,
                        (_, DiffCellKind::Added | DiffCellKind::Modified) => Color::Green,
                        _ => Color::Reset,
                    };
                    lines.push(Line::styled(
                        format!("{left:<half$} │ {right:<half$}"),
                        Style::default().fg(color),
                    ));
                }
            }
            frame.render_widget(
                Paragraph::new(visible_lines(lines, diff.viewport, inner.height)),
                inner,
            );
        }
        RenderContentProjection::Interaction(interaction) => {
            let mut lines = Vec::new();
            if let Some(prompt) = &interaction.prompt {
                lines.push(Line::styled(
                    terminal_safe(prompt),
                    Style::default().fg(Color::Cyan),
                ));
            }
            if let Some(input) = &interaction.input {
                lines.push(Line::from(vec![
                    Span::styled("> ", Style::default().fg(Color::Yellow)),
                    Span::raw(terminal_safe(input)),
                ]));
            }
            if !lines.is_empty() && !interaction.lines.is_empty() {
                lines.push(Line::raw(""));
            }
            lines.extend(interaction.lines.iter().map(|item| {
                let key = item
                    .key
                    .as_ref()
                    .map(|key| format!("{:<12}", format_key(key)))
                    .unwrap_or_default();
                Line::styled(
                    format!("{key}{}", terminal_safe(&item.text)),
                    if item.selected {
                        Style::default().add_modifier(Modifier::REVERSED)
                    } else {
                        Style::default()
                    },
                )
            }));
            if let Some(error) = &interaction.error {
                lines.push(Line::styled(
                    terminal_safe(error),
                    Style::default().fg(Color::Red),
                ));
            }
            frame.render_widget(
                Paragraph::new(visible_lines(lines, interaction.viewport, inner.height)),
                inner,
            );
        }
    }
}

fn centered_overlay_area(layout: &UiLayoutProjection, area: Rect) -> Rect {
    let percentage = match layout {
        UiLayoutProjection::Dataset {
            constraint: LayoutConstraint::Percentage(percentage),
            ..
        } => (*percentage).clamp(10, 100),
        _ => 75,
    };
    let width = ((u32::from(area.width) * u32::from(percentage)) / 100)
        .max(1)
        .min(u32::from(area.width)) as u16;
    let height_percentage = percentage.min(80);
    let height = ((u32::from(area.height) * u32::from(height_percentage)) / 100)
        .max(1)
        .min(u32::from(area.height)) as u16;
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

fn visible_rows(
    rows: &[RowProjection],
    viewport: ViewportProjection,
    height: u16,
) -> impl Iterator<Item = &RowProjection> {
    rows.iter().skip(viewport.offset).take(usize::from(height))
}

fn render_row(row: &RowProjection) -> ListItem<'static> {
    let mut spans = vec![Span::styled(
        if row.cursor { "› " } else { "  " },
        Style::default().fg(Color::Yellow),
    )];
    spans.push(Span::raw(if row.selected { "[x] " } else { "[ ] " }));
    spans.push(Span::raw("  ".repeat(row.depth)));
    spans.push(Span::raw(
        row.cells
            .iter()
            .map(|cell| terminal_safe(&cell.text))
            .collect::<Vec<_>>()
            .join("  "),
    ));
    let style = if row.cursor {
        Style::default().add_modifier(Modifier::REVERSED)
    } else if row.selected {
        Style::default().fg(Color::Green)
    } else {
        Style::default()
    };
    ListItem::new(Line::from(spans)).style(style)
}

fn detail_lines(fields: &[CellProjection]) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for field in fields {
        let label = field
            .label
            .as_deref()
            .map(|label| format!("{}: ", terminal_safe(label)))
            .unwrap_or_default();
        let mut values = field.text.lines();
        if let Some(first) = values.next() {
            lines.push(Line::from(vec![
                Span::styled(label, Style::default().fg(Color::Cyan)),
                Span::raw(terminal_safe(first)),
            ]));
        }
        lines.extend(values.map(|line| Line::raw(terminal_safe(line))));
    }
    lines
}

fn visible_lines(
    lines: Vec<Line<'static>>,
    viewport: ViewportProjection,
    height: u16,
) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .skip(viewport.offset)
        .take(usize::from(height))
        .collect()
}

fn format_diff_cell(line: Option<u32>, text: Option<&str>, width: usize) -> String {
    let value = format!(
        "{:>5} {}",
        line.map_or(String::new(), |line| line.to_string()),
        terminal_safe(text.unwrap_or_default())
    );
    truncate_to_width(&value, width)
}

fn truncate_to_width(value: &str, width: usize) -> String {
    if UnicodeWidthStr::width(value) <= width {
        return value.to_owned();
    }
    let mut rendered_width = 0;
    value
        .chars()
        .take_while(|character| {
            let character_width = character.width().unwrap_or(0);
            if rendered_width + character_width > width {
                false
            } else {
                rendered_width += character_width;
                true
            }
        })
        .collect()
}

fn format_key(stroke: &KeyStroke) -> String {
    let mut parts = Vec::new();
    if stroke.modifiers.control {
        parts.push("Ctrl".to_string());
    }
    if stroke.modifiers.alt {
        parts.push("Alt".to_string());
    }
    if stroke.modifiers.shift {
        parts.push("Shift".to_string());
    }
    if stroke.modifiers.super_key {
        parts.push("Super".to_string());
    }
    parts.push(match stroke.code {
        KeyCode::Character(character) => character.to_string(),
        KeyCode::Up => "↑".into(),
        KeyCode::Down => "↓".into(),
        KeyCode::Left => "←".into(),
        KeyCode::Right => "→".into(),
        KeyCode::Home => "Home".into(),
        KeyCode::End => "End".into(),
        KeyCode::PageUp => "PgUp".into(),
        KeyCode::PageDown => "PgDn".into(),
        KeyCode::Enter => "Enter".into(),
        KeyCode::Escape => "Esc".into(),
        KeyCode::Space => "Space".into(),
        KeyCode::Backspace => "Backspace".into(),
        KeyCode::Tab => "Tab".into(),
    });
    parts.join("+")
}

/// Git metadata and file contents are untrusted terminal input. C0 controls,
/// escape sequences and bidi overrides must never reach the backend verbatim.
fn terminal_safe(value: &str) -> String {
    let mut safe = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\t' => safe.push_str("    "),
            '\r' => {}
            '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' => safe.push('�'),
            character if character.is_control() => safe.push('�'),
            character => safe.push(character),
        }
    }
    safe
}

#[cfg(test)]
mod tests {
    use bevy_ecs::prelude::Entity;
    use pitui_data::{
        CellProjection, FieldId, FooterProjection, InteractionLineProjection,
        InteractionProjection, RenderProxyId, RendererKind, ResolvedKeyBinding, RowsProjection,
        StatusProjection, StyleSpec,
    };
    use ratatui::{Terminal, backend::TestBackend};

    use super::*;

    #[test]
    fn renders_only_ui_frame_data_and_returns_viewport_measurements() {
        let dataset = Entity::PLACEHOLDER;
        let binding = ResolvedKeyBinding {
            stroke: KeyStroke::control('c'),
            label: "More".into(),
            action: ResolvedKeyAction::EnterChord(vec![KeyStroke::control('c')]),
        };
        let ui = UiFrame {
            generation: 1,
            layout: UiLayoutProjection::Dataset {
                constraint: LayoutConstraint::Fill(1),
                focusable: true,
                panel: Box::new(RenderProxyProjection {
                    dataset,
                    proxy: RenderProxyId::from("test"),
                    renderer: RendererKind::List,
                    active: true,
                    title: "Commits".into(),
                    style: StyleSpec::default(),
                    content: RenderContentProjection::Rows(RowsProjection {
                        rows: vec![RowProjection {
                            entity: dataset,
                            depth: 0,
                            cells: vec![CellProjection {
                                field: FieldId::CommitSubject,
                                label: None,
                                text: "safe\u{1b}[31m subject".into(),
                            }],
                            cursor: true,
                            selected: false,
                        }],
                        viewport: ViewportProjection::default(),
                    }),
                }),
            },
            footer: FooterProjection {
                bindings: vec![binding],
            },
            status: StatusProjection {
                items: vec!["repo".into(), "main".into()],
            },
        };
        let backend = TestBackend::new(80, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut measurements = Vec::new();
        terminal
            .draw(|frame| measurements = render(frame, &ui))
            .unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("Commits"));
        assert!(text.contains("subject"));
        assert!(text.contains("Ctrl+c More"));
        assert!(!text.contains('\u{1b}'));
        assert_eq!(measurements.len(), 1);
        assert_eq!(measurements[0].dataset, dataset);
    }

    #[test]
    fn diff_cells_are_truncated_on_unicode_display_boundaries() {
        assert_eq!(truncate_to_width("ab界cd", 4), "ab界");
        assert_eq!(truncate_to_width("ab界cd", 3), "ab");
    }

    #[test]
    fn interaction_overlay_keeps_the_underlay_and_centers_context_data() {
        let mut world = bevy_ecs::world::World::new();
        let underlay = world.spawn_empty().id();
        let interaction = world.spawn_empty().id();
        let ui = UiFrame {
            generation: 1,
            layout: UiLayoutProjection::Overlay(vec![
                UiLayoutProjection::Dataset {
                    constraint: LayoutConstraint::Fill(1),
                    focusable: true,
                    panel: Box::new(RenderProxyProjection {
                        dataset: underlay,
                        proxy: RenderProxyId::from("underlay"),
                        renderer: RendererKind::List,
                        active: false,
                        title: "Commits".into(),
                        style: StyleSpec::default(),
                        content: RenderContentProjection::Empty,
                    }),
                },
                UiLayoutProjection::Dataset {
                    constraint: LayoutConstraint::Percentage(60),
                    focusable: true,
                    panel: Box::new(RenderProxyProjection {
                        dataset: interaction,
                        proxy: RenderProxyId::from("interaction"),
                        renderer: RendererKind::Confirmation,
                        active: true,
                        title: "Command".into(),
                        style: StyleSpec::default(),
                        content: RenderContentProjection::Interaction(InteractionProjection {
                            title: "Command".into(),
                            prompt: Some("Type a command name".into()),
                            input: Some("qui".into()),
                            lines: vec![InteractionLineProjection {
                                key: None,
                                text: "quit  Quit".into(),
                                selected: true,
                            }],
                            error: None,
                            viewport: ViewportProjection::default(),
                        }),
                    }),
                },
            ]),
            footer: FooterProjection::default(),
            status: StatusProjection::default(),
        };
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut measurements = Vec::new();
        terminal
            .draw(|frame| measurements = render(frame, &ui))
            .unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("Commits"));
        assert!(text.contains("Command"));
        assert!(text.contains("quit  Quit"));
        assert_eq!(measurements.len(), 2);
        assert_eq!(measurements[1].dataset, interaction);
    }
}
