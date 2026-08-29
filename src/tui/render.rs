//! Drawing the board.

use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use super::form::{placement_summary, Field, Flag};
use super::state::{App, Mode, PickerTarget};
use super::theme::Theme;
use crate::app::ago;
use crate::model::Column;

pub fn draw(frame: &mut Frame, app: &App, theme: &Theme) {
    let [header, body, detail, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(6),
        Constraint::Length(8),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    draw_header(frame, header, app, theme);
    draw_lanes(frame, body, app, theme);
    draw_detail(frame, detail, app, theme);
    draw_status(frame, status, app, theme);

    match &app.mode {
        // The form stays visible behind its picker, so you keep your place.
        Mode::Form => draw_form(frame, app, theme),
        Mode::RepoPicker => {
            if app.form.is_some() {
                draw_form(frame, app, theme);
            }
            draw_picker(frame, app, theme);
        }
        Mode::Help => draw_help(frame, theme),
        Mode::Confirm(question) => draw_confirm(frame, question, theme),
        _ => {}
    }
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let live = app.cards.iter().filter(|c| c.column.is_live()).count();
    let mut spans = vec![
        Span::styled(" code board ", Style::default().fg(theme.accent).bold()),
        Span::styled(
            format!(
                "{} cards · {live} live · {}",
                app.cards.len(),
                app.filter_label()
            ),
            Style::default().fg(theme.muted),
        ),
    ];
    if !app.search.is_empty() || app.mode == Mode::Search {
        spans.push(Span::styled(
            format!("  /{}", app.search),
            Style::default().fg(theme.waiting),
        ));
        if app.mode == Mode::Search {
            spans.push(Span::styled("_", Style::default().fg(theme.waiting)));
        }
    }
    frame.render_widget(Line::from(spans), area);
}

/// A lane narrower than this cannot show a card title, so it is worse than not
/// showing the lane at all.
const MIN_LANE_WIDTH: u16 = 18;

/// Which slice of the lanes to render, given the space available.
///
/// Nine lanes in a narrow split pane come out three characters wide and useless.
/// So render as many whole lanes as fit and scroll the window to keep the
/// selected one visible, the way any kanban board does.
pub fn lane_window(width: u16, lane_count: usize, selected: usize) -> (usize, usize) {
    let fits = ((width / MIN_LANE_WIDTH) as usize).clamp(1, lane_count);
    if fits >= lane_count {
        return (0, lane_count);
    }
    // Centre the selection in the window, then clamp to the ends.
    let half = fits / 2;
    let start = selected.saturating_sub(half).min(lane_count - fits);
    (start, fits)
}

fn draw_lanes(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let (start, count) = lane_window(area.width, Column::ALL.len(), app.lane);
    let lanes = &Column::ALL[start..start + count];

    // The selected lane gets double width so long titles stay readable.
    let weights: Vec<Constraint> = (start..start + count)
        .map(|i| Constraint::Fill(if i == app.lane { 2 } else { 1 }))
        .collect();
    let columns = Layout::horizontal(weights).split(area);

    for (slot, lane) in lanes.iter().enumerate() {
        let i = start + slot;
        let selected = i == app.lane;
        let cards = app.lane_cards(*lane);
        let color = theme.lane(*lane);

        // Tell the reader when lanes are scrolled off an edge.
        let edge = if slot == 0 && start > 0 {
            "\u{2039}"
        } else if slot + 1 == count && start + count < Column::ALL.len() {
            "\u{203a}"
        } else {
            ""
        };
        let title = Line::from(vec![
            Span::styled(
                format!(" {} ", lane.title()),
                Style::default().fg(color).bold(),
            ),
            Span::styled(
                format!("{}{} ", cards.len(), edge),
                Style::default().fg(theme.muted),
            ),
        ]);
        let block = Block::bordered()
            .border_type(if selected {
                BorderType::Thick
            } else {
                BorderType::Plain
            })
            .border_style(Style::default().fg(if selected { color } else { theme.muted }))
            .title(title);

        let items: Vec<ListItem> = cards
            .iter()
            .map(|card| {
                let mut lines = vec![Line::from(Span::styled(
                    card.title.clone(),
                    Style::default().fg(color),
                ))];
                let mut meta = vec![Span::styled(
                    app.repo_name(card).to_string(),
                    Style::default().fg(theme.muted),
                )];
                meta.push(Span::styled(
                    format!(" · {}", card.agent_kind),
                    Style::default().fg(theme.muted),
                ));
                if lane.is_live() {
                    meta.push(Span::styled(
                        format!(" · {}", ago(card.status_since)),
                        Style::default().fg(theme.muted),
                    ));
                }
                lines.push(Line::from(meta));
                ListItem::new(lines)
            })
            .collect();

        let mut state = ListState::default();
        if selected && !cards.is_empty() {
            state.select(Some(app.cursor[i].min(cards.len() - 1)));
        }

        frame.render_stateful_widget(
            List::new(items)
                .block(block)
                .highlight_style(Style::default().add_modifier(Modifier::REVERSED)),
            columns[slot],
            &mut state,
        );
    }
}

fn draw_detail(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let block = Block::bordered()
        .border_style(Style::default().fg(theme.muted))
        .title(Span::styled(" card ", Style::default().fg(theme.muted)));

    let Some(card) = app.selected() else {
        frame.render_widget(
            Paragraph::new("no card selected — press n to add one, ? for help").block(block),
            area,
        );
        return;
    };

    let mut lines = vec![Line::from(vec![
        Span::styled(
            card.title.clone(),
            Style::default().fg(theme.lane(card.column)).bold(),
        ),
        Span::styled(
            format!("  {} for {}", card.column, ago(card.status_since)),
            Style::default().fg(theme.muted),
        ),
    ])];

    let mut meta = vec![Span::styled(
        format!(
            "{} · {}{} · {}",
            app.repo_name(card),
            card.agent_kind,
            card.model
                .as_deref()
                .map(|m| format!("/{m}"))
                .unwrap_or_default(),
            placement_summary(&card.placement),
        ),
        Style::default().fg(theme.muted),
    )];
    if let Some(pane) = &card.binding.pane_id {
        meta.push(Span::styled(
            format!(" · {pane}"),
            Style::default().fg(theme.accent),
        ));
    }
    if card.attempts > 0 {
        meta.push(Span::styled(
            format!(" · attempt {}", card.attempts),
            Style::default().fg(theme.muted),
        ));
    }
    lines.push(Line::from(meta));

    if !card.tags.is_empty() {
        lines.push(Line::from(Span::styled(
            card.tags.join(", "),
            Style::default().fg(theme.muted),
        )));
    }
    if let Some(err) = &card.last_error {
        lines.push(Line::from(Span::styled(
            err.clone(),
            Style::default().fg(theme.failed),
        )));
    }
    if !card.prompt.trim().is_empty() {
        lines.push(Line::from(Span::styled(
            card.prompt.clone(),
            Style::default().fg(theme.muted),
        )));
    }

    frame.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: true }),
        area,
    );
}

fn draw_status(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let text = if app.status.is_empty() {
        "hjkl move · HL shift lane · space queue · enter jump to pane · n new · e edit · t repos · x cancel · r retry · d delete · / search · s sync · ? help · q quit".to_string()
    } else {
        app.status.clone()
    };
    let color = if app.status.is_empty() {
        theme.muted
    } else {
        theme.waiting
    };
    frame.render_widget(
        Paragraph::new(Span::styled(text, Style::default().fg(color))),
        area,
    );
}

/// A centred box `width` x `height` cells, clipped to the frame.
fn popup(frame: &Frame, width: u16, height: u16) -> Rect {
    let area = frame.area();
    let [row] = Layout::vertical([Constraint::Length(height.min(area.height))])
        .flex(Flex::Center)
        .areas(area);
    let [cell] = Layout::horizontal([Constraint::Length(width.min(area.width))])
        .flex(Flex::Center)
        .areas(row);
    cell
}

fn draw_form(frame: &mut Frame, app: &App, theme: &Theme) {
    let Some(form) = &app.form else { return };
    let fields = form.fields();
    let area = popup(frame, 78, (fields.len() + 4) as u16);
    frame.render_widget(Clear, area);

    let title = if form.editing.is_some() {
        " edit card "
    } else {
        " new card "
    };
    let block = Block::bordered()
        .border_type(BorderType::Thick)
        .border_style(Style::default().fg(theme.accent))
        .title(Span::styled(
            title,
            Style::default().fg(theme.accent).bold(),
        ));

    let mut lines = Vec::new();
    for (i, field) in fields.iter().enumerate() {
        let active = i == form.field;
        let value = match field {
            Field::Title => form.title.clone(),
            Field::Prompt => form.prompt.clone(),
            Field::Repo => form
                .repos
                .get(form.repo)
                .map(|(n, _)| n.clone())
                .unwrap_or_default(),
            Field::Agent => form.agent_kind(),
            Field::Model => form.model.clone(),
            Field::Placement => form.placement_name().to_string(),
            Field::Branch => form.branch.clone(),
            Field::Base => match form.base_name() {
                Some(b) => b.to_string(),
                None => "(repo's current branch)".to_string(),
            },
            Field::Tags => form.tags.clone(),
            Field::Args => form.args.clone(),
            Field::Flags => Flag::ALL
                .iter()
                .enumerate()
                .map(|(fi, flag)| {
                    let mark = if form.flag_value(*flag) { "x" } else { " " };
                    let cursor = if active && fi == form.flag { ">" } else { " " };
                    format!("{cursor}[{mark}] {}", flag.label())
                })
                .collect::<Vec<_>>()
                .join("  "),
        };
        let mut spans = vec![Span::styled(
            format!(" {:<10} ", field.label()),
            Style::default().fg(if active { theme.accent } else { theme.muted }),
        )];
        spans.push(Span::styled(
            value,
            Style::default().fg(if active {
                theme.accent
            } else {
                ratatui::style::Color::Reset
            }),
        ));
        if active && field.is_text() {
            spans.push(Span::styled("_", Style::default().fg(theme.accent)));
        }
        lines.push(Line::from(spans));
    }
    lines.push(Line::from(Span::styled(
        " tab/↑↓ field · ←→ choose · space toggle · enter saves (on repo: picks) · esc cancel",
        Style::default().fg(theme.muted),
    )));

    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn draw_picker(frame: &mut Frame, app: &App, theme: &Theme) {
    let Some(picker) = &app.picker else { return };
    let matches = picker.matches();

    let rows = matches.len().clamp(1, 14) as u16;
    let area = popup(frame, 84, rows + 4);
    frame.render_widget(Clear, area);

    let heading = match picker.target {
        PickerTarget::Form => " run this card in… ",
        PickerTarget::Filter => " repositories ",
    };
    let block = Block::bordered()
        .border_type(BorderType::Thick)
        .border_style(Style::default().fg(theme.accent))
        .title(Span::styled(
            heading,
            Style::default().fg(theme.accent).bold(),
        ))
        .title_bottom(Span::styled(
            format!(
                " {}/{} · type to filter · ↑↓ move · enter pick · esc back ",
                matches.len(),
                picker.items.len()
            ),
            Style::default().fg(theme.muted),
        ));

    let [search, list] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(block.inner(area));
    frame.render_widget(block, area);

    frame.render_widget(
        Line::from(vec![
            Span::styled(" › ", Style::default().fg(theme.accent)),
            Span::raw(picker.query.clone()),
            Span::styled("_", Style::default().fg(theme.accent)),
        ]),
        search,
    );

    let items: Vec<ListItem> = matches
        .iter()
        .map(|choice| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    if choice.tracked { " ● " } else { " ○ " },
                    Style::default().fg(if choice.tracked {
                        theme.done
                    } else {
                        theme.muted
                    }),
                ),
                Span::styled(
                    format!("{:<26}", truncate(&choice.name, 25)),
                    Style::default().fg(theme.accent),
                ),
                Span::styled(
                    format!(
                        "{:<24}",
                        truncate(choice.branch.as_deref().unwrap_or("(detached)"), 23)
                    ),
                    Style::default().fg(theme.waiting),
                ),
                Span::styled(
                    home_relative(&choice.path),
                    Style::default().fg(theme.muted),
                ),
            ]))
        })
        .collect();

    let mut state = ListState::default();
    if !matches.is_empty() {
        state.select(Some(picker.cursor.min(matches.len() - 1)));
    }
    frame.render_stateful_widget(
        List::new(items).highlight_style(Style::default().add_modifier(Modifier::REVERSED)),
        list,
        &mut state,
    );
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    format!(
        "{}…",
        s.chars().take(max.saturating_sub(1)).collect::<String>()
    )
}

/// `/home/you/Documents/x` reads better as `~/Documents/x`.
fn home_relative(path: &std::path::Path) -> String {
    let text = path.to_string_lossy().to_string();
    match std::env::var("HOME") {
        Ok(home) if text.starts_with(&home) => format!("~{}", &text[home.len()..]),
        _ => text,
    }
}

const HELP: &[(&str, &str)] = &[
    ("h l ← →", "move between lanes"),
    ("j k ↑ ↓", "move between cards"),
    ("g G", "first / last card in the lane"),
    ("H L", "shift the card one lane over"),
    ("space", "queue a card, or take it back off the queue"),
    ("enter", "focus the herdr pane the card runs in"),
    ("n", "new card"),
    ("e", "edit the selected card"),
    ("x", "cancel a running card and release its pane"),
    ("r", "re-dispatch the card from scratch"),
    ("d", "delete the card (asks first)"),
    ("/", "search titles, prompts and tags"),
    ("t", "pick a repository — scans your disk for checkouts"),
    ("tab", "cycle the repo filter"),
    ("s", "re-import .herdr-board.toml from tracked repos"),
    ("R", "reload from the database"),
    ("q esc", "quit"),
];

fn draw_help(frame: &mut Frame, theme: &Theme) {
    let area = popup(frame, 62, (HELP.len() + 2) as u16);
    frame.render_widget(Clear, area);
    let lines: Vec<Line> = HELP
        .iter()
        .map(|(keys, what)| {
            Line::from(vec![
                Span::styled(format!(" {keys:<10} "), Style::default().fg(theme.accent)),
                Span::raw(*what),
            ])
        })
        .collect();
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .border_type(BorderType::Thick)
                .border_style(Style::default().fg(theme.accent))
                .title(Span::styled(
                    " keys ",
                    Style::default().fg(theme.accent).bold(),
                )),
        ),
        area,
    );
}

fn draw_confirm(frame: &mut Frame, question: &str, theme: &Theme) {
    let area = popup(frame, (question.len() as u16 + 8).max(30), 4);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(question),
            Line::from(Span::styled(
                "y to confirm, anything else to cancel",
                Style::default().fg(theme.muted),
            )),
        ])
        .alignment(Alignment::Center)
        .block(
            Block::bordered()
                .border_type(BorderType::Thick)
                .border_style(Style::default().fg(theme.blocked)),
        ),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wide_board_shows_every_lane() {
        assert_eq!(lane_window(200, 9, 0), (0, 9));
        assert_eq!(lane_window(9 * MIN_LANE_WIDTH, 9, 4), (0, 9));
    }

    /// A nine-lane board in a 47-column split pane gives three-character lanes.
    /// Show fewer, wider lanes instead.
    #[test]
    fn a_narrow_board_shows_a_window_around_the_selection() {
        let (start, count) = lane_window(47, 9, 0);
        assert_eq!(count, 2);
        assert_eq!(start, 0);

        let (start, count) = lane_window(47, 9, 4);
        assert_eq!(count, 2);
        assert!(
            (start..start + count).contains(&4),
            "the selected lane must be inside the window"
        );
    }

    #[test]
    fn the_window_never_runs_off_either_end() {
        for selected in 0..9 {
            let (start, count) = lane_window(90, 9, selected);
            assert!(start + count <= 9, "selected {selected}");
            assert!(
                (start..start + count).contains(&selected),
                "selected {selected} fell outside {start}..{}",
                start + count
            );
        }
        assert_eq!(lane_window(90, 9, 8).0, 9 - (90 / MIN_LANE_WIDTH) as usize);
    }

    #[test]
    fn a_terminal_too_narrow_for_even_one_lane_still_renders_one() {
        let (start, count) = lane_window(5, 9, 3);
        assert_eq!(count, 1);
        assert_eq!(start, 3, "and it is the one you selected");
    }
}
