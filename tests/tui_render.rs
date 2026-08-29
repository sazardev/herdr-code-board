//! Draw every mode at hostile terminal sizes. A panic here kills the user's pane.
use herdr_code_board::config::Config;
use herdr_code_board::model::{Binding, Card, Column, Placement, Repo};
use herdr_code_board::tui::form::Form;
use herdr_code_board::tui::render;
use herdr_code_board::tui::state::{App, Detail, Key, Mode, PickerTarget, RepoChoice};
use herdr_code_board::tui::theme::Theme;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

fn card(id: &str, title: &str, column: Column) -> Card {
    Card {
        id: id.into(),
        key: None,
        title: title.into(),
        prompt: "a prompt that is reasonably long so wrapping matters".repeat(3),
        repo_id: Some("R1".into()),
        session: None,
        tags: vec!["urgent".into()],
        agent_kind: "claude".into(),
        model: Some("opus".into()),
        extra_args: vec![],
        placement: Placement::default(),
        column,
        binding: Binding {
            pane_id: Some("w1:p2".into()),
            ..Default::default()
        },
        priority: 0,
        auto_complete: true,
        auto_answer: false,
        max_retries: 0,
        attempts: 2,
        created_at: 0,
        updated_at: 0,
        status_since: 0,
        dispatched_at: None,
        last_error: Some("boom".into()),
        prompt_sent: true,
    }
}

fn app() -> App {
    let mut a = App::new(vec!["claude".into(), "codex".into()], "claude".into());
    a.load(
        vec![
            card(
                "1",
                "Ünïcødé 汉字 🎉 a very long card title that will not fit",
                Column::Backlog,
            ),
            card("2", "short", Column::Running),
        ],
        vec![Repo {
            id: "R1".into(),
            name: "erp".into(),
            path: "/repo/erp".into(),
            tags: vec![],
            max_parallel: 2,
            default_agent: None,
            default_model: None,
        }],
    );
    a
}

fn draw_at(a: &App, w: u16, h: u16) {
    let theme = Theme::from_config(&Config::default().theme);
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| render::draw(f, a, &theme)).unwrap();
}

const SIZES: &[(u16, u16)] = &[
    (1, 1),
    (2, 3),
    (5, 5),
    (10, 4),
    (20, 8),
    (40, 10),
    (47, 24),
    (80, 24),
    (200, 50),
    (300, 100),
    (1, 100),
    (100, 1),
];

/// A named way to put the board into one of its modes.
type Setup = (&'static str, Box<dyn Fn(&mut App)>);

#[test]
fn every_mode_draws_at_every_size() {
    let modes: Vec<Setup> = vec![
        ("normal", Box::new(|_: &mut App| {})),
        (
            "help",
            Box::new(|a: &mut App| {
                a.mode = Mode::Help;
            }),
        ),
        (
            "search",
            Box::new(|a: &mut App| {
                a.mode = Mode::Search;
                a.search = "Ünï".into();
            }),
        ),
        (
            "quickadd",
            Box::new(|a: &mut App| {
                a.start_quick();
                a.quick = "x".repeat(400);
            }),
        ),
        (
            "confirm",
            Box::new(|a: &mut App| {
                a.mode = Mode::Confirm("delete “Ünïcødé 汉字 🎉”?".repeat(4));
            }),
        ),
        (
            "form",
            Box::new(|a: &mut App| {
                let mut f = Form::new(&a.repos, a.agents.clone(), "claude");
                f.title = "y".repeat(300);
                f.placement = 4; // worktree, which grows the field list
                f.set_bases(vec![
                    "main".into(),
                    "feature/very-long-branch-name-here".into(),
                ]);
                a.form = Some(f);
                a.mode = Mode::Form;
            }),
        ),
        (
            "picker",
            Box::new(|a: &mut App| {
                let items = (0..60)
                    .map(|i| RepoChoice {
                        name: format!("repo-{i}-with-a-long-name"),
                        path: format!("/home/u/Documents/repo-{i}").into(),
                        branch: Some("feature/syntax-highlight-cli-export-multi-ext".into()),
                        tracked: i % 2 == 0,
                    })
                    .collect();
                a.open_picker(items, PickerTarget::Filter);
            }),
        ),
        (
            "chain",
            Box::new(|a: &mut App| {
                a.on_key(Key::Char('c'));
            }),
        ),
        (
            "chain-trigger",
            Box::new(|a: &mut App| {
                a.on_key(Key::Char('c'));
                a.on_key(Key::Enter);
            }),
        ),
        (
            "detail",
            Box::new(|a: &mut App| {
                a.open_detail(Detail {
                    card_id: "1".into(),
                    title: "Ünïcødé 汉字 🎉".repeat(6),
                    prompt: "prompt line\n".repeat(40),
                    rules: (0..12)
                        .map(|i| {
                            (
                                format!("R{i}"),
                                format!("rule {i} → do something quite long"),
                            )
                        })
                        .collect(),
                    runs: (0..10)
                        .map(|i| format!("#{i} done · 3m ago\n    detail line"))
                        .collect(),
                    events: (0..10)
                        .map(|i| format!("{i}m ago  lane: running"))
                        .collect(),
                    cursor: 11,
                });
            }),
        ),
    ];

    for (name, setup) in modes {
        for (w, h) in SIZES {
            let mut a = app();
            setup(&mut a);
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| draw_at(&a, *w, *h)));
            assert!(r.is_ok(), "PANIC drawing {name} at {w}x{h}");
        }
    }
}

#[test]
fn an_empty_board_draws_at_every_size() {
    let a = App::new(vec!["claude".into()], "claude".into());
    for (w, h) in SIZES {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| draw_at(&a, *w, *h)));
        assert!(r.is_ok(), "PANIC drawing an empty board at {w}x{h}");
    }
}
