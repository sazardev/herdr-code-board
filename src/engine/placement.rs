//! Turning a card's placement policy into a concrete herdr pane.
//!
//! Herdr's own agent skill sets the geometry rule this follows: split a wide pane
//! to the right and a narrow or tall pane down, and never stack repeated splits in
//! the same direction until the columns are unusable. `agent start` also refuses
//! anything but a shell pane sitting at its prompt, so every path here ends with a
//! pane we know is free.

use anyhow::{anyhow, Context, Result};

use crate::herdr::types::{PaneInfo, Rect, WorkspaceInfo};
use crate::herdr::{HerdrApi, WorkspaceCreated};
use crate::model::{Placement, SplitDirection};

/// The pane a card's agent will be started in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub workspace_id: String,
    pub tab_id: Option<String>,
    pub pane_id: String,
    /// Set when this placement created a worktree, so cleanup knows we own it.
    pub worktree_path: Option<String>,
}

/// Which way to split a pane of this shape.
///
/// Terminal cells are about twice as tall as they are wide, so a pane is only
/// "wide" once its column count is more than double its row count.
pub fn choose_direction(rect: Rect) -> SplitDirection {
    if rect.height == 0 {
        return SplitDirection::Right;
    }
    if rect.width as f64 / 2.0 >= rect.height as f64 {
        SplitDirection::Right
    } else {
        SplitDirection::Down
    }
}

/// Is this pane free for `agent start`?
fn is_available(api: &dyn HerdrApi, pane: &PaneInfo) -> bool {
    if pane.agent.is_some() {
        return false;
    }
    api.process_info(&pane.pane_id)
        .map(|p| p.is_at_prompt())
        .unwrap_or(false)
}

fn path_contains(root: &str, candidate: &str) -> bool {
    let root = root.trim_end_matches('/');
    candidate == root || candidate.starts_with(&format!("{root}/"))
}

/// Find the workspace already rooted at `repo_path`.
///
/// Herdr workspace records carry no cwd, so the repo is matched through the
/// panes inside each workspace; the label is only a fallback.
pub fn find_workspace(
    api: &dyn HerdrApi,
    repo_path: &str,
    repo_name: &str,
) -> Result<Option<WorkspaceInfo>> {
    let workspaces = api.workspaces()?;
    let panes = api.panes(None)?;

    for ws in &workspaces {
        let matches = panes.iter().any(|p| {
            p.workspace_id.as_deref() == Some(ws.workspace_id.as_str())
                && p.effective_cwd()
                    .map(|c| path_contains(repo_path, c))
                    .unwrap_or(false)
        });
        if matches {
            return Ok(Some(ws.clone()));
        }
    }

    Ok(workspaces
        .into_iter()
        .find(|w| w.label.as_deref() == Some(repo_name)))
}

fn ensure_workspace(api: &dyn HerdrApi, repo_path: &str, repo_name: &str) -> Result<WorkspaceInfo> {
    if let Some(ws) = find_workspace(api, repo_path, repo_name)? {
        return Ok(ws);
    }
    Ok(api
        .create_workspace(repo_path, repo_name)
        .with_context(|| format!("creating a workspace for {repo_path}"))?
        .workspace)
}

/// The pane a fresh workspace or tab handed us, falling back to a lookup when
/// herdr's response did not include the root pane.
fn root_pane_of(api: &dyn HerdrApi, created: &WorkspaceCreated) -> Result<PaneInfo> {
    if let Some(p) = &created.root_pane {
        return Ok(p.clone());
    }
    let ws = &created.workspace.workspace_id;
    api.panes(Some(ws))?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("herdr created workspace {ws} but reported no pane in it"))
}

/// Pick the pane to split from: prefer the widest one, so repeated dispatches
/// subdivide the roomiest column instead of shaving the same sliver.
fn anchor_pane(api: &dyn HerdrApi, workspace_id: &str) -> Result<(PaneInfo, Rect)> {
    let panes = api.panes(Some(workspace_id))?;
    let first = panes
        .first()
        .ok_or_else(|| anyhow!("workspace {workspace_id} has no panes"))?;
    let layout = api.layout(&first.pane_id)?;

    let mut best: Option<(PaneInfo, Rect)> = None;
    for pane in &panes {
        let rect = layout.rect_of(&pane.pane_id).unwrap_or_default();
        let area = u64::from(rect.width) * u64::from(rect.height);
        let better = best
            .as_ref()
            .map(|(_, r)| area > u64::from(r.width) * u64::from(r.height))
            .unwrap_or(true);
        if better {
            best = Some((pane.clone(), rect));
        }
    }
    best.ok_or_else(|| anyhow!("workspace {workspace_id} has no measurable panes"))
}

/// Resolve `placement` into a free pane, creating whatever herdr topology it asks for.
pub fn resolve(
    api: &dyn HerdrApi,
    repo_path: &str,
    repo_name: &str,
    placement: &Placement,
    slug: &str,
) -> Result<Target> {
    match placement {
        Placement::NewWorkspace => {
            let created = api.create_workspace(repo_path, &format!("{repo_name}/{slug}"))?;
            let pane = root_pane_of(api, &created)?;
            Ok(Target {
                workspace_id: created.workspace.workspace_id,
                tab_id: pane.tab_id.clone(),
                pane_id: pane.pane_id,
                worktree_path: None,
            })
        }

        Placement::Worktree { branch, base } => {
            // `worktree create` needs an existing workspace to hang the repo off.
            let host = ensure_workspace(api, repo_path, repo_name)?;
            let branch = branch.replace("{card}", slug);
            let created =
                api.create_worktree(&host.workspace_id, &branch, base.as_deref(), Some(&branch))?;
            let pane = root_pane_of(api, &created)?;
            Ok(Target {
                workspace_id: created.workspace.workspace_id,
                tab_id: pane.tab_id.clone(),
                worktree_path: pane.cwd.clone(),
                pane_id: pane.pane_id,
            })
        }

        Placement::NewTab => {
            let ws = ensure_workspace(api, repo_path, repo_name)?;
            let created = api.create_tab(&ws.workspace_id, repo_path, slug)?;
            let pane = root_pane_of(api, &created)?;
            Ok(Target {
                workspace_id: ws.workspace_id,
                tab_id: pane.tab_id.clone(),
                pane_id: pane.pane_id,
                worktree_path: None,
            })
        }

        Placement::Reuse => {
            let ws = ensure_workspace(api, repo_path, repo_name)?;
            let panes = api.panes(Some(&ws.workspace_id))?;
            if let Some(free) = panes.iter().find(|p| is_available(api, p)) {
                return Ok(Target {
                    workspace_id: ws.workspace_id,
                    tab_id: free.tab_id.clone(),
                    pane_id: free.pane_id.clone(),
                    worktree_path: None,
                });
            }
            split_into(api, &ws.workspace_id, repo_path, None, None)
        }

        Placement::Split { direction, ratio } => {
            let ws = ensure_workspace(api, repo_path, repo_name)?;
            split_into(api, &ws.workspace_id, repo_path, *direction, *ratio)
        }
    }
}

fn split_into(
    api: &dyn HerdrApi,
    workspace_id: &str,
    cwd: &str,
    direction: Option<SplitDirection>,
    ratio: Option<f64>,
) -> Result<Target> {
    let (anchor, rect) = anchor_pane(api, workspace_id)?;
    let direction = direction.unwrap_or_else(|| choose_direction(rect));
    let pane = api
        .split(&anchor.pane_id, direction, cwd, ratio)
        .with_context(|| format!("splitting {} for a new agent", anchor.pane_id))?;
    Ok(Target {
        workspace_id: workspace_id.to_string(),
        tab_id: pane.tab_id.clone().or(anchor.tab_id),
        pane_id: pane.pane_id,
        worktree_path: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::herdr::fake::FakeHerdr;

    fn rect(width: u32, height: u32) -> Rect {
        Rect {
            width,
            height,
            ..Default::default()
        }
    }

    #[test]
    fn wide_panes_split_right_and_tall_panes_split_down() {
        assert_eq!(choose_direction(rect(200, 50)), SplitDirection::Right);
        assert_eq!(choose_direction(rect(100, 50)), SplitDirection::Right);
        assert_eq!(choose_direction(rect(90, 50)), SplitDirection::Down);
        assert_eq!(choose_direction(rect(80, 60)), SplitDirection::Down);
        // Degenerate geometry must not divide by zero.
        assert_eq!(choose_direction(rect(0, 0)), SplitDirection::Right);
    }

    #[test]
    fn a_repo_workspace_is_found_through_its_pane_cwd() {
        let h = FakeHerdr::new()
            .with_workspace("w1", "other", "/home/u/other")
            .with_workspace("w2", "erp", "/home/u/erp/src");
        let found = find_workspace(&h, "/home/u/erp", "erp").unwrap().unwrap();
        assert_eq!(found.workspace_id, "w2", "a subdirectory still matches");
    }

    #[test]
    fn a_sibling_directory_is_not_mistaken_for_the_repo() {
        let h = FakeHerdr::new().with_workspace("w1", "erp2", "/home/u/erp2");
        // /home/u/erp2 must not match the repo at /home/u/erp.
        assert!(find_workspace(&h, "/home/u/erp", "erp").unwrap().is_none());
    }

    #[test]
    fn split_placement_creates_the_workspace_when_the_repo_is_not_open() {
        let h = FakeHerdr::new();
        let target = resolve(&h, "/home/u/erp", "erp", &Placement::default(), "rev-1").unwrap();
        assert_eq!(
            h.calls_matching("workspace create"),
            vec!["workspace create --cwd /home/u/erp --label erp"]
        );
        assert_eq!(h.calls_matching("pane split").len(), 1);
        assert!(target.pane_id.starts_with(&target.workspace_id));
    }

    #[test]
    fn reuse_takes_a_free_pane_without_splitting() {
        let h = FakeHerdr::new().with_workspace("w1", "erp", "/home/u/erp");
        let target = resolve(&h, "/home/u/erp", "erp", &Placement::Reuse, "rev-1").unwrap();
        assert_eq!(target.pane_id, "w1:p1");
        assert!(h.calls_matching("pane split").is_empty());
    }

    #[test]
    fn reuse_falls_back_to_a_split_when_every_pane_is_occupied() {
        let h = FakeHerdr::new().with_workspace("w1", "erp", "/home/u/erp");
        h.set_busy("w1:p1", true);
        let target = resolve(&h, "/home/u/erp", "erp", &Placement::Reuse, "rev-1").unwrap();
        assert_ne!(target.pane_id, "w1:p1");
        assert_eq!(h.calls_matching("pane split").len(), 1);
    }

    #[test]
    fn split_honours_an_explicit_direction_and_ratio() {
        let h = FakeHerdr::new().with_workspace("w1", "erp", "/home/u/erp");
        resolve(
            &h,
            "/home/u/erp",
            "erp",
            &Placement::Split {
                direction: Some(SplitDirection::Down),
                ratio: Some(0.25),
            },
            "rev-1",
        )
        .unwrap();
        assert_eq!(
            h.calls_matching("pane split"),
            vec!["pane split w1:p1 --direction down --cwd /home/u/erp --ratio 0.25"]
        );
    }

    #[test]
    fn a_worktree_card_hangs_its_worktree_off_the_repo_workspace() {
        let h = FakeHerdr::new().with_workspace("w1", "erp", "/home/u/erp");
        let target = resolve(
            &h,
            "/home/u/erp",
            "erp",
            &Placement::Worktree {
                branch: "feat/{card}".into(),
                base: Some("main".into()),
            },
            "rev-1",
        )
        .unwrap();
        assert_eq!(
            h.calls_matching("worktree create"),
            vec![
                "worktree create --workspace w1 --branch feat/rev-1 --base main --label feat/rev-1"
            ]
        );
        assert_ne!(
            target.workspace_id, "w1",
            "a worktree gets its own workspace"
        );
        assert!(target.worktree_path.is_some());
    }

    #[test]
    fn new_tab_placement_stays_inside_the_repo_workspace() {
        let h = FakeHerdr::new().with_workspace("w1", "erp", "/home/u/erp");
        let target = resolve(&h, "/home/u/erp", "erp", &Placement::NewTab, "rev-1").unwrap();
        assert_eq!(target.workspace_id, "w1");
        assert_eq!(h.calls_matching("tab create").len(), 1);
    }
}
