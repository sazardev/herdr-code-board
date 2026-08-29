//! End-to-end behaviour of the engine against a fake herdr.
//!
//! These are the tests that describe what the plugin actually promises: a ready
//! card starts a real agent in a real pane, and finishing one card starts the
//! next.

use std::sync::Arc;

use herdr_code_board::config::Config;
use herdr_code_board::engine::dispatch::Executor;
use herdr_code_board::herdr::fake::FakeHerdr;
use herdr_code_board::model::{Action, AgentStatus, Column, Placement, Repo, Trigger};
use herdr_code_board::store::cards::NewCard;
use herdr_code_board::store::Store;

struct Harness {
    exec: Executor,
    herdr: Arc<FakeHerdr>,
}

impl Harness {
    fn new() -> Self {
        Self::with_config(Config::default())
    }

    fn with_config(config: Config) -> Self {
        let herdr = Arc::new(FakeHerdr::new().with_workspace("w1", "erp", "/repo/erp"));
        let store = Store::open_in_memory().unwrap();
        store
            .upsert_repo(&Repo {
                id: String::new(),
                name: "erp".into(),
                path: "/repo/erp".into(),
                tags: vec!["work".into()],
                max_parallel: 2,
                default_agent: None,
                default_model: None,
            })
            .unwrap();
        let exec = Executor::new(store, herdr.clone(), config);
        Self { exec, herdr }
    }

    fn repo_id(&self) -> String {
        self.exec.store.list_repos().unwrap()[0].id.clone()
    }

    fn card(&self, title: &str) -> String {
        self.card_with(NewCard {
            repo_id: Some(self.repo_id()),
            prompt: format!("do {title}"),
            column: Column::Ready,
            ..NewCard::new(title, "claude")
        })
    }

    fn card_with(&self, new: NewCard) -> String {
        self.exec.store.create_card(&new).unwrap().id
    }

    fn lane(&self, id: &str) -> Column {
        self.exec.store.get_card(id).unwrap().unwrap().column
    }

    fn pane_of(&self, id: &str) -> String {
        self.exec
            .store
            .get_card(id)
            .unwrap()
            .unwrap()
            .binding
            .pane_id
            .expect("card should own a pane")
    }
}

#[test]
fn a_ready_card_starts_an_agent_and_delivers_its_prompt() {
    let mut h = Harness::new();
    let id = h.card("review diff");

    assert_eq!(h.exec.dispatch_ready().unwrap(), 1);

    assert_eq!(h.lane(&id), Column::Running);
    let pane = h.pane_of(&id);
    assert_eq!(h.herdr.calls_matching("agent start").len(), 1);
    assert!(h.herdr.calls_matching("agent start")[0].contains("--kind claude"));
    assert_eq!(
        h.herdr.calls_matching("agent prompt"),
        vec![format!("agent prompt {pane} do review diff")]
    );

    let card = h.exec.store.get_card(&id).unwrap().unwrap();
    assert!(card.prompt_sent);
    assert_eq!(card.attempts, 1);
    assert_eq!(h.exec.store.runs_for_card(&id, 5).unwrap().len(), 1);
}

#[test]
fn the_model_reaches_the_agent_cli_as_a_flag() {
    let mut h = Harness::new();
    h.card_with(NewCard {
        repo_id: Some(h.repo_id()),
        column: Column::Ready,
        model: Some("opus".into()),
        extra_args: vec!["--permission-mode".into(), "plan".into()],
        ..NewCard::new("with model", "claude")
    });

    h.exec.dispatch_ready().unwrap();

    let start = &h.herdr.calls_matching("agent start")[0];
    assert!(
        start.ends_with("-- --model opus --permission-mode plan"),
        "unexpected argv: {start}"
    );
}

/// The flow the plugin exists for: chain one prompt to the next.
#[test]
fn finishing_a_card_enqueues_and_dispatches_the_next_one() {
    let mut h = Harness::new();
    let first = h.card("write the code");
    let second = h.card_with(NewCard {
        repo_id: Some(h.repo_id()),
        prompt: "run the tests".into(),
        column: Column::Backlog,
        ..NewCard::new("run tests", "claude")
    });
    h.exec
        .store
        .add_rule(
            Some(&first),
            None,
            &Trigger::Done,
            &Action::Enqueue {
                cards: vec![second.clone()],
            },
            0,
        )
        .unwrap();

    h.exec.dispatch_ready().unwrap();
    let pane = h.pane_of(&first);

    // The agent works, then finishes.
    h.exec.on_agent_status(&pane, AgentStatus::Working).unwrap();
    assert_eq!(h.lane(&first), Column::Running);
    h.exec.on_agent_status(&pane, AgentStatus::Done).unwrap();

    assert_eq!(h.lane(&first), Column::Done);
    assert_eq!(h.lane(&second), Column::Ready, "the next card was enqueued");

    assert_eq!(h.exec.dispatch_ready().unwrap(), 1);
    assert_eq!(h.lane(&second), Column::Running);
    assert_eq!(h.herdr.calls_matching("agent start").len(), 2);
}

#[test]
fn a_card_without_auto_complete_parks_in_waiting_for_a_human() {
    let mut h = Harness::new();
    let id = h.card_with(NewCard {
        repo_id: Some(h.repo_id()),
        column: Column::Ready,
        auto_complete: false,
        ..NewCard::new("needs review", "claude")
    });
    h.exec.dispatch_ready().unwrap();
    let pane = h.pane_of(&id);

    h.exec.on_agent_status(&pane, AgentStatus::Idle).unwrap();
    assert_eq!(h.lane(&id), Column::Waiting);
    // It still owns its pane, so a human can jump straight to it.
    assert!(h
        .exec
        .store
        .get_card(&id)
        .unwrap()
        .unwrap()
        .binding
        .pane_id
        .is_some());
}

#[test]
fn a_waiting_card_that_sits_too_long_triggers_its_rule() {
    let mut h = Harness::new();
    let id = h.card_with(NewCard {
        repo_id: Some(h.repo_id()),
        column: Column::Ready,
        auto_complete: false,
        ..NewCard::new("slow one", "claude")
    });
    h.exec.dispatch_ready().unwrap();
    let pane = h.pane_of(&id);
    h.exec
        .store
        .add_rule(
            Some(&id),
            None,
            &Trigger::WaitingFor { seconds: 900 },
            &Action::Prompt {
                text: "are you stuck?".into(),
            },
            1,
        )
        .unwrap();

    h.exec.on_agent_status(&pane, AgentStatus::Idle).unwrap();
    let since = h.exec.store.get_card(&id).unwrap().unwrap().status_since;

    h.exec.tick(since + 899).unwrap();
    assert!(h.herdr.calls_matching("are you stuck").is_empty());

    h.exec.tick(since + 900).unwrap();
    assert_eq!(h.herdr.calls_matching("are you stuck").len(), 1);

    // max_fires = 1, so a later sweep must not send it again.
    h.exec.tick(since + 5_000).unwrap();
    assert_eq!(h.herdr.calls_matching("are you stuck").len(), 1);
}

#[test]
fn the_next_deadline_is_reported_so_the_engine_can_sleep() {
    let mut h = Harness::new();
    let id = h.card_with(NewCard {
        repo_id: Some(h.repo_id()),
        column: Column::Ready,
        auto_complete: false,
        ..NewCard::new("slow one", "claude")
    });
    h.exec.dispatch_ready().unwrap();
    let pane = h.pane_of(&id);
    h.exec
        .store
        .add_rule(
            Some(&id),
            None,
            &Trigger::WaitingFor { seconds: 60 },
            &Action::Cancel,
            0,
        )
        .unwrap();

    assert_eq!(h.exec.next_deadline().unwrap(), None, "not waiting yet");
    h.exec.on_agent_status(&pane, AgentStatus::Idle).unwrap();
    let since = h.exec.store.get_card(&id).unwrap().unwrap().status_since;
    assert_eq!(h.exec.next_deadline().unwrap(), Some(since + 60));
}

#[test]
fn auto_answer_is_refused_unless_both_switches_are_on() {
    // Card opts in, global config does not.
    let mut h = Harness::new();
    let id = h.card_with(NewCard {
        repo_id: Some(h.repo_id()),
        column: Column::Ready,
        auto_answer: true,
        ..NewCard::new("risky", "claude")
    });
    h.exec
        .store
        .add_rule(
            Some(&id),
            None,
            &Trigger::Blocked,
            &Action::Answer { choice: 1 },
            0,
        )
        .unwrap();
    h.exec.dispatch_ready().unwrap();
    let pane = h.pane_of(&id);

    h.exec.on_agent_status(&pane, AgentStatus::Blocked).unwrap();
    assert_eq!(h.lane(&id), Column::Blocked);
    assert!(
        h.herdr.calls_matching("send-keys").is_empty(),
        "must not type into a dialog without the global opt-in"
    );
    let events = h.exec.store.recent_events(20).unwrap();
    assert!(events.iter().any(|e| e.kind == "auto_answer_refused"));
}

#[test]
fn auto_answer_records_the_dialog_before_answering_it() {
    let config = Config {
        allow_auto_answer: true,
        ..Config::default()
    };
    let mut h = Harness::with_config(config);
    let id = h.card_with(NewCard {
        repo_id: Some(h.repo_id()),
        column: Column::Ready,
        auto_answer: true,
        ..NewCard::new("risky", "claude")
    });
    h.exec
        .store
        .add_rule(
            Some(&id),
            None,
            &Trigger::Blocked,
            &Action::Answer { choice: 1 },
            0,
        )
        .unwrap();
    h.exec.dispatch_ready().unwrap();
    let pane = h.pane_of(&id);
    h.herdr
        .set_screen(&pane, "Do you want to make this edit?\n 1. Yes\n 2. No");

    h.exec.on_agent_status(&pane, AgentStatus::Blocked).unwrap();

    assert_eq!(
        h.herdr.calls_matching("send-keys"),
        vec![format!("agent send-keys {pane} 1")]
    );
    // Only the cheap read source may be used against a pane a human is watching.
    let reads = h.herdr.calls_matching("pane read");
    assert_eq!(reads.len(), 1);
    assert!(reads[0].contains("--source visible"));

    let detail = h.exec.store.runs_for_card(&id, 1).unwrap()[0]
        .detail
        .clone()
        .unwrap();
    assert!(detail.contains("Do you want to make this edit?"));
    assert!(detail.contains("auto-answered choice 1"));
}

#[test]
fn concurrency_is_capped_per_repo() {
    let mut h = Harness::new(); // repo max_parallel = 2
    for i in 0..4 {
        h.card(&format!("job {i}"));
    }
    assert_eq!(h.exec.dispatch_ready().unwrap(), 2);
    assert_eq!(h.exec.store.live_cards().unwrap().len(), 2);

    // Nothing more starts until a slot frees up.
    assert_eq!(h.exec.dispatch_ready().unwrap(), 0);

    let running = h.exec.store.cards_in(Column::Running).unwrap();
    let pane = running[0].binding.pane_id.clone().unwrap();
    h.exec.on_agent_status(&pane, AgentStatus::Done).unwrap();
    assert_eq!(h.exec.dispatch_ready().unwrap(), 1);
}

#[test]
fn a_lost_pane_retries_then_fails_the_card() {
    let mut h = Harness::new();
    let id = h.card_with(NewCard {
        repo_id: Some(h.repo_id()),
        column: Column::Ready,
        max_retries: 1,
        ..NewCard::new("flaky", "claude")
    });

    h.exec.dispatch_ready().unwrap();
    let first_pane = h.pane_of(&id);
    h.exec.on_pane_gone(&first_pane).unwrap();
    assert_eq!(h.lane(&id), Column::Ready, "the first loss is retried");

    h.exec.dispatch_ready().unwrap();
    let second_pane = h.pane_of(&id);
    assert_ne!(second_pane, first_pane);
    h.exec.on_pane_gone(&second_pane).unwrap();
    assert_eq!(h.lane(&id), Column::Failed, "retries are exhausted");
    assert_eq!(h.exec.store.runs_for_card(&id, 5).unwrap().len(), 2);
}

#[test]
fn closing_a_workspace_fails_every_card_that_lived_in_it() {
    let mut h = Harness::new();
    let a = h.card("a");
    let b = h.card("b");
    h.exec.dispatch_ready().unwrap();

    let ws = h
        .exec
        .store
        .get_card(&a)
        .unwrap()
        .unwrap()
        .binding
        .workspace_id
        .unwrap();
    h.exec.on_workspace_gone(&ws).unwrap();

    assert_eq!(h.lane(&a), Column::Failed);
    assert_eq!(h.lane(&b), Column::Failed);
}

#[test]
fn a_dispatch_failure_marks_the_card_and_records_why() {
    let mut h = Harness::new();
    let id = h.card("doomed");
    h.herdr.fail_on("agent start", "agent_start_failed: no CLI");

    assert_eq!(h.exec.dispatch_ready().unwrap(), 0);
    assert_eq!(h.lane(&id), Column::Failed);
    let card = h.exec.store.get_card(&id).unwrap().unwrap();
    assert!(card.last_error.unwrap().contains("agent_start_failed"));
    assert!(
        card.binding.pane_id.is_none(),
        "a failed card releases its pane"
    );
}

#[test]
fn an_agent_blocked_at_startup_lands_in_blocked_not_failed() {
    let mut h = Harness::new();
    let id = h.card("slow to boot");
    h.herdr.fail_on("agent start", "agent_not_ready");

    assert_eq!(h.exec.dispatch_ready().unwrap(), 1);
    assert_eq!(h.lane(&id), Column::Blocked);
    assert!(h
        .exec
        .store
        .get_card(&id)
        .unwrap()
        .unwrap()
        .binding
        .pane_id
        .is_some());
}

#[test]
fn a_worktree_card_gets_its_own_workspace() {
    let mut h = Harness::new();
    let id = h.card_with(NewCard {
        repo_id: Some(h.repo_id()),
        column: Column::Ready,
        placement: Placement::Worktree {
            branch: "board/{card}".into(),
            base: Some("main".into()),
        },
        ..NewCard::new("isolated", "claude")
    });

    h.exec.dispatch_ready().unwrap();

    let created = h.herdr.calls_matching("worktree create");
    assert_eq!(created.len(), 1);
    assert!(created[0].contains("--branch board/isolated-"));
    assert!(created[0].contains("--base main"));

    let binding = h.exec.store.get_card(&id).unwrap().unwrap().binding;
    assert_ne!(binding.workspace_id.as_deref(), Some("w1"));
    assert!(binding.worktree_path.is_some());
}

#[test]
fn a_status_event_for_an_unknown_pane_is_ignored() {
    let mut h = Harness::new();
    h.card("a");
    h.exec.dispatch_ready().unwrap();
    // No card owns this pane; the engine must not panic or guess.
    h.exec.on_agent_status("w9:p9", AgentStatus::Done).unwrap();
    assert_eq!(h.exec.store.cards_in(Column::Done).unwrap().len(), 0);
}

#[test]
fn enqueueing_a_card_that_is_already_running_does_not_restart_it() {
    let mut h = Harness::new();
    let first = h.card("a");
    let second = h.card("b");
    h.exec
        .store
        .add_rule(
            Some(&first),
            None,
            &Trigger::Done,
            &Action::Enqueue {
                cards: vec![second.clone()],
            },
            0,
        )
        .unwrap();

    h.exec.dispatch_ready().unwrap(); // starts both, budget is 2
    assert_eq!(h.lane(&second), Column::Running);
    let pane = h.pane_of(&first);
    h.exec.on_agent_status(&pane, AgentStatus::Done).unwrap();

    assert_eq!(h.lane(&second), Column::Running, "still its own run");
    assert_eq!(h.herdr.calls_matching("agent start").len(), 2);
    let events = h.exec.store.recent_events(30).unwrap();
    assert!(events.iter().any(|e| e.kind == "enqueue_skipped"));
}
