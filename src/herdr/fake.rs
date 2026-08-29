//! An in-memory [`HerdrApi`] for tests.
//!
//! It models just enough of herdr to exercise dispatch: workspaces with panes,
//! panes that are either at a prompt or occupied by an agent, and a call log so
//! a test can assert on the exact CLI surface the engine used.

use std::sync::Mutex;

use anyhow::{bail, Result};

use super::types::*;
use super::{HerdrApi, Sound, Tokens, WorkspaceCreated};
use crate::model::{AgentStatus, SplitDirection};

#[derive(Debug, Default)]
struct State {
    workspaces: Vec<WorkspaceInfo>,
    panes: Vec<PaneInfo>,
    agents: Vec<AgentInfo>,
    /// Panes with a foreground process, i.e. not startable.
    busy_panes: Vec<String>,
    /// Screen contents returned by `read_visible`, per pane.
    screens: Vec<(String, String)>,
    calls: Vec<String>,
    /// Substring of a call that should fail, and the error to raise.
    failures: Vec<(String, String)>,
    seq: u32,
}

#[derive(Debug, Default)]
pub struct FakeHerdr {
    state: Mutex<State>,
}

impl FakeHerdr {
    pub fn new() -> Self {
        Self::default()
    }

    /// Seed a workspace with one pane sitting at its shell prompt.
    pub fn with_workspace(self, workspace_id: &str, label: &str, cwd: &str) -> Self {
        {
            let mut s = self.state.lock().unwrap();
            s.workspaces.push(WorkspaceInfo {
                workspace_id: workspace_id.into(),
                label: Some(label.into()),
                active_tab_id: Some(format!("{workspace_id}:t1")),
                ..Default::default()
            });
            s.panes.push(PaneInfo {
                pane_id: format!("{workspace_id}:p1"),
                workspace_id: Some(workspace_id.into()),
                tab_id: Some(format!("{workspace_id}:t1")),
                cwd: Some(cwd.into()),
                foreground_cwd: Some(cwd.into()),
                ..Default::default()
            });
        }
        self
    }

    /// Make a pane look occupied, so the dispatcher must split instead of reusing it.
    pub fn set_busy(&self, pane_id: &str, busy: bool) {
        let mut s = self.state.lock().unwrap();
        s.busy_panes.retain(|p| p != pane_id);
        if busy {
            s.busy_panes.push(pane_id.into());
        }
    }

    pub fn set_screen(&self, pane_id: &str, text: &str) {
        let mut s = self.state.lock().unwrap();
        s.screens.retain(|(p, _)| p != pane_id);
        s.screens.push((pane_id.into(), text.into()));
    }

    /// Make any call whose logged form contains `needle` fail.
    pub fn fail_on(&self, needle: &str, message: &str) {
        self.state
            .lock()
            .unwrap()
            .failures
            .push((needle.into(), message.into()));
    }

    /// Every call made so far, in order, in `command arg arg` form.
    pub fn calls(&self) -> Vec<String> {
        self.state.lock().unwrap().calls.clone()
    }

    pub fn calls_matching(&self, needle: &str) -> Vec<String> {
        self.calls()
            .into_iter()
            .filter(|c| c.contains(needle))
            .collect()
    }

    pub fn agent_for_pane(&self, pane_id: &str) -> Option<AgentInfo> {
        self.state
            .lock()
            .unwrap()
            .agents
            .iter()
            .find(|a| a.pane_id == pane_id)
            .cloned()
    }

    pub fn set_agent_status(&self, pane_id: &str, status: AgentStatus) {
        let mut s = self.state.lock().unwrap();
        if let Some(a) = s.agents.iter_mut().find(|a| a.pane_id == pane_id) {
            a.agent_status = Some(status);
        }
    }

    fn record(&self, call: String) -> Result<()> {
        let mut s = self.state.lock().unwrap();
        if let Some((_, msg)) = s.failures.iter().find(|(n, _)| call.contains(n.as_str())) {
            let msg = msg.clone();
            s.calls.push(call);
            bail!("{msg}");
        }
        s.calls.push(call);
        Ok(())
    }
}

impl HerdrApi for FakeHerdr {
    fn workspaces(&self) -> Result<Vec<WorkspaceInfo>> {
        self.record("workspace list".into())?;
        Ok(self.state.lock().unwrap().workspaces.clone())
    }

    fn create_workspace(&self, cwd: &str, label: &str) -> Result<WorkspaceCreated> {
        self.record(format!("workspace create --cwd {cwd} --label {label}"))?;
        let mut s = self.state.lock().unwrap();
        s.seq += 1;
        let id = format!("wf{}", s.seq);
        let workspace = WorkspaceInfo {
            workspace_id: id.clone(),
            label: Some(label.into()),
            active_tab_id: Some(format!("{id}:t1")),
            ..Default::default()
        };
        let pane = PaneInfo {
            pane_id: format!("{id}:p1"),
            workspace_id: Some(id.clone()),
            tab_id: Some(format!("{id}:t1")),
            cwd: Some(cwd.into()),
            foreground_cwd: Some(cwd.into()),
            ..Default::default()
        };
        s.workspaces.push(workspace.clone());
        s.panes.push(pane.clone());
        Ok(WorkspaceCreated {
            workspace,
            tab: Some(TabInfo {
                tab_id: format!("{id}:t1"),
                workspace_id: Some(id),
                label: Some(label.into()),
            }),
            root_pane: Some(pane),
        })
    }

    fn panes(&self, workspace: Option<&str>) -> Result<Vec<PaneInfo>> {
        self.record(format!("pane list {}", workspace.unwrap_or("-")))?;
        let s = self.state.lock().unwrap();
        Ok(s.panes
            .iter()
            .filter(|p| workspace.is_none() || p.workspace_id.as_deref() == workspace)
            .cloned()
            .collect())
    }

    fn pane(&self, pane_id: &str) -> Result<PaneInfo> {
        self.record(format!("pane get {pane_id}"))?;
        let s = self.state.lock().unwrap();
        s.panes
            .iter()
            .find(|p| p.pane_id == pane_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no such pane {pane_id}"))
    }

    fn process_info(&self, pane_id: &str) -> Result<ProcessInfo> {
        self.record(format!("pane process-info {pane_id}"))?;
        let s = self.state.lock().unwrap();
        let busy = s.busy_panes.iter().any(|p| p == pane_id);
        Ok(ProcessInfo {
            pane_id: pane_id.into(),
            shell_pid: Some(100),
            foreground_process_group_id: Some(if busy { 200 } else { 100 }),
            foreground_processes: vec![ForegroundProcess {
                pid: Some(if busy { 200 } else { 100 }),
                name: Some(if busy { "claude".into() } else { "fish".into() }),
                cmdline: None,
            }],
        })
    }

    fn layout(&self, pane_id: &str) -> Result<Layout> {
        self.record(format!("pane layout {pane_id}"))?;
        Ok(Layout {
            focused_pane_id: Some(pane_id.into()),
            area: Rect {
                width: 200,
                height: 50,
                ..Default::default()
            },
            panes: vec![LayoutPane {
                pane_id: pane_id.into(),
                rect: Rect {
                    width: 200,
                    height: 50,
                    ..Default::default()
                },
            }],
            ..Default::default()
        })
    }

    fn split(
        &self,
        pane_id: &str,
        direction: SplitDirection,
        cwd: &str,
        ratio: Option<f64>,
    ) -> Result<PaneInfo> {
        self.record(format!(
            "pane split {pane_id} --direction {} --cwd {cwd}{}",
            direction.as_str(),
            ratio.map(|r| format!(" --ratio {r}")).unwrap_or_default()
        ))?;
        let mut s = self.state.lock().unwrap();
        let parent = s
            .panes
            .iter()
            .find(|p| p.pane_id == pane_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no such pane {pane_id}"))?;
        s.seq += 1;
        let ws = parent.workspace_id.clone().unwrap_or_default();
        let pane = PaneInfo {
            pane_id: format!("{ws}:ps{}", s.seq),
            workspace_id: parent.workspace_id.clone(),
            tab_id: parent.tab_id.clone(),
            cwd: Some(cwd.into()),
            foreground_cwd: Some(cwd.into()),
            ..Default::default()
        };
        s.panes.push(pane.clone());
        Ok(pane)
    }

    fn create_tab(&self, workspace: &str, cwd: &str, label: &str) -> Result<WorkspaceCreated> {
        self.record(format!(
            "tab create --workspace {workspace} --cwd {cwd} --label {label}"
        ))?;
        let mut s = self.state.lock().unwrap();
        s.seq += 1;
        let tab_id = format!("{workspace}:t{}", s.seq);
        let pane = PaneInfo {
            pane_id: format!("{workspace}:pt{}", s.seq),
            workspace_id: Some(workspace.into()),
            tab_id: Some(tab_id.clone()),
            cwd: Some(cwd.into()),
            foreground_cwd: Some(cwd.into()),
            ..Default::default()
        };
        s.panes.push(pane.clone());
        let workspace_info = s
            .workspaces
            .iter()
            .find(|w| w.workspace_id == workspace)
            .cloned()
            .unwrap_or(WorkspaceInfo {
                workspace_id: workspace.into(),
                ..Default::default()
            });
        Ok(WorkspaceCreated {
            workspace: workspace_info,
            tab: Some(TabInfo {
                tab_id,
                workspace_id: Some(workspace.into()),
                label: Some(label.into()),
            }),
            root_pane: Some(pane),
        })
    }

    fn close_pane(&self, pane_id: &str) -> Result<()> {
        self.record(format!("pane close {pane_id}"))?;
        let mut s = self.state.lock().unwrap();
        s.panes.retain(|p| p.pane_id != pane_id);
        s.agents.retain(|a| a.pane_id != pane_id);
        Ok(())
    }

    fn rename_pane(&self, pane_id: &str, label: &str) -> Result<()> {
        self.record(format!("pane rename {pane_id} {label}"))
    }

    fn read_visible(&self, pane_id: &str, lines: u32) -> Result<String> {
        self.record(format!(
            "pane read {pane_id} --source visible --lines {lines}"
        ))?;
        let s = self.state.lock().unwrap();
        Ok(s.screens
            .iter()
            .find(|(p, _)| p == pane_id)
            .map(|(_, t)| t.clone())
            .unwrap_or_default())
    }

    fn agents(&self) -> Result<Vec<AgentInfo>> {
        self.record("agent list".into())?;
        Ok(self.state.lock().unwrap().agents.clone())
    }

    fn start_agent(&self, name: &str, kind: &str, pane_id: &str, args: &[String]) -> Result<()> {
        self.record(format!(
            "agent start {name} --kind {kind} --pane {pane_id}{}",
            if args.is_empty() {
                String::new()
            } else {
                format!(" -- {}", args.join(" "))
            }
        ))?;
        let mut s = self.state.lock().unwrap();
        if s.busy_panes.iter().any(|p| p == pane_id) {
            bail!("pane_not_available");
        }
        let (workspace_id, tab_id, cwd) = s
            .panes
            .iter()
            .find(|p| p.pane_id == pane_id)
            .map(|p| (p.workspace_id.clone(), p.tab_id.clone(), p.cwd.clone()))
            .unwrap_or_default();
        s.agents.push(AgentInfo {
            pane_id: pane_id.into(),
            workspace_id,
            tab_id,
            agent: Some(kind.into()),
            name: Some(name.into()),
            agent_status: Some(AgentStatus::Idle),
            cwd,
        });
        s.busy_panes.push(pane_id.into());
        if let Some(p) = s.panes.iter_mut().find(|p| p.pane_id == pane_id) {
            p.agent = Some(kind.into());
            p.agent_status = Some(AgentStatus::Idle);
        }
        Ok(())
    }

    fn prompt_agent(&self, target: &str, text: &str) -> Result<()> {
        self.record(format!("agent prompt {target} {text}"))
    }

    fn send_keys(&self, target: &str, keys: &[String]) -> Result<()> {
        self.record(format!("agent send-keys {target} {}", keys.join(" ")))
    }

    fn create_worktree(
        &self,
        workspace: &str,
        branch: &str,
        base: Option<&str>,
        label: Option<&str>,
    ) -> Result<WorkspaceCreated> {
        self.record(format!(
            "worktree create --workspace {workspace} --branch {branch}{}{}",
            base.map(|b| format!(" --base {b}")).unwrap_or_default(),
            label.map(|l| format!(" --label {l}")).unwrap_or_default(),
        ))?;
        // Herdr opens a worktree as its own workspace.
        self.create_workspace(&format!("/worktrees/{branch}"), label.unwrap_or(branch))
    }

    fn notify(&self, title: &str, body: Option<&str>, sound: Sound) -> Result<()> {
        self.record(format!(
            "notification show {title} {} [{}]",
            body.unwrap_or(""),
            sound.as_str()
        ))
    }

    fn report_pane_tokens(&self, pane_id: &str, source: &str, tokens: &Tokens) -> Result<()> {
        self.record(format!(
            "pane report-metadata {pane_id} --source {source} {}",
            render_tokens(tokens)
        ))
    }

    fn report_workspace_tokens(
        &self,
        workspace_id: &str,
        source: &str,
        tokens: &Tokens,
    ) -> Result<()> {
        self.record(format!(
            "workspace report-metadata {workspace_id} --source {source} {}",
            render_tokens(tokens)
        ))
    }
}

fn render_tokens(tokens: &Tokens) -> String {
    tokens
        .iter()
        .map(|(k, v)| {
            if v.is_empty() {
                format!("-{k}")
            } else {
                format!("{k}={v}")
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starting_an_agent_occupies_the_pane() {
        let h = FakeHerdr::new().with_workspace("w1", "erp", "/repo");
        h.start_agent(
            "rev-abc",
            "claude",
            "w1:p1",
            &["--model".into(), "opus".into()],
        )
        .unwrap();

        assert!(!h.process_info("w1:p1").unwrap().is_at_prompt());
        assert_eq!(
            h.agent_for_pane("w1:p1").unwrap().name.as_deref(),
            Some("rev-abc")
        );
        // A second agent cannot take the same pane.
        assert!(h.start_agent("other", "codex", "w1:p1", &[]).is_err());
        assert_eq!(
            h.calls_matching("agent start")[0],
            "agent start rev-abc --kind claude --pane w1:p1 -- --model opus"
        );
    }

    #[test]
    fn splitting_produces_a_free_pane_in_the_same_tab() {
        let h = FakeHerdr::new().with_workspace("w1", "erp", "/repo");
        let pane = h
            .split("w1:p1", SplitDirection::Right, "/repo", Some(0.5))
            .unwrap();
        assert_eq!(pane.tab_id.as_deref(), Some("w1:t1"));
        assert!(h.process_info(&pane.pane_id).unwrap().is_at_prompt());
    }

    #[test]
    fn injected_failures_still_record_the_call() {
        let h = FakeHerdr::new().with_workspace("w1", "erp", "/repo");
        h.fail_on("agent start", "agent_not_ready");
        let err = h.start_agent("a", "claude", "w1:p1", &[]).unwrap_err();
        assert!(err.to_string().contains("agent_not_ready"));
        assert_eq!(h.calls_matching("agent start").len(), 1);
    }
}
