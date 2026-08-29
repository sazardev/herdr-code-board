//! Plugin paths and user configuration.
//!
//! Herdr injects `HERDR_PLUGIN_CONFIG_DIR` and `HERDR_PLUGIN_STATE_DIR` for every
//! runtime command. Outside herdr (tests, `--help`, local hacking) we fall back to
//! XDG paths so the binary stays usable on its own.

use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Directories the plugin owns.
#[derive(Debug, Clone)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub state_dir: PathBuf,
}

impl Paths {
    pub fn resolve() -> Result<Self> {
        let config_dir = match env::var_os("HERDR_PLUGIN_CONFIG_DIR") {
            Some(v) => PathBuf::from(v),
            None => xdg_dir("XDG_CONFIG_HOME", ".config")?.join("herdr-code-board"),
        };
        let state_dir = match env::var_os("HERDR_PLUGIN_STATE_DIR") {
            Some(v) => PathBuf::from(v),
            None => xdg_dir("XDG_STATE_HOME", ".local/state")?.join("herdr-code-board"),
        };
        std::fs::create_dir_all(&config_dir)
            .with_context(|| format!("creating config dir {}", config_dir.display()))?;
        std::fs::create_dir_all(&state_dir)
            .with_context(|| format!("creating state dir {}", state_dir.display()))?;
        Ok(Self {
            config_dir,
            state_dir,
        })
    }

    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    pub fn database(&self) -> PathBuf {
        self.state_dir.join("board.db")
    }

    /// Advisory lock that keeps the timer daemon a singleton.
    pub fn engine_lock(&self) -> PathBuf {
        self.state_dir.join("engine.lock")
    }

    /// Advisory lock held around dispatch, so concurrent event hooks and the
    /// daemon can never start the same card twice.
    pub fn dispatch_lock(&self) -> PathBuf {
        self.state_dir.join("dispatch.lock")
    }

    /// Touched by one-shot hooks to wake the engine between socket events.
    pub fn nudge_file(&self) -> PathBuf {
        self.state_dir.join("nudge")
    }

    pub fn engine_log(&self) -> PathBuf {
        self.state_dir.join("engine.log")
    }
}

fn xdg_dir(var: &str, fallback: &str) -> Result<PathBuf> {
    if let Some(v) = env::var_os(var) {
        return Ok(PathBuf::from(v));
    }
    let home = env::var_os("HOME").context("neither $HOME nor the XDG variable is set")?;
    Ok(Path::new(&home).join(fallback))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Master switch for rules that type into an agent's approval dialog.
    /// Cards also need their own `auto_answer` flag; both must be true.
    pub allow_auto_answer: bool,
    /// Default agent kind for new cards.
    pub default_agent: String,
    /// Default cards live at once, per repo, when the repo does not override it.
    pub default_max_parallel: u32,
    /// Seconds between engine sweeps when nothing else wakes it.
    pub engine_tick_seconds: u64,
    /// TUI redraw poll, in milliseconds.
    pub tui_poll_ms: u64,
    /// Desktop/herdr notifications on rule fires and failures.
    pub notifications: bool,
    /// How the `--model` value is passed to each agent CLI.
    pub model_flags: BTreeMap<String, ModelFlag>,
    pub theme: Theme,
}

/// How a given agent CLI accepts a model selection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ModelFlag {
    /// `["--model", "<value>"]`
    Flag(String),
    /// Explicit argv template; `{model}` is substituted.
    Template(Vec<String>),
}

impl ModelFlag {
    pub fn render(&self, model: &str) -> Vec<String> {
        match self {
            ModelFlag::Flag(flag) => vec![flag.clone(), model.to_string()],
            ModelFlag::Template(parts) => {
                parts.iter().map(|p| p.replace("{model}", model)).collect()
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Theme {
    pub accent: String,
    pub running: String,
    pub waiting: String,
    pub blocked: String,
    pub done: String,
    pub failed: String,
    pub muted: String,
}

impl Default for Theme {
    /// Gruvbox dark, to match a common herdr terminal setup.
    fn default() -> Self {
        Self {
            accent: "#83a598".into(),
            running: "#b8bb26".into(),
            waiting: "#fabd2f".into(),
            blocked: "#fb4934".into(),
            done: "#8ec07c".into(),
            failed: "#cc241d".into(),
            muted: "#928374".into(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            allow_auto_answer: false,
            default_agent: "claude".into(),
            default_max_parallel: 2,
            engine_tick_seconds: 30,
            tui_poll_ms: 250,
            notifications: true,
            model_flags: crate::agents::default_model_flags(),
            theme: Theme::default(),
        }
    }
}

impl Config {
    pub fn load(paths: &Paths) -> Result<Self> {
        let file = paths.config_file();
        if !file.exists() {
            return Ok(Config::default());
        }
        let raw = std::fs::read_to_string(&file)
            .with_context(|| format!("reading {}", file.display()))?;
        let mut cfg: Config =
            toml::from_str(&raw).with_context(|| format!("parsing {}", file.display()))?;
        // User entries override the built-ins rather than replacing the whole table.
        let mut merged = crate::agents::default_model_flags();
        merged.extend(cfg.model_flags);
        cfg.model_flags = merged;
        Ok(cfg)
    }

    /// Write the default config so users have something to edit.
    pub fn write_default(paths: &Paths) -> Result<PathBuf> {
        let file = paths.config_file();
        if !file.exists() {
            let body = toml::to_string_pretty(&Config::default())?;
            std::fs::write(&file, body).with_context(|| format!("writing {}", file.display()))?;
        }
        Ok(file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_flag_renders_both_shapes() {
        assert_eq!(
            ModelFlag::Flag("--model".into()).render("opus"),
            vec!["--model", "opus"]
        );
        assert_eq!(
            ModelFlag::Template(vec!["-c".into(), "model={model}".into()]).render("opus"),
            vec!["-c", "model=opus"]
        );
    }

    #[test]
    fn default_config_round_trips_through_toml() {
        let body = toml::to_string_pretty(&Config::default()).unwrap();
        let back: Config = toml::from_str(&body).unwrap();
        assert_eq!(back.default_agent, Config::default().default_agent);
        assert!(!back.allow_auto_answer, "auto answer must default to off");
    }

    #[test]
    fn user_model_flags_merge_over_builtins() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            config_dir: dir.path().to_path_buf(),
            state_dir: dir.path().to_path_buf(),
        };
        std::fs::write(
            paths.config_file(),
            "[model_flags]\nclaude = \"--custom\"\n",
        )
        .unwrap();
        let cfg = Config::load(&paths).unwrap();
        assert_eq!(
            cfg.model_flags.get("claude"),
            Some(&ModelFlag::Flag("--custom".into()))
        );
        // A built-in the user did not mention survives the merge.
        assert!(cfg.model_flags.contains_key("codex"));
    }
}
