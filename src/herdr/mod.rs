//! The herdr control surface this plugin depends on.
//!
//! Everything the engine does to herdr goes through [`HerdrApi`]. The real
//! implementation shells out to `HERDR_BIN_PATH`; [`fake::FakeHerdr`] records the
//! same calls in memory so the dispatcher and the rule engine can be tested
//! without a running server.

pub mod client;
pub mod fake;
pub mod types;

use anyhow::Result;

use crate::model::SplitDirection;
use types::{AgentInfo, Layout, PaneInfo, ProcessInfo, TabInfo, WorkspaceInfo};

/// What `workspace create` and `worktree create` hand back.
#[derive(Debug, Clone)]
pub struct WorkspaceCreated {
    pub workspace: WorkspaceInfo,
    pub tab: Option<TabInfo>,
    pub root_pane: Option<PaneInfo>,
}

pub trait HerdrApi: Send + Sync {
    fn workspaces(&self) -> Result<Vec<WorkspaceInfo>>;
    fn create_workspace(&self, cwd: &str, label: &str) -> Result<WorkspaceCreated>;

    fn panes(&self, workspace: Option<&str>) -> Result<Vec<PaneInfo>>;
    fn pane(&self, pane_id: &str) -> Result<PaneInfo>;
    fn process_info(&self, pane_id: &str) -> Result<ProcessInfo>;
    fn layout(&self, pane_id: &str) -> Result<Layout>;
    fn split(
        &self,
        pane_id: &str,
        direction: SplitDirection,
        cwd: &str,
        ratio: Option<f64>,
    ) -> Result<PaneInfo>;
    fn create_tab(&self, workspace: &str, cwd: &str, label: &str) -> Result<WorkspaceCreated>;
    fn close_pane(&self, pane_id: &str) -> Result<()>;
    fn rename_pane(&self, pane_id: &str, label: &str) -> Result<()>;

    /// Read the pane's current screen.
    ///
    /// Only the `visible` source is exposed on purpose. `recent` and
    /// `recent-unwrapped` take ~4.4s and visibly repaint the pane the user is
    /// watching (measured and documented by herdr-agent-quota); a background
    /// board must never do that.
    fn read_visible(&self, pane_id: &str, lines: u32) -> Result<String>;

    fn agents(&self) -> Result<Vec<AgentInfo>>;
    fn start_agent(&self, name: &str, kind: &str, pane_id: &str, args: &[String]) -> Result<()>;
    fn prompt_agent(&self, target: &str, text: &str) -> Result<()>;
    fn send_keys(&self, target: &str, keys: &[String]) -> Result<()>;

    fn create_worktree(
        &self,
        workspace: &str,
        branch: &str,
        base: Option<&str>,
        label: Option<&str>,
    ) -> Result<WorkspaceCreated>;

    fn notify(&self, title: &str, body: Option<&str>) -> Result<()>;
}
