//! One board, several herdr sessions.
//!
//! The board is a single database per user, but herdr can run several sessions
//! at once, each behind its own socket, and an event hook inherits whichever one
//! fired it. Without the routing these tests cover, a card queued while you
//! worked in one session could be started in another — whichever swept first.

use std::sync::Arc;

use herdr_code_board::config::Config;
use herdr_code_board::engine::dispatch::Executor;
use herdr_code_board::herdr::fake::FakeHerdr;
use herdr_code_board::model::{Column, Repo};
use herdr_code_board::session::{fixed_directory, Session};
use herdr_code_board::store::cards::NewCard;
use herdr_code_board::store::Store;

const HERE: &str = "default";
const OTHER: &str = "work";
const OTHER_SOCKET: &str = "/home/u/.config/herdr/sessions/work/herdr.sock";

fn board(sessions: Vec<Session>) -> (Executor, Arc<FakeHerdr>, String) {
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
    let exec = Executor::new(store, herdr.clone(), Config::default())
        .with_sessions(fixed_directory(sessions));
    (exec, herdr, id)
}

fn card(exec: &Executor, repo: &str, title: &str, session: Option<&str>) -> String {
    exec.store
        .create_card(&NewCard {
            repo_id: Some(repo.to_string()),
            session: session.map(str::to_string),
            column: Column::Ready,
            prompt: "p".into(),
            ..NewCard::new(title, "claude")
        })
        .unwrap()
        .id
}

fn both_sessions() -> Vec<Session> {
    vec![
        Session::new(HERE, "/home/u/.config/herdr/herdr.sock", true),
        Session::new(OTHER, OTHER_SOCKET, true),
    ]
}

/// The headline: a card claimed by another session is started against *that*
/// session's server, not the one whose hook happened to run.
#[test]
fn a_card_runs_in_the_session_that_queued_it() {
    let (mut exec, herdr, repo) = board(both_sessions());
    card(&exec, &repo, "belongs elsewhere", Some(OTHER));

    assert_eq!(exec.dispatch_ready().unwrap(), 1);

    let starts = herdr.calls_matching("agent start");
    assert_eq!(starts.len(), 1);
    assert!(
        starts[0].starts_with(&format!("[{OTHER_SOCKET}] ")),
        "started against the wrong session: {}",
        starts[0]
    );
}

#[test]
fn a_card_of_our_own_session_uses_the_api_we_already_have() {
    let (mut exec, herdr, repo) = board(both_sessions());
    // `current_name` reads the environment; in a plain test run there is none,
    // so an explicitly-named session is "elsewhere" and an unclaimed one is ours.
    card(&exec, &repo, "unclaimed", None);

    exec.dispatch_ready().unwrap();

    let starts = herdr.calls_matching("agent start");
    assert_eq!(starts.len(), 1);
    assert!(!starts[0].starts_with('['), "should not have been routed");
}

/// A session that is registered but stopped: hold the card, do not fail it, and
/// do not start it somewhere else.
#[test]
fn a_card_waits_for_its_session_instead_of_running_in_another() {
    let sessions = vec![
        Session::new(HERE, "/home/u/.config/herdr/herdr.sock", true),
        Session::new(OTHER, OTHER_SOCKET, false),
    ];
    let (mut exec, herdr, repo) = board(sessions);
    let id = card(&exec, &repo, "waits", Some(OTHER));

    assert_eq!(exec.dispatch_ready().unwrap(), 0);
    assert!(herdr.calls_matching("agent start").is_empty());

    let card = exec.store.get_card(&id).unwrap().unwrap();
    assert_eq!(card.column, Column::Ready, "held, not failed");
    assert!(card
        .last_error
        .unwrap()
        .contains("waiting for herdr session"));

    // And it starts as soon as that session comes up.
    exec.sessions = fixed_directory(both_sessions());
    assert_eq!(exec.dispatch_ready().unwrap(), 1);
    assert_eq!(
        exec.store.get_card(&id).unwrap().unwrap().column,
        Column::Running
    );
}

#[test]
fn a_card_naming_a_session_that_no_longer_exists_says_so_once() {
    let (mut exec, _h, repo) = board(vec![Session::new(
        HERE,
        "/home/u/.config/herdr/herdr.sock",
        true,
    )]);
    let id = card(&exec, &repo, "orphan", Some("deleted-session"));

    for _ in 0..5 {
        exec.dispatch_ready().unwrap();
    }

    let card = exec.store.get_card(&id).unwrap().unwrap();
    assert_eq!(card.column, Column::Ready);
    assert!(card.last_error.unwrap().contains("does not exist"));
    // Logged once, not once per sweep.
    let notes = exec
        .store
        .recent_events(50)
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "waiting_for_session")
        .count();
    assert_eq!(notes, 1);
}

/// Cards for several sessions in one sweep each go to the right place.
#[test]
fn one_sweep_serves_every_session() {
    let third = "side";
    let third_socket = "/home/u/.config/herdr/sessions/side/herdr.sock";
    let mut sessions = both_sessions();
    sessions.push(Session::new(third, third_socket, true));
    let (mut exec, herdr, repo) = board(sessions);

    card(&exec, &repo, "for work", Some(OTHER));
    card(&exec, &repo, "for side", Some(third));
    card(&exec, &repo, "for nobody", None);

    assert_eq!(exec.dispatch_ready().unwrap(), 3);

    let starts = herdr.calls_matching("agent start");
    assert_eq!(starts.len(), 3);
    assert_eq!(
        starts.iter().filter(|c| c.contains(OTHER_SOCKET)).count(),
        1
    );
    assert_eq!(
        starts.iter().filter(|c| c.contains(third_socket)).count(),
        1
    );
    assert_eq!(starts.iter().filter(|c| !c.starts_with('[')).count(), 1);
}

/// A board that has never seen a second session must not pay for one.
#[test]
fn an_unclaimed_board_never_asks_herdr_about_sessions() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = calls.clone();
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
    let mut exec =
        Executor::new(store, herdr, Config::default()).with_sessions(Arc::new(move || {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Vec::new()
        }));
    card(&exec, &repo.id, "plain", None);

    exec.dispatch_ready().unwrap();

    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "no card names a session, so herdr should not be asked"
    );
}
