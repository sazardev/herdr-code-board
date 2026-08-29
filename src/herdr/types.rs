//! Wire types for the herdr socket API.
//!
//! These mirror only the fields this plugin reads. Every struct is tolerant of
//! unknown and missing fields, because the shapes differ per command and herdr
//! does not promise them as a stable contract.

use serde::{Deserialize, Serialize};

use crate::model::AgentStatus;

/// `{"id": ..., "result": {...}}` or `{"id": ..., "error": {...}}`.
#[derive(Debug, Deserialize)]
pub struct Envelope<T> {
    #[serde(default)]
    pub result: Option<T>,
    #[serde(default)]
    pub error: Option<ApiError>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ApiError {
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (&self.code, &self.message) {
            (Some(c), Some(m)) => write!(f, "{c}: {m}"),
            (Some(c), None) => write!(f, "{c}"),
            (None, Some(m)) => write!(f, "{m}"),
            (None, None) => write!(f, "unknown herdr error"),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct WorkspaceInfo {
    pub workspace_id: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub active_tab_id: Option<String>,
    #[serde(default)]
    pub focused: bool,
    #[serde(default)]
    pub pane_count: u32,
    #[serde(default)]
    pub agent_status: Option<AgentStatus>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct TabInfo {
    pub tab_id: String,
    #[serde(default)]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PaneInfo {
    pub pane_id: String,
    #[serde(default)]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub tab_id: Option<String>,
    /// Working directory of the pane's shell.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Working directory of whatever is currently in the foreground.
    #[serde(default)]
    pub foreground_cwd: Option<String>,
    /// The agent kind herdr recognized in this pane, if any.
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub agent_status: Option<AgentStatus>,
    #[serde(default)]
    pub focused: bool,
    #[serde(default)]
    pub terminal_title_stripped: Option<String>,
}

impl PaneInfo {
    /// The directory this pane is really sitting in.
    pub fn effective_cwd(&self) -> Option<&str> {
        self.foreground_cwd.as_deref().or(self.cwd.as_deref())
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct AgentInfo {
    pub pane_id: String,
    #[serde(default)]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub tab_id: Option<String>,
    /// Herdr's agent kind label, e.g. `claude`.
    #[serde(default)]
    pub agent: Option<String>,
    /// The unique name assigned via `agent start <name>` or `agent rename`.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub agent_status: Option<AgentStatus>,
    #[serde(default)]
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ProcessInfo {
    pub pane_id: String,
    #[serde(default)]
    pub shell_pid: Option<i64>,
    #[serde(default)]
    pub foreground_process_group_id: Option<i64>,
    #[serde(default)]
    pub foreground_processes: Vec<ForegroundProcess>,
}

impl ProcessInfo {
    /// True when the shell itself is in the foreground, which is herdr's
    /// definition of a pane that `agent start` will accept.
    pub fn is_at_prompt(&self) -> bool {
        let Some(shell) = self.shell_pid else {
            return false;
        };
        self.foreground_processes
            .iter()
            .all(|p| p.pid == Some(shell))
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ForegroundProcess {
    #[serde(default)]
    pub pid: Option<i64>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub cmdline: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
pub struct Rect {
    #[serde(default)]
    pub x: u32,
    #[serde(default)]
    pub y: u32,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct LayoutPane {
    pub pane_id: String,
    #[serde(default)]
    pub rect: Rect,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Layout {
    #[serde(default)]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub tab_id: Option<String>,
    #[serde(default)]
    pub focused_pane_id: Option<String>,
    #[serde(default)]
    pub area: Rect,
    #[serde(default)]
    pub panes: Vec<LayoutPane>,
}

impl Layout {
    pub fn rect_of(&self, pane_id: &str) -> Option<Rect> {
        self.panes
            .iter()
            .find(|p| p.pane_id == pane_id)
            .map(|p| p.rect)
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PaneRead {
    #[serde(default)]
    pub pane_id: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub lines: Vec<String>,
}

impl PaneRead {
    /// The read text, whichever field herdr populated for this source.
    pub fn body(&self) -> String {
        if let Some(t) = &self.text {
            return t.clone();
        }
        self.lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pane_is_at_prompt_only_when_the_shell_is_foreground() {
        let busy = ProcessInfo {
            pane_id: "w1:p1".into(),
            shell_pid: Some(100),
            foreground_process_group_id: Some(200),
            foreground_processes: vec![ForegroundProcess {
                pid: Some(200),
                name: Some("claude".into()),
                cmdline: None,
            }],
        };
        assert!(!busy.is_at_prompt());

        let idle = ProcessInfo {
            foreground_processes: vec![ForegroundProcess {
                pid: Some(100),
                name: Some("fish".into()),
                cmdline: None,
            }],
            ..busy.clone()
        };
        assert!(idle.is_at_prompt());

        // No shell pid means we cannot prove it is free, so we must not claim it is.
        let unknown = ProcessInfo {
            shell_pid: None,
            ..idle.clone()
        };
        assert!(!unknown.is_at_prompt());
    }

    #[test]
    fn pane_info_prefers_the_foreground_cwd() {
        let p = PaneInfo {
            cwd: Some("/home/u".into()),
            foreground_cwd: Some("/home/u/repo".into()),
            ..Default::default()
        };
        assert_eq!(p.effective_cwd(), Some("/home/u/repo"));
    }

    /// Real payload captured from `herdr pane list` on 0.8.2.
    #[test]
    fn pane_info_parses_a_real_payload_with_unknown_fields() {
        let raw = r#"{
            "agent": "claude",
            "agent_status": "working",
            "cwd": "/home/sazar/Documents/rustock",
            "focused": false,
            "foreground_cwd": "/home/sazar/Documents/rustock",
            "pane_id": "w18:p1",
            "revision": 51,
            "scroll": {"max_offset_from_bottom": 0, "offset_from_bottom": 0, "viewport_rows": 47},
            "tab_id": "w18:t1",
            "terminal_id": "term_65a304788acd15",
            "terminal_title_stripped": "Rediseno app",
            "tokens": {"quota_5h": "5h 70% 1h40m"},
            "workspace_id": "w18"
        }"#;
        let p: PaneInfo = serde_json::from_str(raw).unwrap();
        assert_eq!(p.pane_id, "w18:p1");
        assert_eq!(p.agent.as_deref(), Some("claude"));
        assert_eq!(p.agent_status, Some(AgentStatus::Working));
        assert_eq!(p.effective_cwd(), Some("/home/sazar/Documents/rustock"));
    }

    #[test]
    fn an_error_envelope_parses_and_renders() {
        let raw = r#"{"id":"x","error":{"code":"agent_blocked","message":"waiting on approval"}}"#;
        let env: Envelope<serde_json::Value> = serde_json::from_str(raw).unwrap();
        assert!(env.result.is_none());
        assert_eq!(
            env.error.unwrap().to_string(),
            "agent_blocked: waiting on approval"
        );
    }
}
