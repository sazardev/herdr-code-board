//! The manifest is the contract with herdr. These tests keep it honest about the
//! binary it launches, the events it hooks, and the version it claims to need.

use std::collections::BTreeSet;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Manifest {
    id: String,
    name: String,
    version: String,
    min_herdr_version: String,
    description: String,
    platforms: Vec<String>,
    #[serde(default)]
    build: Vec<Entry>,
    #[serde(default)]
    startup: Vec<Entry>,
    #[serde(default)]
    actions: Vec<Entry>,
    #[serde(default)]
    events: Vec<Entry>,
    #[serde(default)]
    panes: Vec<Entry>,
}

#[derive(Debug, Deserialize)]
struct Entry {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    on: Option<String>,
    #[serde(default)]
    placement: Option<String>,
    command: Vec<String>,
}

fn manifest() -> Manifest {
    let raw = include_str!("../herdr-plugin.toml");
    toml::from_str(raw).expect("herdr-plugin.toml must parse")
}

/// Every event name herdr 0.8.2 emits, from `herdr api schema --json`.
const HERDR_EVENTS: &[&str] = &[
    "workspace.created",
    "workspace.updated",
    "workspace.metadata_updated",
    "workspace.closed",
    "workspace.renamed",
    "workspace.moved",
    "workspace.reordered",
    "workspace.focused",
    "worktree.created",
    "worktree.opened",
    "worktree.removed",
    "tab.created",
    "tab.closed",
    "tab.renamed",
    "tab.moved",
    "tab.focused",
    "pane.created",
    "pane.closed",
    "pane.updated",
    "pane.focused",
    "pane.moved",
    "pane.output_changed",
    "pane.exited",
    "pane.agent_detected",
    "pane.agent_status_changed",
    "layout.updated",
];

/// Placements a manifest pane entry may declare.
const PLACEMENTS: &[&str] = &["overlay", "popup", "split", "tab", "zoomed"];

#[test]
fn the_manifest_declares_the_required_metadata() {
    let m = manifest();
    assert_eq!(m.id, "herdr-code-board");
    assert!(!m.name.is_empty());
    assert!(!m.description.is_empty());
    assert_eq!(m.min_herdr_version, "0.8.0");
    assert_eq!(m.platforms, vec!["linux", "macos"]);
}

#[test]
fn the_manifest_version_matches_the_crate_version() {
    assert_eq!(manifest().version, env!("CARGO_PKG_VERSION"));
}

#[test]
fn every_hooked_event_is_one_herdr_actually_emits() {
    for event in manifest().events {
        let on = event.on.expect("an event entry needs `on`");
        assert!(
            HERDR_EVENTS.contains(&on.as_str()),
            "{on:?} is not an event herdr 0.8.2 emits"
        );
    }
}

#[test]
fn every_command_runs_our_own_binary_from_the_plugin_root() {
    let m = manifest();
    let runtime = m
        .startup
        .iter()
        .chain(&m.actions)
        .chain(&m.events)
        .chain(&m.panes);
    for entry in runtime {
        let joined = entry.command.join(" ");
        assert!(
            joined.contains("$HERDR_PLUGIN_ROOT/target/release/herdr-code-board"),
            "command does not run the built binary: {joined}"
        );
        // Herdr does not run commands through a shell, so anything using shell
        // syntax has to start one itself.
        assert_eq!(entry.command[0], "sh", "shell syntax needs an explicit sh");
        assert_eq!(entry.command[1], "-c");
    }
    assert_eq!(m.build.len(), 1);
    assert_eq!(m.build[0].command, vec!["cargo", "build", "--release"]);
}

#[test]
fn every_subcommand_the_manifest_invokes_exists_in_the_cli() {
    use clap::CommandFactory;
    let cli = herdr_code_board::cli::Cli::command();
    let known: BTreeSet<String> = cli
        .get_subcommands()
        .map(|c| c.get_name().to_string())
        .collect();

    let m = manifest();
    for entry in m
        .startup
        .iter()
        .chain(&m.actions)
        .chain(&m.events)
        .chain(&m.panes)
    {
        let script = entry.command.last().expect("a command body");
        // `... herdr-code-board" <sub> [args]`
        let after = script
            .rsplit_once("herdr-code-board\"")
            .map(|(_, rest)| rest)
            .unwrap_or_default();
        let sub = after.split_whitespace().next().unwrap_or_default();
        assert!(
            known.contains(sub),
            "the manifest invokes `{sub}`, which is not a subcommand; known: {known:?}"
        );
    }
}

#[test]
fn ids_are_unique_within_each_kind_and_carry_no_dots() {
    let m = manifest();
    for group in [&m.actions, &m.panes, &m.events] {
        let ids: Vec<&String> = group.iter().filter_map(|e| e.id.as_ref()).collect();
        let unique: BTreeSet<&&String> = ids.iter().collect();
        assert_eq!(ids.len(), unique.len(), "duplicate ids in {ids:?}");
        for id in ids {
            assert!(!id.contains('.'), "local ids may not contain dots: {id}");
            assert!(
                id.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == ':'),
                "invalid id: {id}"
            );
        }
    }
}

#[test]
fn panes_and_actions_are_labelled_and_placed() {
    let m = manifest();
    for pane in &m.panes {
        assert!(pane.id.is_some() && pane.title.is_some());
        let placement = pane.placement.as_deref().unwrap_or("overlay");
        assert!(
            PLACEMENTS.contains(&placement),
            "{placement:?} is not a valid pane placement"
        );
    }
    assert!(
        m.panes.iter().any(|p| p.id.as_deref() == Some("board")),
        "the `board` entrypoint is what `open` asks herdr for"
    );
    for action in &m.actions {
        assert!(action.id.is_some() && action.title.is_some());
    }
}
