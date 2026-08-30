//! Drawing the board, in herdr's own idiom.
//!
//! Herdr's sidebar is flat: rows of text, a coloured background on the active
//! one, dim subtext underneath, no boxes. The board follows that rather than
//! drawing a grid of framed panels, so it reads as part of herdr and not as
//! something running inside it. Only the modals get borders, because herdr's
//! popups do too.

use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap,
};
use ratatui::Frame;

use super::form::{placement_summary, Field, Flag};
use super::state::{chain_triggers, App, ChainStage, Mode, PickerTarget};
use super::theme::Theme;
use crate::app::ago;
use crate::engine::present::glyph;
use crate::model::Column;

/// A lane narrower than this cannot show a card title, so it is worse than not
/// showing the lane at all.
const MIN_LANE_WIDTH: u16 = 18;

pub fn draw(frame: &mut Frame, app: &App, theme: &Theme) {
    let area = frame.area();
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.panel_bg).fg(theme.text)),
        area,
    );

    let detail_height = if area.height >= 20 { 7 } else { 0 };
    let [header, body, detail, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(4),
        Constraint::Length(detail_height),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_header(frame, header, app, theme);
    draw_lanes(frame, body, app, theme);
    if detail_height > 0 {
        draw_detail(frame, detail, app, theme);
    }
    draw_status(frame, status, app, theme);

    match &app.mode {
        Mode::Form => draw_form(frame, app, theme),
        Mode::RepoPicker => {
            if app.form.is_some() {
                draw_form(frame, app, theme);
            }
            draw_picker(frame, app, theme);
        }
        Mode::Chain => draw_chain(frame, app, theme),
        Mode::Detail => draw_detail_overlay(frame, app, theme),
        Mode::Help => draw_help(frame, theme),
        Mode::Confirm(question) => draw_confirm(frame, question, theme),
        _ => {}
    }
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let live = app.cards.iter().filter(|c| c.column.is_live()).count();
    let mut left = vec![
        // The accent bar herdr uses to mark what you are looking at.
        Span::styled("▌", Style::default().fg(theme.accent)),
        Span::styled(
            " code board ",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("· ", Style::default().fg(theme.surface)),
        Span::styled(app.filter_label(), Style::default().fg(theme.text)),
    ];
    if app.mode == Mode::Search || !app.search.is_empty() {
        left.push(Span::styled(
            format!("  /{}", app.search),
            Style::default().fg(theme.yellow),
        ));
        if app.mode == Mode::Search {
            left.push(Span::styled("▏", Style::default().fg(theme.yellow)));
        }
    }

    let right = if live > 0 {
        format!("{} cards · {live} running ", app.cards.len())
    } else {
        format!("{} cards ", app.cards.len())
    };
    let pad = area
        .width
        .saturating_sub(width_of(&left) + right.chars().count() as u16);
    left.push(Span::raw(" ".repeat(pad as usize)));
    left.push(Span::styled(right, Style::default().fg(theme.overlay)));
    frame.render_widget(Line::from(left), area);
}

fn width_of(spans: &[Span]) -> u16 {
    spans.iter().map(|s| s.content.chars().count() as u16).sum()
}

/// Which slice of the lanes to render, given the space available.
pub fn lane_window(width: u16, lane_count: usize, selected: usize) -> (usize, usize) {
    let fits = ((width / MIN_LANE_WIDTH) as usize).clamp(1, lane_count);
    if fits >= lane_count {
        return (0, lane_count);
    }
    let half = fits / 2;
    let start = selected.saturating_sub(half).min(lane_count - fits);
    (start, fits)
}

fn draw_lanes(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let (start, count) = lane_window(area.width, Column::ALL.len(), app.lane);
    let lanes = &Column::ALL[start..start + count];
    let weights: Vec<Constraint> = (start..start + count)
        .map(|i| Constraint::Fill(if i == app.lane { 2 } else { 1 }))
        .collect();
    let columns = Layout::horizontal(weights).split(area);

    for (slot, lane) in lanes.iter().enumerate() {
        let i = start + slot;
        let selected = i == app.lane;
        let cards = app.lane_cards(*lane);
        let color = theme.lane(*lane);
        let cell = columns[slot];
        if cell.height < 2 {
            continue;
        }

        let [head, rule, list] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .areas(cell);

        // Lane heading: name in its colour, count dim, an arrow when there are
        // more lanes off the edge.
        let edge = if slot == 0 && start > 0 {
            "‹"
        } else if slot + 1 == count && start + count < Column::ALL.len() {
            "›"
        } else {
            " "
        };
        let heading = Style::default().fg(if selected { color } else { theme.overlay });
        frame.render_widget(
            Line::from(vec![
                Span::styled(
                    format!(" {} ", lane.title().to_uppercase()),
                    if selected {
                        heading.add_modifier(Modifier::BOLD)
                    } else {
                        heading
                    },
                ),
                Span::styled(
                    format!("{}{edge}", cards.len()),
                    Style::default().fg(theme.overlay),
                ),
            ]),
            head,
        );
        // A thin rule instead of a box, brighter under the lane you are in.
        frame.render_widget(
            Line::from(Span::styled(
                "─".repeat(rule.width.saturating_sub(1) as usize),
                Style::default().fg(if selected { color } else { theme.surface }),
            )),
            rule,
        );

        let items: Vec<ListItem> = cards
            .iter()
            .map(|card| {
                let mut meta = vec![Span::styled(
                    format!("   {}", app.repo_name(card)),
                    Style::default().fg(theme.overlay),
                )];
                meta.push(Span::styled(
                    format!(" · {}", card.agent_kind),
                    Style::default().fg(theme.overlay),
                ));
                if lane.is_live() {
                    meta.push(Span::styled(
                        format!(" · {}", ago(card.status_since)),
                        Style::default().fg(theme.overlay),
                    ));
                }
                if let Some(session) = app.foreign_session(card) {
                    meta.push(Span::styled(
                        format!(" @{session}"),
                        Style::default().fg(theme.yellow),
                    ));
                }
                ListItem::new(vec![
                    Line::from(vec![
                        Span::styled(
                            format!(" {} ", glyph(card.column)),
                            Style::default().fg(color),
                        ),
                        Span::styled(card.title.clone(), Style::default().fg(theme.text)),
                    ]),
                    Line::from(meta),
                ])
            })
            .collect();

        let mut state = ListState::default();
        if selected && !cards.is_empty() {
            state.select(Some(app.cursor[i].min(cards.len() - 1)));
        }
        frame.render_stateful_widget(
            List::new(items)
                // Herdr marks the active row with a background, not by inverting.
                .highlight_style(Style::default().bg(theme.active_row_bg))
                .highlight_symbol(""),
            list,
            &mut state,
        );
    }
}

fn draw_detail(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let [rule, body] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
    frame.render_widget(
        Line::from(Span::styled(
            "─".repeat(rule.width as usize),
            Style::default().fg(theme.surface),
        )),
        rule,
    );

    let Some(card) = app.selected() else {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                " nothing selected — a adds a card, ? shows the keys",
                Style::default().fg(theme.overlay),
            ))),
            body,
        );
        return;
    };

    let mut lines = vec![Line::from(vec![
        Span::styled(
            format!(" {} ", glyph(card.column)),
            Style::default().fg(theme.lane(card.column)),
        ),
        Span::styled(
            card.title.clone(),
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {} for {}", card.column, ago(card.status_since)),
            Style::default().fg(theme.overlay),
        ),
    ])];

    let mut meta = vec![Span::styled(
        format!(
            "   {} · {}{} · {}",
            app.repo_name(card),
            card.agent_kind,
            card.model
                .as_deref()
                .map(|m| format!("/{m}"))
                .unwrap_or_default(),
            placement_summary(&card.placement),
        ),
        Style::default().fg(theme.subtext),
    )];
    if let Some(pane) = &card.binding.pane_id {
        meta.push(Span::styled(
            format!(" · {pane}"),
            Style::default().fg(theme.accent),
        ));
    }
    if let Some(session) = app.foreign_session(card) {
        meta.push(Span::styled(
            format!(" · @{session}"),
            Style::default().fg(theme.yellow),
        ));
    }
    if card.attempts > 1 {
        meta.push(Span::styled(
            format!(" · attempt {}", card.attempts),
            Style::default().fg(theme.overlay),
        ));
    }
    lines.push(Line::from(meta));

    if let Some(err) = &card.last_error {
        lines.push(Line::from(Span::styled(
            format!("   {err}"),
            Style::default().fg(theme.red),
        )));
    }
    if !card.prompt.trim().is_empty() {
        lines.push(Line::from(Span::styled(
            format!("   {}", card.prompt.replace('\n', " ")),
            Style::default().fg(theme.subtext),
        )));
    }

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), body);
}

/// The key hints herdr-style: key in accent, meaning dim.
const HINTS: &[(&str, &str)] = &[
    ("a", "add"),
    ("⏎", "pane"),
    ("v", "detail"),
    ("c", "chain"),
    ("e", "edit"),
    ("t", "repos"),
    ("?", "help"),
];

fn draw_status(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    if app.mode == Mode::QuickAdd {
        frame.render_widget(
            Line::from(vec![
                Span::styled("▌", Style::default().fg(theme.green)),
                Span::styled(
                    format!(" queue in {} ", app.filter_label()),
                    Style::default()
                        .fg(theme.green)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(app.quick.clone(), Style::default().fg(theme.text)),
                Span::styled("▏", Style::default().fg(theme.green)),
            ]),
            area,
        );
        return;
    }

    if !app.status.is_empty() {
        frame.render_widget(
            Line::from(vec![
                Span::styled("▌", Style::default().fg(theme.yellow)),
                Span::styled(
                    format!(" {}", app.status),
                    Style::default().fg(theme.subtext),
                ),
            ]),
            area,
        );
        return;
    }

    let mut spans = vec![Span::raw(" ")];
    for (key, what) in HINTS {
        spans.push(Span::styled(
            format!(" {key}"),
            Style::default().fg(theme.accent),
        ));
        spans.push(Span::styled(
            format!(" {what}"),
            Style::default().fg(theme.overlay),
        ));
    }
    frame.render_widget(Line::from(spans), area);
}

// ---------------------------------------------------------------- modals

/// A centred box, clipped to the frame.
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

/// Herdr's popups: a rounded border in the accent, a dim hint along the bottom.
fn modal(title: String, hint: String, theme: &Theme) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.surface))
        .style(Style::default().bg(theme.panel_bg))
        .title(Span::styled(
            format!(" {title} "),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Span::styled(
            format!(" {hint} "),
            Style::default().fg(theme.overlay),
        ))
}

fn draw_form(frame: &mut Frame, app: &App, theme: &Theme) {
    let Some(form) = &app.form else { return };
    let fields = form.fields();
    let area = popup(frame, 78, (fields.len() + 3) as u16);
    frame.render_widget(Clear, area);

    let block = modal(
        if form.editing.is_some() {
            "edit card".into()
        } else {
            "new card".into()
        },
        "tab field · ←→ choose · space toggle · enter save · esc cancel".into(),
        theme,
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

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
            Field::Base => form
                .base_name()
                .map(str::to_string)
                .unwrap_or_else(|| "the repo's current branch".into()),
            Field::Tags => form.tags.clone(),
            Field::Args => form.args.clone(),
            Field::Flags => Flag::ALL
                .iter()
                .enumerate()
                .map(|(fi, flag)| {
                    let mark = if form.flag_value(*flag) { "x" } else { " " };
                    let cursor = if active && fi == form.flag {
                        "▸"
                    } else {
                        " "
                    };
                    format!("{cursor}[{mark}] {}", flag.label())
                })
                .collect::<Vec<_>>()
                .join(" "),
        };
        let mut spans = vec![
            Span::styled(
                if active { "▌" } else { " " },
                Style::default().fg(theme.accent),
            ),
            Span::styled(
                format!(" {:<10} ", field.label()),
                Style::default().fg(if active { theme.accent } else { theme.overlay }),
            ),
            Span::styled(value, Style::default().fg(theme.text)),
        ];
        if active && field.is_text() {
            spans.push(Span::styled("▏", Style::default().fg(theme.accent)));
        }
        lines.push(Line::from(spans));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_picker(frame: &mut Frame, app: &App, theme: &Theme) {
    let Some(picker) = &app.picker else { return };
    let matches = picker.matches();
    let area = popup(frame, 84, matches.len().clamp(1, 14) as u16 + 4);
    frame.render_widget(Clear, area);

    let block = modal(
        match picker.target {
            PickerTarget::Form => "run this card in…".into(),
            PickerTarget::Filter => "repositories".into(),
        },
        format!(
            "{}/{} · type to filter · ↑↓ move · enter pick · esc back",
            matches.len(),
            picker.items.len()
        ),
        theme,
    );
    let [search, list] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(block.inner(area));
    frame.render_widget(block, area);
    frame.render_widget(
        Line::from(vec![
            Span::styled(" ▸ ", Style::default().fg(theme.accent)),
            Span::styled(picker.query.clone(), Style::default().fg(theme.text)),
            Span::styled("▏", Style::default().fg(theme.accent)),
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
                        theme.green
                    } else {
                        theme.overlay
                    }),
                ),
                Span::styled(
                    format!("{:<26}", truncate(&choice.name, 25)),
                    Style::default().fg(theme.text),
                ),
                Span::styled(
                    format!(
                        "{:<24}",
                        truncate(choice.branch.as_deref().unwrap_or("(detached)"), 23)
                    ),
                    Style::default().fg(theme.yellow),
                ),
                Span::styled(
                    home_relative(&choice.path),
                    Style::default().fg(theme.overlay),
                ),
            ]))
        })
        .collect();
    let mut state = ListState::default();
    if !matches.is_empty() {
        state.select(Some(picker.cursor.min(matches.len() - 1)));
    }
    frame.render_stateful_widget(
        List::new(items).highlight_style(Style::default().bg(theme.active_row_bg)),
        list,
        &mut state,
    );
}

fn draw_chain(frame: &mut Frame, app: &App, theme: &Theme) {
    let Some(chain) = &app.chain else { return };
    match chain.stage {
        ChainStage::PickCard => {
            let matches = chain.matches();
            let area = popup(frame, 70, matches.len().clamp(1, 12) as u16 + 4);
            frame.render_widget(Clear, area);
            let block = modal(
                format!("after “{}”, run…", truncate(&chain.from_title, 32)),
                "type to filter · ↑↓ move · enter pick · esc cancel".into(),
                theme,
            );
            let [search, list] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)])
                .areas(block.inner(area));
            frame.render_widget(block, area);
            frame.render_widget(
                Line::from(vec![
                    Span::styled(" ▸ ", Style::default().fg(theme.accent)),
                    Span::styled(chain.query.clone(), Style::default().fg(theme.text)),
                    Span::styled("▏", Style::default().fg(theme.accent)),
                ]),
                search,
            );
            let items: Vec<ListItem> = matches
                .iter()
                .map(|(_, title)| {
                    ListItem::new(Span::styled(
                        format!(" {}", truncate(title, 60)),
                        Style::default().fg(theme.text),
                    ))
                })
                .collect();
            let mut state = ListState::default();
            if !matches.is_empty() {
                state.select(Some(chain.cursor.min(matches.len() - 1)));
            }
            frame.render_stateful_widget(
                List::new(items).highlight_style(Style::default().bg(theme.active_row_bg)),
                list,
                &mut state,
            );
        }
        ChainStage::PickTrigger => {
            let triggers = chain_triggers();
            let area = popup(frame, 62, triggers.len() as u16 + 3);
            frame.render_widget(Clear, area);
            let target = chain
                .chosen
                .as_ref()
                .map(|(_, t)| t.as_str())
                .unwrap_or("it");
            let block = modal(
                format!("run “{}” …", truncate(target, 32)),
                "↑↓ move · enter confirm · esc back".into(),
                theme,
            );
            let items: Vec<ListItem> = triggers
                .iter()
                .map(|(label, _)| {
                    ListItem::new(Span::styled(
                        format!(" {label}"),
                        Style::default().fg(theme.text),
                    ))
                })
                .collect();
            let mut state = ListState::default();
            state.select(Some(chain.trigger.min(triggers.len() - 1)));
            frame.render_stateful_widget(
                List::new(items)
                    .block(block)
                    .highlight_style(Style::default().bg(theme.active_row_bg)),
                area,
                &mut state,
            );
        }
    }
}

fn draw_detail_overlay(frame: &mut Frame, app: &App, theme: &Theme) {
    let Some(detail) = &app.detail else { return };
    let full = frame.area();
    let area = popup(
        frame,
        full.width.saturating_sub(8).min(110),
        full.height.saturating_sub(4),
    );
    frame.render_widget(Clear, area);
    let block = modal(
        truncate(&detail.title, 60),
        "jk over rules · d remove one · E edit the prompt · esc close".into(),
        theme,
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [prompt_area, rules_area, runs_area] = Layout::vertical([
        Constraint::Percentage(34),
        Constraint::Percentage(33),
        Constraint::Percentage(33),
    ])
    .areas(inner);

    frame.render_widget(
        Paragraph::new(if detail.prompt.trim().is_empty() {
            "(no prompt — press E to write one)".to_string()
        } else {
            detail.prompt.clone()
        })
        .style(Style::default().fg(theme.text))
        .wrap(Wrap { trim: false })
        .block(section("prompt", theme)),
        prompt_area,
    );

    let rules: Vec<ListItem> = if detail.rules.is_empty() {
        vec![ListItem::new(Span::styled(
            " no rules — press c on the board to chain a card",
            Style::default().fg(theme.overlay),
        ))]
    } else {
        detail
            .rules
            .iter()
            .map(|(_, text)| {
                ListItem::new(Span::styled(
                    format!(" {text}"),
                    Style::default().fg(theme.text),
                ))
            })
            .collect()
    };
    let mut state = ListState::default();
    if !detail.rules.is_empty() {
        state.select(Some(detail.cursor.min(detail.rules.len() - 1)));
    }
    frame.render_stateful_widget(
        List::new(rules)
            .block(section("rules", theme))
            .highlight_style(Style::default().bg(theme.active_row_bg)),
        rules_area,
        &mut state,
    );

    let mut history: Vec<Line> = detail
        .runs
        .iter()
        .flat_map(|r| {
            r.lines()
                .map(|l| {
                    Line::from(Span::styled(
                        l.to_string(),
                        Style::default().fg(theme.subtext),
                    ))
                })
                .collect::<Vec<_>>()
        })
        .collect();
    if !detail.events.is_empty() {
        history.push(Line::from(Span::styled(
            "log",
            Style::default().fg(theme.overlay),
        )));
        history.extend(detail.events.iter().map(|e| {
            Line::from(Span::styled(
                format!("  {e}"),
                Style::default().fg(theme.overlay),
            ))
        }));
    }
    if history.is_empty() {
        history.push(Line::from(Span::styled(
            "never dispatched",
            Style::default().fg(theme.overlay),
        )));
    }
    frame.render_widget(
        Paragraph::new(history)
            .wrap(Wrap { trim: true })
            .block(section("runs", theme)),
        runs_area,
    );
}

fn section(title: &'static str, theme: &Theme) -> Block<'static> {
    Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(theme.surface))
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(theme.overlay),
        ))
}

/// The help sheet: keys on the left, then the commands, because half of what
/// this plugin does is reachable from a shell too.
const HELP_KEYS: &[(&str, &str)] = &[
    ("a", "quick add, queued at once"),
    ("n", "new card, full form"),
    ("space", "queue / unqueue"),
    ("Q", "queue the whole lane"),
    ("enter", "jump to its herdr pane"),
    ("v", "detail: rules, runs, log"),
    ("c", "chain to another card"),
    ("e", "edit card"),
    ("E", "edit prompt in $EDITOR"),
    ("y", "duplicate"),
    ("d", "delete (asks first)"),
    ("x", "cancel, release its pane"),
    ("r", "re-dispatch"),
    ("", ""),
    ("h l ← →", "lanes"),
    ("j k ↑ ↓", "cards"),
    ("1..9", "jump to a lane"),
    ("g G", "first / last"),
    ("H L", "shift a lane over"),
    ("J K", "move up / down the lane"),
    ("", ""),
    ("t", "pick a repository"),
    ("tab", "cycle the repo filter"),
    ("/", "search"),
    ("s", "re-import overlays"),
    ("R", "reload"),
    ("?", "this sheet"),
    ("q", "quit"),
];

const HELP_COMMANDS: &[(&str, &str)] = &[
    ("prefix+shift+b", "open this board"),
    ("prefix+a", "popup: one line, it runs"),
    ("", ""),
    ("add \"…\" -p \"…\"", "add a card, here"),
    ("repo scan --add", "find and track checkouts"),
    ("link A B", "when A is done, run B"),
    ("ls", "list cards"),
    ("show <card>", "inspect one"),
    ("move <card> ready", "queue from a shell"),
    ("retry | cancel <card>", "un-stick one"),
    ("sync", "re-import .herdr-board.toml"),
    ("configure", "sidebar, keys, PATH"),
    ("doctor", "check the wiring"),
];

fn draw_help(frame: &mut Frame, theme: &Theme) {
    let full = frame.area();
    let stacked_rows = (HELP_KEYS.len() + HELP_COMMANDS.len() + 4) as u16;
    // Stack only when it genuinely fits: a sheet cut off at the bottom loses
    // whole entries, where two columns only shave the odd long tail.
    let side_by_side = full.width >= 88 || full.height < stacked_rows;
    let width = if side_by_side {
        100.min(full.width.saturating_sub(2))
    } else {
        full.width.saturating_sub(2)
    };
    let rows = if side_by_side {
        (HELP_KEYS.len().max(HELP_COMMANDS.len()) + 3) as u16
    } else {
        stacked_rows
    };
    let area = popup(frame, width, rows);
    frame.render_widget(Clear, area);

    let block = modal("keys and commands".into(), "any key to close".into(), theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let render = |pairs: &[(&str, &str)], heading: &str, pad: usize| {
        let mut lines = vec![Line::from(Span::styled(
            format!(" {heading}"),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ))];
        lines.extend(pairs.iter().map(|(k, what)| {
            if k.is_empty() && what.is_empty() {
                return Line::from("");
            }
            Line::from(vec![
                Span::styled(format!(" {k:<pad$} "), Style::default().fg(theme.accent)),
                Span::styled((*what).to_string(), Style::default().fg(theme.subtext)),
            ])
        }));
        Paragraph::new(lines)
    };

    if side_by_side {
        let [left, right] =
            Layout::horizontal([Constraint::Length(38), Constraint::Min(0)]).areas(inner);
        frame.render_widget(render(HELP_KEYS, "on the board", 9), left);
        frame.render_widget(render(HELP_COMMANDS, "elsewhere", 21), right);
    } else {
        let [top, bottom] = Layout::vertical([
            Constraint::Length(HELP_KEYS.len() as u16 + 2),
            Constraint::Min(0),
        ])
        .areas(inner);
        frame.render_widget(render(HELP_KEYS, "on the board", 9), top);
        frame.render_widget(
            render(HELP_COMMANDS, "elsewhere — herdr-code-board …", 21),
            bottom,
        );
    }
}

fn draw_confirm(frame: &mut Frame, question: &str, theme: &Theme) {
    let area = popup(frame, (question.chars().count() as u16 + 8).max(34), 4);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                question.to_string(),
                Style::default().fg(theme.text),
            )),
            Line::from(Span::styled(
                "y to confirm, anything else to cancel",
                Style::default().fg(theme.overlay),
            )),
        ])
        .alignment(Alignment::Center)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(theme.red))
                .style(Style::default().bg(theme.panel_bg)),
        ),
        area,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wide_board_shows_every_lane() {
        assert_eq!(lane_window(200, 9, 0), (0, 9));
        assert_eq!(lane_window(9 * MIN_LANE_WIDTH, 9, 4), (0, 9));
    }

    #[test]
    fn a_narrow_board_shows_a_window_around_the_selection() {
        let (start, count) = lane_window(47, 9, 0);
        assert_eq!(count, 2);
        assert_eq!(start, 0);
        let (start, count) = lane_window(47, 9, 4);
        assert!((start..start + count).contains(&4));
    }

    #[test]
    fn the_window_never_runs_off_either_end() {
        for selected in 0..9 {
            let (start, count) = lane_window(90, 9, selected);
            assert!(start + count <= 9);
            assert!((start..start + count).contains(&selected));
        }
    }

    #[test]
    fn a_terminal_too_narrow_for_even_one_lane_still_renders_one() {
        let (start, count) = lane_window(5, 9, 3);
        assert_eq!((start, count), (3, 1));
    }

    #[test]
    fn the_help_sheet_covers_the_keys_the_status_bar_advertises() {
        for (key, _) in HINTS {
            let listed = HELP_KEYS
                .iter()
                .any(|(k, _)| k.split_whitespace().any(|part| part == *key));
            // `⏎` is spelled `enter` in the help, which is the readable form.
            assert!(
                listed || *key == "⏎",
                "the status bar offers {key} but help does not explain it"
            );
        }
        assert!(HELP_KEYS.iter().any(|(k, _)| *k == "enter"));
    }
}
