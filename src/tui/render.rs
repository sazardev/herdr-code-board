//! Drawing the board.

use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use super::form::{placement_summary, Field, Flag};
use super::state::{App, Mode};
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
        Mode::Form => draw_form(frame, app, theme),
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

fn draw_lanes(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    // The selected lane gets double width so long titles stay readable even with
    // nine lanes on screen.
    let weights: Vec<Constraint> = Column::ALL
        .iter()
        .enumerate()
        .map(|(i, _)| Constraint::Fill(if i == app.lane { 2 } else { 1 }))
        .collect();
    let columns = Layout::horizontal(weights).split(area);

    for (i, lane) in Column::ALL.iter().enumerate() {
        let selected = i == app.lane;
        let cards = app.lane_cards(*lane);
        let color = theme.lane(*lane);

        let title = Line::from(vec![
            Span::styled(
                format!(" {} ", lane.title()),
                Style::default().fg(color).bold(),
            ),
            Span::styled(
                format!("{} ", cards.len()),
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
            columns[i],
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
        "hjkl move · HL shift lane · space queue · enter jump to pane · n new · e edit · x cancel · r retry · d delete · / search · tab repo · s sync · ? help · q quit".to_string()
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
    let area = popup(frame, 74, (Field::ALL.len() + 4) as u16);
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
    for (i, field) in Field::ALL.iter().enumerate() {
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
        " tab/↑↓ field · ←→ choose · space toggle · enter save · esc cancel",
        Style::default().fg(theme.muted),
    )));

    frame.render_widget(Paragraph::new(lines).block(block), area);
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
