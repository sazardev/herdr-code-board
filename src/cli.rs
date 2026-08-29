//! Command line surface.
//!
//! Herdr invokes some of these itself (`startup`, `event`, the manifest actions);
//! the rest exist so a human — or an agent already running inside herdr — can
//! drive the board from a shell.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "herdr-code-board",
    version,
    about = "Kanban queue for agentic prompts inside Herdr",
    long_about = None
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Open the kanban TUI. This is the plugin's `board` pane entrypoint.
    Board,

    /// Run the timer daemon in the foreground.
    Daemon,

    /// Plugin startup hook: seed config, sync tracked repos, start the daemon.
    Startup,

    /// Plugin event hook: react to one herdr event, then dispatch what fits.
    Event,

    /// Ask herdr to open the board pane.
    Open,

    /// Add a card to the board.
    Add(AddArgs),

    /// Add a card for the repo this shell is in. Backs the `enqueue-here` action.
    EnqueueHere(AddArgs),

    /// List cards.
    Ls(LsArgs),

    /// Show one card in full, with its rules and run history.
    Show { card: String },

    /// Move a card to another lane. `ready` makes the engine pick it up.
    Move {
        card: String,
        #[arg(value_enum)]
        lane: LaneArg,
    },

    /// Re-dispatch a card from scratch.
    Retry { card: String },

    /// Stop a card and release its pane.
    Cancel {
        card: String,
        /// Also close the herdr pane the card was using.
        #[arg(long)]
        close_pane: bool,
    },

    /// Delete a card and its rules.
    Rm { card: String },

    /// Link two cards: when `from` reaches a state, enqueue `to`.
    Link(LinkArgs),

    /// Repositories the board dispatches into.
    #[command(subcommand)]
    Repo(RepoCommand),

    /// Re-import `.herdr-board.toml` from tracked repos.
    Sync {
        /// Sync only these paths instead of every tracked repo.
        paths: Vec<PathBuf>,
    },

    /// Check that herdr, the database and the agent CLIs are usable.
    Doctor,
}

#[derive(Debug, Subcommand)]
pub enum RepoCommand {
    /// Look for checkouts on disk and show what is already tracked.
    Scan(ScanArgs),

    /// Track a repository, importing its overlay file if it has one.
    ///
    /// With no path, tracks the repository you are standing in. If you are not
    /// in one, it lists what it found instead of failing.
    Add {
        path: Option<PathBuf>,
        #[arg(long)]
        name: Option<String>,
        #[arg(long = "tag")]
        tags: Vec<String>,
        /// How many cards of this repo may run at once.
        #[arg(long)]
        max_parallel: Option<u32>,
        #[arg(long)]
        agent: Option<String>,
        #[arg(long)]
        model: Option<String>,
    },
    /// List tracked repositories.
    Ls,
    /// Stop tracking a repository. Its cards become global.
    Rm { repo: String },
}

#[derive(Debug, Args)]
pub struct ScanArgs {
    /// Look here instead of the configured roots.
    pub paths: Vec<PathBuf>,
    /// Track everything found, not just list it.
    #[arg(long)]
    pub add: bool,
    /// Directory levels to descend. Defaults to the configured `scan_depth`.
    #[arg(long)]
    pub depth: Option<usize>,
    /// Only repositories whose name or path contains this.
    #[arg(long)]
    pub filter: Option<String>,
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct AddArgs {
    /// Card title. Also used for the herdr agent name and the pane label.
    pub title: String,

    /// The prompt to send once the agent is up.
    #[arg(short, long)]
    pub prompt: Option<String>,

    /// Read the prompt from a file, or from stdin with `-`.
    #[arg(long, conflicts_with = "prompt")]
    pub prompt_file: Option<PathBuf>,

    /// Repo id, path or name. Defaults to the current directory's repo.
    #[arg(short, long)]
    pub repo: Option<String>,

    /// Agent kind, e.g. claude, codex, opencode.
    #[arg(short, long)]
    pub agent: Option<String>,

    #[arg(short, long)]
    pub model: Option<String>,

    /// Extra arguments passed to the agent CLI after `--`.
    #[arg(long = "arg")]
    pub args: Vec<String>,

    #[arg(long = "tag")]
    pub tags: Vec<String>,

    #[arg(long, value_enum, default_value_t = PlacementArg::Split)]
    pub placement: PlacementArg,

    #[arg(long, value_enum)]
    pub direction: Option<DirectionArg>,

    #[arg(long)]
    pub ratio: Option<f64>,

    /// Branch for `--placement worktree`. `{card}` expands to the card slug.
    #[arg(long, default_value = "board/{card}")]
    pub branch: String,

    /// Base ref for `--placement worktree`. Defaults to the repo's current branch.
    #[arg(long)]
    pub base: Option<String>,

    #[arg(long, default_value_t = 0)]
    pub priority: i64,

    /// Park the card in `waiting` for a human instead of completing it.
    #[arg(long)]
    pub review: bool,

    /// Let rules answer this card's approval dialogs. Needs `allow_auto_answer`
    /// in config.toml too.
    #[arg(long)]
    pub auto_answer: bool,

    #[arg(long, default_value_t = 0)]
    pub retries: u32,

    /// Queue it straight away instead of leaving it in the backlog.
    #[arg(long)]
    pub start: bool,
}

#[derive(Debug, Args)]
pub struct LsArgs {
    /// Only this lane.
    #[arg(long, value_enum)]
    pub lane: Option<LaneArg>,
    /// Only this repo.
    #[arg(long)]
    pub repo: Option<String>,
    /// Only cards carrying this tag.
    #[arg(long)]
    pub tag: Option<String>,
    /// Machine-readable output.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct LinkArgs {
    /// The card that has to reach a state.
    pub from: String,
    /// The card to enqueue when it does.
    pub to: Vec<String>,
    /// Which state triggers the link.
    #[arg(long, value_enum, default_value_t = TriggerArg::Done)]
    pub on: TriggerArg,
    /// Required with `--on waiting`; optional with `--on blocked`. e.g. `15m`.
    #[arg(long)]
    pub after: Option<String>,
    /// Fire at most this many times. 0 means unlimited.
    #[arg(long, default_value_t = 0)]
    pub max_fires: u32,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum LaneArg {
    Backlog,
    Ready,
    Running,
    Waiting,
    Blocked,
    Review,
    Done,
    Failed,
    Cancelled,
}

impl From<LaneArg> for crate::model::Column {
    fn from(v: LaneArg) -> Self {
        use crate::model::Column as C;
        match v {
            LaneArg::Backlog => C::Backlog,
            LaneArg::Ready => C::Ready,
            LaneArg::Running => C::Running,
            LaneArg::Waiting => C::Waiting,
            LaneArg::Blocked => C::Blocked,
            LaneArg::Review => C::Review,
            LaneArg::Done => C::Done,
            LaneArg::Failed => C::Failed,
            LaneArg::Cancelled => C::Cancelled,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum PlacementArg {
    /// Take an idle pane in the repo's workspace, splitting only if none is free.
    Reuse,
    /// Always split a new pane. One vertical column per card.
    Split,
    /// A new tab in the repo's workspace.
    Tab,
    /// A new workspace rooted at the repo.
    Workspace,
    /// A git worktree, which herdr opens as its own workspace.
    Worktree,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum DirectionArg {
    Right,
    Down,
}

impl From<DirectionArg> for crate::model::SplitDirection {
    fn from(v: DirectionArg) -> Self {
        match v {
            DirectionArg::Right => crate::model::SplitDirection::Right,
            DirectionArg::Down => crate::model::SplitDirection::Down,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum TriggerArg {
    /// The card completed.
    Done,
    /// The card's turn ended and it is parked for a human.
    Waiting,
    /// The card failed.
    Failed,
    /// The card hit an approval dialog.
    Blocked,
    /// A human moved it to review.
    Review,
}

impl AddArgs {
    /// `base` is resolved by the caller, which knows the repo and can default it
    /// to whatever branch that repo is actually on.
    pub fn placement_with_base(&self, base: Option<String>) -> crate::model::Placement {
        use crate::model::Placement as P;
        match self.placement {
            PlacementArg::Reuse => P::Reuse,
            PlacementArg::Split => P::Split {
                direction: self.direction.map(Into::into),
                ratio: self.ratio,
            },
            PlacementArg::Tab => P::NewTab,
            PlacementArg::Workspace => P::NewWorkspace,
            PlacementArg::Worktree => P::Worktree {
                branch: self.branch.clone(),
                base,
            },
        }
    }

    pub fn placement(&self) -> crate::model::Placement {
        self.placement_with_base(self.base.clone())
    }

    /// True when the placement actually cuts a branch, so a base ref matters.
    pub fn needs_base(&self) -> bool {
        matches!(self.placement, PlacementArg::Worktree)
    }

    /// The prompt text, reading the file or stdin when asked to.
    pub fn resolve_prompt(&self) -> anyhow::Result<String> {
        if let Some(p) = &self.prompt {
            return Ok(p.clone());
        }
        let Some(path) = &self.prompt_file else {
            return Ok(String::new());
        };
        if path.as_os_str() == "-" {
            let mut buf = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)?;
            return Ok(buf);
        }
        Ok(std::fs::read_to_string(path)?)
    }
}

impl LinkArgs {
    pub fn trigger(&self) -> anyhow::Result<crate::model::Trigger> {
        use crate::model::Trigger as T;
        let after = self
            .after
            .as_deref()
            .map(crate::overlay::parse_duration)
            .transpose()?;
        Ok(match (self.on, after) {
            (TriggerArg::Done, _) => T::Done,
            (TriggerArg::Review, _) => T::Review,
            (TriggerArg::Failed, _) => T::Failed,
            (TriggerArg::Blocked, None) => T::Blocked,
            (TriggerArg::Blocked, Some(seconds)) => T::BlockedFor { seconds },
            (TriggerArg::Waiting, Some(seconds)) => T::WaitingFor { seconds },
            (TriggerArg::Waiting, None) => {
                anyhow::bail!("--on waiting needs --after, e.g. --on waiting --after 15m")
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn waiting_links_require_a_duration() {
        let mut args = LinkArgs {
            from: "a".into(),
            to: vec!["b".into()],
            on: TriggerArg::Waiting,
            after: None,
            max_fires: 0,
        };
        assert!(args.trigger().is_err());
        args.after = Some("15m".into());
        assert_eq!(
            args.trigger().unwrap(),
            crate::model::Trigger::WaitingFor { seconds: 900 }
        );
    }

    #[test]
    fn a_bare_blocked_link_fires_on_arrival_but_takes_a_delay_if_given() {
        let mut args = LinkArgs {
            from: "a".into(),
            to: vec![],
            on: TriggerArg::Blocked,
            after: None,
            max_fires: 0,
        };
        assert_eq!(args.trigger().unwrap(), crate::model::Trigger::Blocked);
        args.after = Some("5m".into());
        assert_eq!(
            args.trigger().unwrap(),
            crate::model::Trigger::BlockedFor { seconds: 300 }
        );
    }

    #[test]
    fn placement_flags_map_onto_the_model() {
        let parse = |argv: &[&str]| {
            let cli = Cli::try_parse_from(argv).unwrap();
            match cli.command.unwrap() {
                Command::Add(a) => a.placement(),
                _ => unreachable!(),
            }
        };
        assert_eq!(
            parse(&["herdr-code-board", "add", "t", "--placement", "reuse"]),
            crate::model::Placement::Reuse
        );
        assert_eq!(
            parse(&[
                "herdr-code-board",
                "add",
                "t",
                "--placement",
                "split",
                "--direction",
                "down",
                "--ratio",
                "0.3"
            ]),
            crate::model::Placement::Split {
                direction: Some(crate::model::SplitDirection::Down),
                ratio: Some(0.3)
            }
        );
        assert_eq!(
            parse(&[
                "herdr-code-board",
                "add",
                "t",
                "--placement",
                "worktree",
                "--base",
                "main"
            ]),
            crate::model::Placement::Worktree {
                branch: "board/{card}".into(),
                base: Some("main".into())
            }
        );
    }
}
