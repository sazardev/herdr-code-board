//! Command implementations.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};

use crate::cli::{AddArgs, Cli, Command, LinkArgs, LsArgs, RepoCommand};
use crate::config::{Config, Paths};
use crate::engine::daemon;
use crate::engine::dispatch::Executor;
use crate::engine::events::PluginEvent;
use crate::engine::lock::DispatchLock;
use crate::herdr::client::CliHerdr;
use crate::herdr::HerdrApi;
use crate::model::{Action, Card, Column, Placement, Repo};
use crate::store::cards::NewCard;
use crate::store::{now, Store};
use crate::{agents, overlay};

pub fn run(cli: Cli) -> Result<()> {
    let paths = Paths::resolve()?;
    let config = Config::load(&paths)?;

    match cli.command.unwrap_or(Command::Board) {
        Command::Board => crate::tui::run(&paths, &config),
        Command::Daemon => daemon::run(&paths, &config),
        Command::Startup => startup(&paths, &config),
        Command::Event => event(&paths, &config),
        Command::Open => open_board(),
        // Both attach the card to the repository you are standing in. `add` is
        // what you type; `enqueue-here` is the same thing wired to a herdr action,
        // where the directory comes from the focused pane rather than the shell.
        Command::Add(args) | Command::EnqueueHere(args) => {
            let cwd = context_cwd();
            add(&paths, &config, args, Some(cwd))
        }
        Command::Ls(args) => list(&paths, args),
        Command::Show { card } => show(&paths, &card),
        Command::Move { card, lane } => move_card(&paths, &config, &card, lane.into()),
        Command::Retry { card } => retry(&paths, &config, &card),
        Command::Cancel { card, close_pane } => cancel(&paths, &card, close_pane),
        Command::Rm { card } => remove(&paths, &card),
        Command::Link(args) => link(&paths, args),
        Command::Repo(cmd) => repo(&paths, &config, cmd),
        Command::Sync { paths: which } => sync(&paths, &config, which),
        Command::Doctor => doctor(&paths, &config),
    }
}

fn store_of(paths: &Paths) -> Result<Store> {
    Store::open(&paths.database())
}

fn herdr() -> Arc<dyn HerdrApi> {
    Arc::new(CliHerdr::new())
}

/// Keys herdr uses for a working directory in `HERDR_PLUGIN_CONTEXT_JSON`, most
/// specific first. Captured from a real `plugin.action.invoke` on 0.8.2, which
/// is flat and uses prefixed names rather than a nested pane object.
const CWD_KEYS: [&str; 4] = ["focused_pane_cwd", "workspace_cwd", "foreground_cwd", "cwd"];

/// The directory the invocation came from.
///
/// Herdr puts the focused pane's context in `HERDR_PLUGIN_CONTEXT_JSON` when it
/// runs an action, which is what makes `enqueue-here` know which repo you meant.
/// Without it we would fall back to the process cwd, which for a plugin action
/// is the plugin's own directory — the wrong repo, silently.
pub fn context_cwd_from(raw: Option<&str>) -> Option<PathBuf> {
    let value: serde_json::Value = serde_json::from_str(raw?).ok()?;
    CWD_KEYS
        .iter()
        .find_map(|key| find_string(&value, key))
        .map(PathBuf::from)
}

fn context_cwd() -> PathBuf {
    let raw = std::env::var("HERDR_PLUGIN_CONTEXT_JSON").ok();
    context_cwd_from(raw.as_deref())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

fn find_string(value: &serde_json::Value, key: &str) -> Option<String> {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::String(s)) = map.get(key) {
                return Some(s.clone());
            }
            map.values().find_map(|v| find_string(v, key))
        }
        serde_json::Value::Array(items) => items.iter().find_map(|v| find_string(v, key)),
        _ => None,
    }
}

/// Walk up from `start` looking for a git checkout.
pub fn git_root(start: &Path) -> Option<PathBuf> {
    let mut cur = start.canonicalize().ok()?;
    loop {
        if cur.join(".git").exists() {
            return Some(cur);
        }
        cur = cur.parent()?.to_path_buf();
    }
}

// ---- herdr-invoked entry points -------------------------------------------

fn startup(paths: &Paths, config: &Config) -> Result<()> {
    let file = Config::write_default(paths)?;
    let store = store_of(paths)?;

    // Refresh every tracked repo's overlay, so a flow edited in git takes effect
    // the next time herdr comes up.
    for repo in store.list_repos()? {
        let path = PathBuf::from(&repo.path);
        if !path.exists() {
            continue;
        }
        if let Err(e) = overlay::sync_repo(&store, &path, &config.default_agent) {
            eprintln!("herdr-code-board: syncing {} failed: {e:#}", repo.path);
        }
    }

    if daemon::ensure_running(paths)? {
        println!("herdr-code-board: timer daemon started");
    }
    println!("herdr-code-board: ready ({})", file.display());
    Ok(())
}

fn event(paths: &Paths, config: &Config) -> Result<()> {
    let ev = PluginEvent::from_env();

    let Some(_lock) = DispatchLock::acquire(&paths.dispatch_lock(), daemon::LOCK_TIMEOUT)? else {
        // Another hook is mid-dispatch. It will sweep the ready queue anyway,
        // so dropping this one is safe and better than piling up processes.
        return Ok(());
    };

    let store = store_of(paths)?;
    let mut exec = Executor::new(store, herdr(), config.clone());

    if let (Some(pane), Some(status)) = (&ev.pane_id, ev.agent_status) {
        exec.on_agent_status(pane, status)?;
    }
    if ev.is_pane_gone() {
        if let Some(pane) = &ev.pane_id {
            exec.on_pane_gone(pane)?;
        }
    }
    if ev.is_workspace_gone() {
        if let Some(ws) = &ev.workspace_id {
            exec.on_workspace_gone(ws)?;
        }
    }

    exec.dispatch_ready()?;
    Ok(())
}

fn open_board() -> Result<()> {
    CliHerdr::new()
        .call_raw(&[
            "plugin",
            "pane",
            "open",
            "--plugin",
            "herdr-code-board",
            "--entrypoint",
            "board",
        ])
        .context("asking herdr to open the board pane")?;
    Ok(())
}

// ---- cards -----------------------------------------------------------------

fn resolve_repo_for(
    store: &Store,
    config: &Config,
    explicit: Option<&str>,
    cwd: Option<PathBuf>,
) -> Result<Option<Repo>> {
    if let Some(needle) = explicit {
        if let Some(r) = store.resolve_repo(needle)? {
            return Ok(Some(r));
        }
        // Not tracked yet, but it may well be sitting on disk. Track it rather
        // than making the user run a second command for no reason.
        return match find_on_disk(store, config, needle)? {
            Some(path) => {
                let report = overlay::sync_repo(store, &path, &config.default_agent)?;
                eprintln!("tracking {}", path.display());
                store.get_repo(&report.repo_id)
            }
            None => bail!(
                "no repo matches {needle:?}; run `herdr-code-board repo scan` to see what is around"
            ),
        };
    }
    let Some(cwd) = cwd else {
        return Ok(None);
    };
    // Outside a checkout there is no repo to attach to, and inventing one from
    // the current directory would track ~/Downloads the first time you slipped.
    let Some(root) = crate::git::root_of(&cwd) else {
        return Ok(None);
    };
    let path = root.to_string_lossy().to_string();
    if let Some(found) = store.find_repo_by_path(&path)? {
        return Ok(Some(found));
    }
    // Track it on the spot: this is what makes adding a card a one-step action.
    let report = overlay::sync_repo(store, &root, &config.default_agent)?;
    store.get_repo(&report.repo_id)
}

fn add(paths: &Paths, config: &Config, args: AddArgs, cwd: Option<PathBuf>) -> Result<()> {
    let store = store_of(paths)?;
    let repo = resolve_repo_for(&store, config, args.repo.as_deref(), cwd)?;

    let agent_kind = args
        .agent
        .clone()
        .or_else(|| repo.as_ref().and_then(|r| r.default_agent.clone()))
        .unwrap_or_else(|| config.default_agent.clone());
    if !agents::is_known_kind(&agent_kind) {
        bail!("{agent_kind:?} is not a herdr agent kind; run `herdr agent` for the list");
    }

    // A worktree needs something to branch from. Default it to whatever the repo
    // is actually on, and refuse a base that does not exist rather than letting
    // `worktree create` fail later with `invalid reference`.
    let base = if args.needs_base() {
        resolve_base(repo.as_ref(), args.base.as_deref())?
    } else {
        args.base.clone()
    };

    let card = store.create_card(&NewCard {
        key: None,
        title: args.title.clone(),
        prompt: args.resolve_prompt()?,
        repo_id: repo.as_ref().map(|r| r.id.clone()),
        tags: args.tags.clone(),
        agent_kind,
        model: args
            .model
            .clone()
            .or_else(|| repo.as_ref().and_then(|r| r.default_model.clone())),
        extra_args: args.args.clone(),
        placement: args.placement_with_base(base),
        column: if args.start {
            Column::Ready
        } else {
            Column::Backlog
        },
        priority: args.priority,
        auto_complete: !args.review,
        auto_answer: args.auto_answer,
        max_retries: args.retries,
    })?;

    println!(
        "{}  {}  [{}]  {}",
        card.id,
        card.title,
        card.column,
        repo.as_ref()
            .map(|r| r.name.as_str())
            .unwrap_or("no repo — runs from $HOME")
    );
    if let Placement::Worktree { branch, base } = &card.placement {
        println!(
            "  worktree {} from {}",
            branch.replace("{card}", &card.slug()),
            base.as_deref().unwrap_or("the repo's current branch")
        );
    }
    if args.start {
        dispatch_now(paths, config)?;
    }
    Ok(())
}

/// The branch a worktree should be cut from.
///
/// Unset means "wherever the repo is now", which is what someone queuing work
/// from that checkout means. A named base is checked against the real branch
/// list, because the alternative is a dispatch that dies on `invalid reference`
/// minutes later.
pub fn resolve_base(repo: Option<&Repo>, requested: Option<&str>) -> Result<Option<String>> {
    let Some(repo) = repo else {
        return Ok(requested.map(str::to_string));
    };
    let path = Path::new(&repo.path);
    match requested {
        None => Ok(crate::git::head_branch(path)),
        Some(want) => {
            let branches = crate::git::branches(path).unwrap_or_default();
            if branches.is_empty() || branches.iter().any(|b| b == want) {
                return Ok(Some(want.to_string()));
            }
            bail!(
                "{:?} is not a branch of {}. Available:\n{}",
                want,
                repo.name,
                branches
                    .iter()
                    .take(20)
                    .map(|b| format!("  {b}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        }
    }
}

/// Take the dispatch lock and start whatever is ready.
fn dispatch_now(paths: &Paths, config: &Config) -> Result<()> {
    let started = if daemon::sweep_once(paths, config, herdr())? {
        "swept"
    } else {
        "busy"
    };
    if started == "busy" {
        println!("(another dispatch is in flight; the queue will be picked up)");
    }
    Ok(())
}

fn card_or_die(store: &Store, needle: &str) -> Result<Card> {
    store
        .resolve_card(needle)?
        .with_context(|| format!("no card matches {needle:?}"))
}

fn list(paths: &Paths, args: LsArgs) -> Result<()> {
    let store = store_of(paths)?;
    let repos = store.list_repos()?;
    let repo_filter = match &args.repo {
        Some(needle) => Some(
            store
                .resolve_repo(needle)?
                .with_context(|| format!("no tracked repo matches {needle:?}"))?
                .id,
        ),
        None => None,
    };

    let cards: Vec<Card> = store
        .list_cards()?
        .into_iter()
        .filter(|c| args.lane.map(|l| c.column == l.into()).unwrap_or(true))
        .filter(|c| {
            repo_filter
                .as_ref()
                .map(|r| c.repo_id.as_ref() == Some(r))
                .unwrap_or(true)
        })
        .filter(|c| {
            args.tag
                .as_ref()
                .map(|t| c.tags.iter().any(|x| x == t))
                .unwrap_or(true)
        })
        .collect();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&cards)?);
        return Ok(());
    }

    if cards.is_empty() {
        println!("no cards");
        return Ok(());
    }
    for card in cards {
        let repo = card
            .repo_id
            .as_ref()
            .and_then(|id| repos.iter().find(|r| &r.id == id))
            .map(|r| r.name.as_str())
            .unwrap_or("-");
        println!(
            "{:<10} {:<9} {:<12} {:<10} {}",
            &card.id[card.id.len().saturating_sub(8)..],
            card.column.as_str(),
            repo,
            card.agent_kind,
            card.title
        );
    }
    Ok(())
}

fn show(paths: &Paths, needle: &str) -> Result<()> {
    let store = store_of(paths)?;
    let card = card_or_die(&store, needle)?;

    println!("{}  {}", card.id, card.title);
    println!(
        "  lane      {} (for {})",
        card.column,
        ago(card.status_since)
    );
    println!(
        "  agent     {}{}",
        card.agent_kind,
        card.model
            .as_deref()
            .map(|m| format!(" / {m}"))
            .unwrap_or_default()
    );
    println!(
        "  placement {}",
        crate::tui::form::placement_summary(&card.placement)
    );
    if !card.tags.is_empty() {
        println!("  tags      {}", card.tags.join(", "));
    }
    if let Some(pane) = &card.binding.pane_id {
        println!("  pane      {pane}");
    }
    if let Some(err) = &card.last_error {
        println!("  error     {err}");
    }
    if !card.prompt.trim().is_empty() {
        println!("\n  prompt:\n{}", indent(&card.prompt, "    "));
    }

    let rules = store.rules_for_card(&card.id, card.repo_id.as_deref())?;
    if !rules.is_empty() {
        println!("\n  rules:");
        for r in rules {
            println!(
                "    {} -> {}{}",
                r.trigger.describe(),
                r.action.describe(),
                if r.max_fires > 0 {
                    format!("  ({}/{} fired)", r.fired, r.max_fires)
                } else {
                    String::new()
                }
            );
        }
    }

    let runs = store.runs_for_card(&card.id, 10)?;
    if !runs.is_empty() {
        println!("\n  runs:");
        for r in runs {
            println!(
                "    #{} {} {}",
                r.attempt,
                r.outcome.as_deref().unwrap_or("open"),
                ago(r.started_at)
            );
            if let Some(d) = r.detail {
                println!("{}", indent(&d, "        "));
            }
        }
    }
    Ok(())
}

fn indent(text: &str, prefix: &str) -> String {
    text.lines()
        .map(|l| format!("{prefix}{l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Shorten to `max` characters, marking the cut.
pub fn ellipsize(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    format!(
        "{}…",
        s.chars().take(max.saturating_sub(1)).collect::<String>()
    )
}

/// A compact "3m ago" for timestamps in listings.
pub fn ago(at: i64) -> String {
    let secs = (now() - at).max(0);
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3_600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h{:02}m", s / 3_600, (s % 3_600) / 60),
        s => format!("{}d", s / 86_400),
    }
}

fn move_card(paths: &Paths, config: &Config, needle: &str, lane: Column) -> Result<()> {
    let store = store_of(paths)?;
    let card = card_or_die(&store, needle)?;
    store.set_lane(&card.id, lane)?;
    println!("{} -> {lane}", card.title);
    if lane == Column::Ready {
        dispatch_now(paths, config)?;
    }
    Ok(())
}

fn retry(paths: &Paths, config: &Config, needle: &str) -> Result<()> {
    let store = store_of(paths)?;
    let card = card_or_die(&store, needle)?;
    store.clear_binding(&card.id)?;
    store.reset_rule_fires(&card.id)?;
    store.set_error(&card.id, None)?;
    store.set_lane(&card.id, Column::Ready)?;
    println!("{} queued for another attempt", card.title);
    dispatch_now(paths, config)
}

fn cancel(paths: &Paths, needle: &str, close_pane: bool) -> Result<()> {
    let store = store_of(paths)?;
    let card = card_or_die(&store, needle)?;
    if close_pane {
        if let Some(pane) = &card.binding.pane_id {
            herdr().close_pane(pane)?;
        }
    }
    store.finish_open_run(&card.id, "cancelled", Some("by hand"))?;
    store.clear_binding(&card.id)?;
    store.set_lane(&card.id, Column::Cancelled)?;
    println!("{} cancelled", card.title);
    Ok(())
}

fn remove(paths: &Paths, needle: &str) -> Result<()> {
    let store = store_of(paths)?;
    let card = card_or_die(&store, needle)?;
    if card.column.is_live() {
        bail!(
            "{} is still running in {}; cancel it first",
            card.title,
            card.binding.pane_id.as_deref().unwrap_or("a pane")
        );
    }
    store.delete_card(&card.id)?;
    println!("{} deleted", card.title);
    Ok(())
}

fn link(paths: &Paths, args: LinkArgs) -> Result<()> {
    let store = store_of(paths)?;
    let from = card_or_die(&store, &args.from)?;
    let trigger = args.trigger()?;

    if args.to.is_empty() {
        bail!("name at least one card to enqueue");
    }
    let mut targets = Vec::new();
    for needle in &args.to {
        targets.push(card_or_die(&store, needle)?.id);
    }

    store.add_rule(
        Some(&from.id),
        None,
        &trigger,
        &Action::Enqueue {
            cards: targets.clone(),
        },
        args.max_fires,
    )?;
    println!(
        "{}: {} -> enqueue {} card(s)",
        from.title,
        trigger.describe(),
        targets.len()
    );
    Ok(())
}

// ---- repos -----------------------------------------------------------------

/// Every checkout on disk, annotated with whether the board already tracks it.
pub struct Candidate {
    pub found: crate::git::FoundRepo,
    pub tracked: bool,
}

/// Find repositories, marking the ones already on the board.
pub fn scan(store: &Store, roots: Vec<crate::git::ScanRoot>) -> Result<Vec<Candidate>> {
    let tracked: Vec<String> = store.list_repos()?.into_iter().map(|r| r.path).collect();
    Ok(crate::git::discover(&roots)
        .into_iter()
        .map(|found| Candidate {
            tracked: tracked.iter().any(|p| Path::new(p) == found.path),
            found,
        })
        .collect())
}

fn scan_repos(paths: &Paths, config: &Config, args: crate::cli::ScanArgs) -> Result<()> {
    let store = store_of(paths)?;
    let depth = args.depth.unwrap_or(config.scan_depth);
    let roots = if args.paths.is_empty() {
        config.roots()
    } else {
        args.paths
            .iter()
            .map(|p| crate::git::ScanRoot {
                path: p.clone(),
                depth,
            })
            .collect()
    };

    let mut found = scan(&store, roots)?;
    if let Some(filter) = &args.filter {
        let needle = filter.to_lowercase();
        found.retain(|c| {
            c.found.name.to_lowercase().contains(&needle)
                || c.found
                    .path
                    .to_string_lossy()
                    .to_lowercase()
                    .contains(&needle)
        });
    }

    if args.json {
        let rows: Vec<serde_json::Value> = found
            .iter()
            .map(|c| {
                serde_json::json!({
                    "name": c.found.name,
                    "path": c.found.path,
                    "branch": c.found.branch,
                    "tracked": c.tracked,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }

    if found.is_empty() {
        println!("no checkouts found; try `repo scan <path>` or set scan_roots in config.toml");
        return Ok(());
    }

    let mut added = 0;
    for c in &found {
        if args.add && !c.tracked {
            overlay::sync_repo(&store, &c.found.path, &config.default_agent)?;
            added += 1;
        }
        println!(
            "{} {:<24} {:<22} {}",
            if c.tracked || args.add { "*" } else { " " },
            ellipsize(&c.found.name, 24),
            ellipsize(c.found.branch.as_deref().unwrap_or("(detached)"), 22),
            c.found.path.display()
        );
    }
    println!();
    if args.add {
        println!("{} found, {added} newly tracked", found.len());
    } else {
        let tracked = found.iter().filter(|c| c.tracked).count();
        println!(
            "{} found, {tracked} tracked (* )  —  `repo add <name>` to track one, `repo scan --add` for all",
            found.len()
        );
    }
    Ok(())
}

/// Resolve a repo the user named loosely: a path, a tracked name, or the name of
/// something the scan can find.
fn find_on_disk(store: &Store, config: &Config, needle: &str) -> Result<Option<PathBuf>> {
    let direct = crate::config::expand_home(needle);
    if direct.is_dir() {
        return Ok(crate::git::root_of(&direct));
    }
    let lower = needle.to_lowercase();
    let mut matches: Vec<PathBuf> = scan(store, config.roots())?
        .into_iter()
        .filter(|c| c.found.name.to_lowercase() == lower)
        .map(|c| c.found.path)
        .collect();
    if matches.is_empty() {
        matches = scan(store, config.roots())?
            .into_iter()
            .filter(|c| c.found.name.to_lowercase().contains(&lower))
            .map(|c| c.found.path)
            .collect();
    }
    match matches.len() {
        0 => Ok(None),
        1 => Ok(Some(matches.remove(0))),
        _ => bail!(
            "{needle:?} matches {} checkouts; name the path:\n{}",
            matches.len(),
            matches
                .iter()
                .map(|p| format!("  {}", p.display()))
                .collect::<Vec<_>>()
                .join("\n")
        ),
    }
}

fn repo(paths: &Paths, config: &Config, cmd: RepoCommand) -> Result<()> {
    let store = store_of(paths)?;
    match cmd {
        RepoCommand::Scan(args) => scan_repos(paths, config, args),
        RepoCommand::Add {
            path,
            name,
            tags,
            max_parallel,
            agent,
            model,
        } => {
            let root = match path {
                // An argument may be a path or just a project name.
                Some(given) => match find_on_disk(&store, config, &given.to_string_lossy())? {
                    Some(found) => found,
                    None => bail!(
                        "no checkout matches {:?}; run `repo scan` to see what is around",
                        given.display()
                    ),
                },
                // No argument: use the repo you are standing in.
                None => match crate::git::root_of(&context_cwd()) {
                    Some(found) => found,
                    None => return suggest_repos(&store, config),
                },
            };
            let report = overlay::sync_repo(&store, &root, &config.default_agent)?;
            let mut repo = store
                .get_repo(&report.repo_id)?
                .context("the repo vanished mid-sync")?;
            if let Some(v) = name {
                repo.name = v;
            }
            if !tags.is_empty() {
                repo.tags = tags;
            }
            if let Some(v) = max_parallel {
                repo.max_parallel = v;
            }
            if agent.is_some() {
                repo.default_agent = agent;
            }
            if model.is_some() {
                repo.default_model = model;
            }
            let repo = store.upsert_repo(&repo)?;
            let branch = crate::git::head_branch(Path::new(&repo.path));
            println!(
                "tracking {} on {}",
                repo.name,
                branch.as_deref().unwrap_or("(detached)")
            );
            println!("  {}", repo.path);
            if report.created + report.updated > 0 {
                println!(
                    "  {} card(s) imported from {}, {} updated",
                    report.created,
                    overlay::OVERLAY_FILE,
                    report.updated
                );
            }
            println!("\nnext: herdr-code-board add \"Title\" -p \"prompt\" --start");
            Ok(())
        }
        RepoCommand::Ls => {
            let repos = store.list_repos()?;
            if repos.is_empty() {
                return suggest_repos(&store, config);
            }
            let cards = store.list_cards()?;
            println!(
                "{:<18} {:<20} {:>3} {:>5} {:<18} PATH",
                "REPO", "BRANCH", "PAR", "CARDS", "TAGS"
            );
            for r in repos {
                let mine = cards.iter().filter(|c| c.repo_id.as_ref() == Some(&r.id));
                let total = mine.clone().count();
                let live = mine.filter(|c| c.column.is_live()).count();
                // Long branch names are common; keep the columns lined up.
                let branch = ellipsize(
                    &crate::git::head_branch(Path::new(&r.path))
                        .unwrap_or_else(|| "(detached)".into()),
                    20,
                );
                println!(
                    "{:<18} {:<20} {:>3} {:>5} {:<18} {}",
                    r.name,
                    branch,
                    r.max_parallel,
                    if live > 0 {
                        format!("{total}/{live}")
                    } else {
                        total.to_string()
                    },
                    r.tags.join(","),
                    r.path
                );
            }
            Ok(())
        }
        RepoCommand::Rm { repo } => {
            let found = store
                .resolve_repo(&repo)?
                .with_context(|| format!("no tracked repo matches {repo:?}"))?;
            store.delete_repo(&found.id)?;
            println!("{} untracked", found.name);
            Ok(())
        }
    }
}

/// Nothing tracked and nowhere obvious to start: show what is on disk rather
/// than an error telling the user to go find it themselves.
fn suggest_repos(store: &Store, config: &Config) -> Result<()> {
    let found = scan(store, config.roots())?;
    if found.is_empty() {
        println!("no checkouts found under your home directory.");
        println!("point it somewhere: herdr-code-board repo scan ~/where/your/code/is");
        return Ok(());
    }
    println!("not inside a repository. {} found nearby:\n", found.len());
    for c in found.iter().take(15) {
        println!(
            "{} {:<24} {:<20} {}",
            if c.tracked { "*" } else { " " },
            ellipsize(&c.found.name, 24),
            ellipsize(c.found.branch.as_deref().unwrap_or("(detached)"), 20),
            c.found.path.display()
        );
    }
    if found.len() > 15 {
        println!("  … and {} more", found.len() - 15);
    }
    println!("\ntrack one:  herdr-code-board repo add <name>");
    println!("track all:  herdr-code-board repo scan --add");
    Ok(())
}

fn sync(paths: &Paths, config: &Config, which: Vec<PathBuf>) -> Result<()> {
    let store = store_of(paths)?;
    let targets: Vec<PathBuf> = if which.is_empty() {
        store
            .list_repos()?
            .into_iter()
            .map(|r| r.path.into())
            .collect()
    } else {
        which
            .into_iter()
            .map(|p| git_root(&p).unwrap_or(p))
            .collect()
    };

    if targets.is_empty() {
        println!("no repos tracked; run `repo add` inside one");
        return Ok(());
    }
    for path in targets {
        if !path.exists() {
            println!("{}: missing, skipped", path.display());
            continue;
        }
        match overlay::sync_repo(&store, &path, &config.default_agent) {
            Ok(r) => println!(
                "{}: {} created, {} updated, {} rules",
                path.display(),
                r.created,
                r.updated,
                r.rules
            ),
            Err(e) => println!("{}: {e:#}", path.display()),
        }
    }
    Ok(())
}

// ---- doctor ----------------------------------------------------------------

fn doctor(paths: &Paths, config: &Config) -> Result<()> {
    let mut problems = 0;

    println!("paths");
    println!("  config  {}", paths.config_file().display());
    println!("  state   {}", paths.database().display());
    if !paths.from_herdr {
        println!("  note    resolved without herdr's injected environment; these are");
        println!("          herdr's own plugin directories, so this is the same board");
        println!("          the event hooks write to.");
    }

    println!("\nherdr");
    let api = CliHerdr::new();
    println!("  binary  {}", api.binary().to_string_lossy());
    match api.workspaces() {
        Ok(ws) => println!("  socket  ok ({} workspace(s))", ws.len()),
        Err(e) => {
            problems += 1;
            println!("  socket  FAILED: {e:#}");
        }
    }
    if std::env::var("HERDR_ENV").as_deref() != Ok("1") {
        println!("  note    not running inside a herdr pane; that is fine for CLI use");
    }

    println!("\nboard");
    match store_of(paths) {
        Ok(store) => {
            let cards = store.list_cards()?;
            let live = store.live_cards()?.len();
            println!("  schema  v{}", crate::store::migrations::latest_version());
            println!("  cards   {} total, {live} live", cards.len());
            println!("  repos   {}", store.list_repos()?.len());
        }
        Err(e) => {
            problems += 1;
            println!("  store   FAILED: {e:#}");
        }
    }

    println!("\ntimer daemon");
    match DispatchLock::try_acquire(&paths.engine_lock()) {
        Ok(None) => println!("  running"),
        Ok(Some(_)) => println!("  not running (timed rules will not fire; run `startup`)"),
        Err(e) => println!("  unknown: {e:#}"),
    }

    println!("\nagent model flags");
    println!(
        "  verified on this machine: {}",
        agents::VERIFIED_MODEL_FLAGS.join(", ")
    );
    let guessed: Vec<&str> = config
        .model_flags
        .keys()
        .map(String::as_str)
        .filter(|k| !agents::VERIFIED_MODEL_FLAGS.contains(k))
        .collect();
    if !guessed.is_empty() {
        println!(
            "  assumed (override in config.toml): {}",
            guessed.join(", ")
        );
    }
    let unmapped: Vec<&str> = agents::KINDS
        .iter()
        .copied()
        .filter(|k| !config.model_flags.contains_key(*k))
        .collect();
    if !unmapped.is_empty() {
        println!(
            "  no mapping, will fall back to {}: {}",
            agents::FALLBACK_MODEL_FLAG,
            unmapped.join(", ")
        );
    }

    println!("\nauto answer");
    println!(
        "  allow_auto_answer = {} (per-card opt-in is also required)",
        config.allow_auto_answer
    );

    if problems > 0 {
        bail!("{problems} check(s) failed");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ellipsize_keeps_columns_aligned() {
        assert_eq!(ellipsize("main", 10), "main");
        assert_eq!(ellipsize("feature/syntax-highlight", 10), "feature/s…");
        assert_eq!(ellipsize("exactlyten", 10), "exactlyten");
    }

    #[test]
    fn ago_reads_like_a_human_wrote_it() {
        let t = now();
        assert_eq!(ago(t), "0s");
        assert_eq!(ago(t - 90), "1m");
        assert_eq!(ago(t - 3_700), "1h01m");
        assert_eq!(ago(t - 200_000), "2d");
        // A timestamp from the future must not underflow.
        assert_eq!(ago(t + 500), "0s");
    }

    #[test]
    fn git_root_walks_up_from_a_subdirectory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        let deep = dir.path().join("src/engine");
        std::fs::create_dir_all(&deep).unwrap();
        assert_eq!(
            git_root(&deep).unwrap().canonicalize().unwrap(),
            dir.path().canonicalize().unwrap()
        );
    }

    #[test]
    fn git_root_is_none_outside_a_checkout() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(git_root(dir.path()), None);
    }

    /// Real payload captured from `herdr plugin action invoke` on 0.8.2. It is
    /// flat and uses prefixed key names, so a naive lookup for `cwd` finds nothing.
    #[test]
    fn the_context_cwd_is_read_from_herdrs_real_action_payload() {
        let raw = r#"{
            "correlation_id": "cli:plugin",
            "focused_pane_agent": "claude",
            "focused_pane_cwd": "/home/sazar/Documents/rustock",
            "focused_pane_id": "w18:p1",
            "invocation_source": "cli",
            "tab_id": "w18:t1",
            "workspace_cwd": "/home/sazar/Documents/rustock",
            "workspace_id": "w18"
        }"#;
        assert_eq!(
            context_cwd_from(Some(raw)),
            Some(PathBuf::from("/home/sazar/Documents/rustock"))
        );
    }

    #[test]
    fn the_focused_pane_wins_over_the_workspace_root() {
        let raw = r#"{"workspace_cwd":"/repo","focused_pane_cwd":"/repo/sub"}"#;
        assert_eq!(
            context_cwd_from(Some(raw)),
            Some(PathBuf::from("/repo/sub"))
        );
    }

    #[test]
    fn a_nested_payload_still_resolves() {
        let raw = r#"{"pane":{"pane_id":"w1:p1","cwd":"/repo/erp"}}"#;
        assert_eq!(
            context_cwd_from(Some(raw)),
            Some(PathBuf::from("/repo/erp"))
        );
    }

    #[test]
    fn no_context_means_no_answer_rather_than_a_wrong_one() {
        assert_eq!(context_cwd_from(None), None);
        assert_eq!(context_cwd_from(Some("not json")), None);
        assert_eq!(context_cwd_from(Some(r#"{"workspace_id":"w1"}"#)), None);
    }
}
