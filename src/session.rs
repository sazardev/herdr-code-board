//! Which herdr session a card belongs to.
//!
//! The board is one database per user, but herdr can run several sessions at
//! once, each its own server behind its own socket. An event hook inherits the
//! socket of whichever session fired it, so without this a card queued while you
//! worked in one session could be started in another — whichever happened to
//! sweep first.
//!
//! A card therefore records the session it was queued from, and only that
//! session runs it. A card with no session recorded is unclaimed and any session
//! may take it, which is what a single-session install always looks like.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Deserialize;

/// The name herdr gives its unnamed session.
pub const DEFAULT: &str = "default";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Session {
    pub name: String,
    pub socket_path: PathBuf,
    #[serde(default)]
    pub running: bool,
    #[serde(default, rename = "default")]
    pub is_default: bool,
}

#[derive(Debug, Deserialize)]
struct SessionList {
    #[serde(default)]
    sessions: Vec<Session>,
}

/// Ask herdr which sessions exist.
pub fn list(bin: &std::ffi::OsStr) -> Result<Vec<Session>> {
    let out = std::process::Command::new(bin)
        .args(["session", "list", "--json"])
        .output()?;
    if !out.status.success() {
        return Ok(Vec::new());
    }
    let parsed: SessionList = serde_json::from_slice(&out.stdout)?;
    Ok(parsed.sessions)
}

/// The socket this process is pointed at, if it is inside herdr at all.
pub fn current_socket() -> Option<PathBuf> {
    std::env::var_os("HERDR_SOCKET_PATH").map(PathBuf::from)
}

/// A session name derived from a socket path, for when herdr cannot be asked.
///
/// Named sessions live at `<config>/sessions/<name>/herdr.sock`; the unnamed one
/// sits directly in the config directory.
pub fn name_from_socket(socket: &Path) -> String {
    let Some(dir) = socket.parent() else {
        return DEFAULT.to_string();
    };
    let looks_named = dir
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n == "sessions")
        .unwrap_or(false);
    if looks_named {
        if let Some(name) = dir.file_name() {
            return name.to_string_lossy().to_string();
        }
    }
    DEFAULT.to_string()
}

/// The name of the session this process is talking to.
///
/// `None` outside herdr — a plain shell — where there is no session to claim a
/// card for.
pub fn current_name() -> Option<String> {
    current_socket().map(|s| name_from_socket(&s))
}

/// How the engine finds out what sessions exist.
///
/// Behind a closure so a sweep can read the list once instead of shelling out
/// per card, and so tests can describe a multi-session machine without needing
/// one.
pub type Directory = std::sync::Arc<dyn Fn() -> Vec<Session> + Send + Sync>;

/// The real thing: ask the installed herdr.
pub fn herdr_directory() -> Directory {
    std::sync::Arc::new(|| {
        let bin = std::env::var_os("HERDR_BIN_PATH").unwrap_or_else(|| "herdr".into());
        list(&bin).unwrap_or_default()
    })
}

/// A fixed list, for tests.
pub fn fixed_directory(sessions: Vec<Session>) -> Directory {
    std::sync::Arc::new(move || sessions.clone())
}

impl Session {
    pub fn new(name: &str, socket: &str, running: bool) -> Self {
        Self {
            name: name.into(),
            socket_path: PathBuf::from(socket),
            running,
            is_default: name == DEFAULT,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_unnamed_session_is_called_default() {
        assert_eq!(
            name_from_socket(Path::new("/home/u/.config/herdr/herdr.sock")),
            "default"
        );
    }

    #[test]
    fn a_named_session_takes_its_directory_name() {
        assert_eq!(
            name_from_socket(Path::new(
                "/home/u/.config/herdr/sessions/board-test/herdr.sock"
            )),
            "board-test"
        );
        assert_eq!(
            name_from_socket(Path::new("/tmp/x/sessions/work/herdr.sock")),
            "work"
        );
    }

    #[test]
    fn an_unfamiliar_path_falls_back_to_default_rather_than_guessing() {
        for weird in ["/herdr.sock", "herdr.sock", "/a/b/c/herdr.sock"] {
            assert_eq!(name_from_socket(Path::new(weird)), "default");
        }
    }

    /// Real payload from `herdr session list --json` on 0.8.2.
    #[test]
    fn the_session_list_parses_herdrs_own_output() {
        let raw = r#"{"sessions":[
            {"default":true,"name":"default","running":true,
             "session_dir":"/home/sazar/.config/herdr",
             "socket_path":"/home/sazar/.config/herdr/herdr.sock"},
            {"default":false,"name":"board-test","running":false,
             "session_dir":"/home/sazar/.config/herdr/sessions/board-test",
             "socket_path":"/home/sazar/.config/herdr/sessions/board-test/herdr.sock"}
        ]}"#;
        let parsed: SessionList = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.sessions.len(), 2);
        assert!(parsed.sessions[0].is_default);
        assert!(parsed.sessions[0].running);
        assert_eq!(parsed.sessions[1].name, "board-test");
        assert!(!parsed.sessions[1].running);
        assert_eq!(
            parsed.sessions[1].socket_path,
            PathBuf::from("/home/sazar/.config/herdr/sessions/board-test/herdr.sock")
        );
    }
}
