//! Safety properties: the ways this plugin could burn a user's quota, wedge
//! their board, or hand herdr something it will reject.
//!
//! Every test here started as a probe that found a real defect.

use std::sync::Arc;

use herdr_code_board::config::Config;
use herdr_code_board::engine::dispatch::Executor;
use herdr_code_board::herdr::fake::FakeHerdr;
use herdr_code_board::model::{Action, AgentStatus, Column, Repo, Trigger};
use herdr_code_board::store::cards::NewCard;
use herdr_code_board::store::Store;

fn board() -> (Executor, Arc<FakeHerdr>, String) {
    let herdr = Arc::new(FakeHerdr::new().with_workspace("w1", "erp", "/repo/erp"));
    let store = Store::open_in_memory().unwrap();
    let repo = store
        .upsert_repo(&Repo {
            id: String::new(),
            name: "erp".into(),
            path: "/repo/erp".into(),
            tags: vec![],
            max_parallel: 4,
            default_agent: None,
            default_model: None,
        })
        .unwrap();
    let id = repo.id.clone();
    (
        Executor::new(store, herdr.clone(), Config::default()),
        herdr,
        id,
    )
}

/// Run the board like a user who walked away: dispatch, complete, repeat.
fn run_rounds(exec: &mut Executor, rounds: usize) {
    for _ in 0..rounds {
        exec.dispatch_ready().unwrap();
        for card in exec.store.live_cards().unwrap() {
            let pane = card.binding.pane_id.clone().unwrap();
            exec.on_agent_status(&pane, AgentStatus::Done).unwrap();
        }
    }
}

/// A queues B, B queues A. Nothing in the rule engine stops that on its own, and
/// each pass is a real agent in a real pane. Before the dispatch budget existed
/// this ran forever.
#[test]
fn a_rule_cycle_cannot_dispatch_forever() {
    let (mut exec, herdr, repo) = board();
    let mk = |t: &str| {
        exec.store
            .create_card(&NewCard {
                repo_id: Some(repo.clone()),
                ..NewCard::new(t, "claude")
            })
            .unwrap()
            .id
    };
    let a = mk("A");
    let b = mk("B");
    exec.store
        .add_rule(
            Some(&a),
            None,
            &Trigger::Done,
            &Action::Enqueue {
                cards: vec![b.clone()],
            },
            0,
        )
        .unwrap();
    exec.store
        .add_rule(
            Some(&b),
            None,
            &Trigger::Done,
            &Action::Enqueue {
                cards: vec![a.clone()],
            },
            0,
        )
        .unwrap();
    exec.store.set_lane(&a, Column::Ready).unwrap();

    run_rounds(&mut exec, 300);

    let starts = herdr.calls_matching("agent start").len();
    let budget = Config::default().max_dispatches as usize;
    assert!(
        starts <= budget * 2,
        "a two-card cycle started {starts} agents; the budget allows {}",
        budget * 2
    );
    // And both cards end up stopped with an explanation, not silently stuck.
    for id in [&a, &b] {
        let card = exec.store.get_card(id).unwrap().unwrap();
        assert_eq!(card.column, Column::Failed);
        assert!(card.last_error.unwrap().contains("max_dispatches"));
    }
}

#[test]
fn a_card_linked_to_itself_is_bounded_too() {
    let (mut exec, herdr, repo) = board();
    let a = exec
        .store
        .create_card(&NewCard {
            repo_id: Some(repo),
            column: Column::Ready,
            ..NewCard::new("A", "claude")
        })
        .unwrap()
        .id;
    exec.store
        .add_rule(
            Some(&a),
            None,
            &Trigger::Done,
            &Action::Enqueue {
                cards: vec![a.clone()],
            },
            0,
        )
        .unwrap();

    run_rounds(&mut exec, 100);

    assert_eq!(
        herdr.calls_matching("agent start").len(),
        Config::default().max_dispatches as usize
    );
}

/// The budget is a safety net, not a policy. A human saying "run it again"
/// clears it.
#[test]
fn retrying_by_hand_clears_the_dispatch_budget() {
    let (mut exec, herdr, repo) = board();
    let a = exec
        .store
        .create_card(&NewCard {
            repo_id: Some(repo),
            column: Column::Ready,
            ..NewCard::new("A", "claude")
        })
        .unwrap()
        .id;
    exec.store
        .add_rule(
            Some(&a),
            None,
            &Trigger::Done,
            &Action::Enqueue {
                cards: vec![a.clone()],
            },
            0,
        )
        .unwrap();
    run_rounds(&mut exec, 100);
    let spent = herdr.calls_matching("agent start").len();

    exec.store.reset_attempts(&a).unwrap();
    exec.store.set_lane(&a, Column::Ready).unwrap();
    exec.dispatch_ready().unwrap();

    assert_eq!(herdr.calls_matching("agent start").len(), spent + 1);
}

/// A rule pointing at a card that was deleted must not take the sweep down.
#[test]
fn an_enqueue_of_a_deleted_card_is_logged_not_fatal() {
    let (mut exec, _h, repo) = board();
    let a = exec
        .store
        .create_card(&NewCard {
            repo_id: Some(repo.clone()),
            column: Column::Ready,
            ..NewCard::new("A", "claude")
        })
        .unwrap()
        .id;
    let b = exec
        .store
        .create_card(&NewCard {
            repo_id: Some(repo),
            ..NewCard::new("B", "claude")
        })
        .unwrap()
        .id;
    exec.store
        .add_rule(
            Some(&a),
            None,
            &Trigger::Done,
            &Action::Enqueue {
                cards: vec![b.clone()],
            },
            0,
        )
        .unwrap();
    exec.store.delete_card(&b).unwrap();

    exec.dispatch_ready().unwrap();
    let pane = exec.store.live_cards().unwrap()[0]
        .binding
        .pane_id
        .clone()
        .unwrap();
    exec.on_agent_status(&pane, AgentStatus::Done).unwrap();

    assert!(exec
        .store
        .recent_events(20)
        .unwrap()
        .iter()
        .any(|e| e.kind == "enqueue_missing"));
    assert_eq!(
        exec.store.get_card(&a).unwrap().unwrap().column,
        Column::Done
    );
}

/// Untracking a repo while its card is running must not wedge the card.
#[test]
fn deleting_a_repo_under_a_live_card_leaves_the_card_workable() {
    let (mut exec, _h, repo) = board();
    exec.store
        .create_card(&NewCard {
            repo_id: Some(repo.clone()),
            column: Column::Ready,
            ..NewCard::new("A", "claude")
        })
        .unwrap();
    exec.dispatch_ready().unwrap();
    exec.store.delete_repo(&repo).unwrap();

    let card = exec.store.list_cards().unwrap()[0].clone();
    assert_eq!(card.repo_id, None, "the row is detached, not deleted");
    let pane = card.binding.pane_id.clone().unwrap();
    exec.on_agent_status(&pane, AgentStatus::Done).unwrap();
    exec.present().unwrap();
    assert_eq!(
        exec.store.get_card(&card.id).unwrap().unwrap().column,
        Column::Done
    );
}

/// Herdr rejects an agent name outside `[a-z][a-z0-9_-]{0,31}`, so a card title
/// is never handed over raw.
#[test]
fn any_title_a_human_can_type_still_produces_a_legal_agent_name() {
    let (exec, _h, repo) = board();
    let long = "x".repeat(500);
    for title in [
        "Ünïcødé 汉字 🎉 emoji",
        long.as_str(),
        "line one\nline two",
        "; rm -rf / #",
        "   ",
        "--not-a-flag",
        "9 lives",
        "\t\ttabs",
    ] {
        let card = exec
            .store
            .create_card(&NewCard {
                repo_id: Some(repo.clone()),
                ..NewCard::new(title, "claude")
            })
            .unwrap();
        let slug = card.slug();
        assert!(slug.len() <= 32, "{title:?} -> {slug:?} is too long");
        assert!(
            slug.starts_with(|c: char| c.is_ascii_lowercase()),
            "{title:?} -> {slug:?} must start with a letter"
        );
        assert!(
            slug.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_'),
            "{title:?} -> {slug:?} has characters herdr will reject"
        );
    }
}
