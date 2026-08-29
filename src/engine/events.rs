//! Decoding `HERDR_PLUGIN_EVENT_JSON`.
//!
//! Herdr's plugin event payloads are nested and the shape differs per event; the
//! herdr-agent-quota plugin documents this explicitly and walks the tree rather
//! than assuming a path. This does the same: find the fields wherever they are,
//! and treat a missing field as "no information" rather than an error.

use serde_json::Value;

use crate::model::AgentStatus;

/// What the engine can learn from one plugin event.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginEvent {
    /// The event name, e.g. `pane_agent_status_changed`.
    pub kind: Option<String>,
    pub pane_id: Option<String>,
    pub workspace_id: Option<String>,
    pub agent_status: Option<AgentStatus>,
}

impl PluginEvent {
    pub fn parse(raw: &str) -> Self {
        let Ok(value) = serde_json::from_str::<Value>(raw) else {
            return Self::default();
        };
        Self {
            kind: find_string(&value, "event")
                .or_else(|| find_string(&value, "type"))
                .map(normalize_kind),
            pane_id: find_string(&value, "pane_id"),
            workspace_id: find_string(&value, "workspace_id"),
            agent_status: find_string(&value, "agent_status")
                .and_then(|s| s.parse::<AgentStatus>().ok()),
        }
    }

    /// Read the event herdr put in the environment for this hook invocation.
    pub fn from_env() -> Self {
        match std::env::var("HERDR_PLUGIN_EVENT_JSON") {
            Ok(raw) => Self::parse(&raw),
            Err(_) => Self::default(),
        }
    }

    /// True when this event says the card's pane is gone.
    pub fn is_pane_gone(&self) -> bool {
        matches!(
            self.kind.as_deref(),
            Some("pane_closed") | Some("pane_exited")
        )
    }

    pub fn is_workspace_gone(&self) -> bool {
        self.kind.as_deref() == Some("workspace_closed")
    }
}

/// `pane.closed` and `pane_closed` name the same event; normalize to underscores.
fn normalize_kind(s: String) -> String {
    s.replace('.', "_")
}

/// Depth-first search for the first string value under `key`.
fn find_string(value: &Value, key: &str) -> Option<String> {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(s)) = map.get(key) {
                return Some(s.clone());
            }
            map.values().find_map(|v| find_string(v, key))
        }
        Value::Array(items) => items.iter().find_map(|v| find_string(v, key)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shape documented by herdr-agent-quota for `pane.focused`.
    #[test]
    fn parses_the_documented_nested_shape() {
        let raw = r#"{"event":"pane_focused","data":{"type":"pane_focused","pane_id":"w1:p9","workspace_id":"w1"}}"#;
        let ev = PluginEvent::parse(raw);
        assert_eq!(ev.kind.as_deref(), Some("pane_focused"));
        assert_eq!(ev.pane_id.as_deref(), Some("w1:p9"));
        assert_eq!(ev.workspace_id.as_deref(), Some("w1"));
        assert_eq!(ev.agent_status, None);
    }

    #[test]
    fn parses_an_agent_status_change() {
        let raw = r#"{
            "event": "pane.agent_status_changed",
            "data": {
                "type": "pane_agent_status_changed",
                "pane_id": "w18:p1",
                "workspace_id": "w18",
                "agent_status": "blocked",
                "display_agent": "Claude",
                "state_labels": {"blocked": "needs approval"}
            }
        }"#;
        let ev = PluginEvent::parse(raw);
        assert_eq!(ev.kind.as_deref(), Some("pane_agent_status_changed"));
        assert_eq!(ev.pane_id.as_deref(), Some("w18:p1"));
        assert_eq!(ev.agent_status, Some(AgentStatus::Blocked));
    }

    #[test]
    fn a_flat_payload_works_too() {
        let ev = PluginEvent::parse(r#"{"type":"pane_exited","pane_id":"w1:p2"}"#);
        assert!(ev.is_pane_gone());
        assert_eq!(ev.pane_id.as_deref(), Some("w1:p2"));
    }

    #[test]
    fn unparseable_or_empty_input_yields_no_information() {
        for raw in ["", "not json", "null", "[]"] {
            let ev = PluginEvent::parse(raw);
            assert_eq!(ev, PluginEvent::default(), "input {raw:?}");
            assert!(!ev.is_pane_gone());
        }
    }

    #[test]
    fn an_unknown_agent_status_is_dropped_rather_than_guessed() {
        let ev = PluginEvent::parse(r#"{"pane_id":"w1:p1","agent_status":"thinking"}"#);
        assert_eq!(ev.agent_status, None);
        assert_eq!(ev.pane_id.as_deref(), Some("w1:p1"));
    }

    #[test]
    fn workspace_closed_is_recognized() {
        let ev = PluginEvent::parse(
            r#"{"event":"workspace.closed","data":{"workspace_id":"w7","workspace":null}}"#,
        );
        assert!(ev.is_workspace_gone());
        assert_eq!(ev.workspace_id.as_deref(), Some("w7"));
    }
}
