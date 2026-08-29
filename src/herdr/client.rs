//! `HerdrApi` over the `herdr` CLI.
//!
//! The docs are explicit that the CLI *is* the plugin API, and that calling
//! through `HERDR_BIN_PATH` is what keeps a plugin portable across Unix sockets
//! and Windows named pipes. So this shells out rather than speaking the socket
//! protocol; only the engine's long-lived event subscription uses a raw socket.

use std::env;
use std::ffi::OsString;
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use serde::de::DeserializeOwned;

use super::types::*;
use super::{HerdrApi, Sound, Tokens, WorkspaceCreated};
use crate::model::SplitDirection;

pub struct CliHerdr {
    bin: OsString,
    /// Overrides `HERDR_SOCKET_PATH` for the commands we spawn, so one process
    /// can talk to more than one herdr session.
    socket: Option<std::path::PathBuf>,
}

impl Default for CliHerdr {
    fn default() -> Self {
        Self::new()
    }
}

impl CliHerdr {
    pub fn new() -> Self {
        Self {
            bin: env::var_os("HERDR_BIN_PATH").unwrap_or_else(|| OsString::from("herdr")),
            socket: None,
        }
    }

    /// Talk to the herdr server behind `socket` instead of the ambient one.
    pub fn at_socket(socket: &std::path::Path) -> Self {
        Self {
            socket: Some(socket.to_path_buf()),
            ..Self::new()
        }
    }

    pub fn binary(&self) -> &OsString {
        &self.bin
    }

    /// Run a herdr subcommand and return the raw `result` object.
    pub fn call_raw(&self, args: &[&str]) -> Result<serde_json::Value> {
        let mut command = Command::new(&self.bin);
        command.args(args);
        if let Some(socket) = &self.socket {
            command.env("HERDR_SOCKET_PATH", socket);
        }
        let out = command.output().with_context(|| {
            format!("running {} {}", self.bin.to_string_lossy(), args.join(" "))
        })?;

        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);

        // Server errors come back as JSON on stderr with exit status 1;
        // syntax errors exit 2 with plain text.
        let body = if stdout.trim().is_empty() {
            stderr.trim()
        } else {
            stdout.trim()
        };

        if body.is_empty() {
            if out.status.success() {
                return Ok(serde_json::Value::Null);
            }
            bail!("herdr {} failed with {}", args.join(" "), out.status);
        }

        let env: Envelope<serde_json::Value> = serde_json::from_str(body).map_err(|e| {
            anyhow!(
                "herdr {} returned unparseable output ({e}): {}",
                args.join(" "),
                truncate(body, 400)
            )
        })?;

        if let Some(err) = env.error {
            bail!("herdr {}: {err}", args.join(" "));
        }
        Ok(env.result.unwrap_or(serde_json::Value::Null))
    }

    fn call<T: DeserializeOwned>(&self, args: &[&str], field: &str) -> Result<T> {
        let result = self.call_raw(args)?;
        let value = result
            .get(field)
            .cloned()
            .ok_or_else(|| anyhow!("herdr {} had no {field} field", args.join(" ")))?;
        serde_json::from_value(value)
            .with_context(|| format!("decoding {field} from herdr {}", args.join(" ")))
    }

    /// `workspace create`, `tab create` and `worktree create` all return some
    /// subset of workspace/tab/root_pane. Decode whichever fields are present.
    fn call_created(&self, args: &[&str]) -> Result<WorkspaceCreated> {
        let result = self.call_raw(args)?;
        let workspace: WorkspaceInfo = match result.get("workspace") {
            Some(v) => serde_json::from_value(v.clone())?,
            None => serde_json::from_value(result.clone()).with_context(|| {
                format!("herdr {} returned no workspace record", args.join(" "))
            })?,
        };
        Ok(WorkspaceCreated {
            workspace,
            tab: result
                .get("tab")
                .and_then(|v| serde_json::from_value(v.clone()).ok()),
            root_pane: result
                .get("root_pane")
                .or_else(|| result.get("pane"))
                .and_then(|v| serde_json::from_value(v.clone()).ok()),
        })
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}

impl HerdrApi for CliHerdr {
    fn for_session(&self, socket: &std::path::Path) -> Option<std::sync::Arc<dyn HerdrApi>> {
        Some(std::sync::Arc::new(CliHerdr::at_socket(socket)))
    }

    fn workspaces(&self) -> Result<Vec<WorkspaceInfo>> {
        self.call(&["workspace", "list"], "workspaces")
    }

    fn create_workspace(&self, cwd: &str, label: &str) -> Result<WorkspaceCreated> {
        self.call_created(&[
            "workspace",
            "create",
            "--cwd",
            cwd,
            "--label",
            label,
            "--no-focus",
        ])
    }

    fn panes(&self, workspace: Option<&str>) -> Result<Vec<PaneInfo>> {
        match workspace {
            Some(ws) => self.call(&["pane", "list", "--workspace", ws], "panes"),
            None => self.call(&["pane", "list"], "panes"),
        }
    }

    fn pane(&self, pane_id: &str) -> Result<PaneInfo> {
        self.call(&["pane", "get", pane_id], "pane")
    }

    fn process_info(&self, pane_id: &str) -> Result<ProcessInfo> {
        self.call(&["pane", "process-info", "--pane", pane_id], "process_info")
    }

    fn layout(&self, pane_id: &str) -> Result<Layout> {
        self.call(&["pane", "layout", "--pane", pane_id], "layout")
    }

    fn split(
        &self,
        pane_id: &str,
        direction: SplitDirection,
        cwd: &str,
        ratio: Option<f64>,
    ) -> Result<PaneInfo> {
        let ratio_s = ratio.map(|r| r.to_string());
        let mut args: Vec<&str> = vec![
            "pane",
            "split",
            pane_id,
            "--direction",
            direction.as_str(),
            "--cwd",
            cwd,
            "--no-focus",
        ];
        if let Some(r) = ratio_s.as_deref() {
            args.push("--ratio");
            args.push(r);
        }
        self.call(&args, "pane")
    }

    fn create_tab(&self, workspace: &str, cwd: &str, label: &str) -> Result<WorkspaceCreated> {
        self.call_created(&[
            "tab",
            "create",
            "--workspace",
            workspace,
            "--cwd",
            cwd,
            "--label",
            label,
            "--no-focus",
        ])
    }

    fn close_pane(&self, pane_id: &str) -> Result<()> {
        self.call_raw(&["pane", "close", pane_id]).map(|_| ())
    }

    fn rename_pane(&self, pane_id: &str, label: &str) -> Result<()> {
        self.call_raw(&["pane", "rename", pane_id, label])
            .map(|_| ())
    }

    fn read_visible(&self, pane_id: &str, lines: u32) -> Result<String> {
        let lines = lines.to_string();
        let result = self.call_raw(&[
            "pane", "read", pane_id, "--source", "visible", "--lines", &lines, "--format", "text",
        ])?;
        let read: PaneRead = serde_json::from_value(
            result
                .get("read")
                .cloned()
                .unwrap_or_else(|| result.clone()),
        )
        .unwrap_or_default();
        Ok(read.body())
    }

    fn agents(&self) -> Result<Vec<AgentInfo>> {
        self.call(&["agent", "list"], "agents")
    }

    fn start_agent(&self, name: &str, kind: &str, pane_id: &str, args: &[String]) -> Result<()> {
        let mut argv: Vec<&str> = vec!["agent", "start", name, "--kind", kind, "--pane", pane_id];
        if !args.is_empty() {
            argv.push("--");
            argv.extend(args.iter().map(String::as_str));
        }
        self.call_raw(&argv).map(|_| ())
    }

    fn prompt_agent(&self, target: &str, text: &str) -> Result<()> {
        // No --wait: the engine is event-driven and must not block a dispatch
        // sweep on one agent's turn.
        self.call_raw(&["agent", "prompt", target, text])
            .map(|_| ())
    }

    fn send_keys(&self, target: &str, keys: &[String]) -> Result<()> {
        let mut argv: Vec<&str> = vec!["agent", "send-keys", target];
        argv.extend(keys.iter().map(String::as_str));
        self.call_raw(&argv).map(|_| ())
    }

    fn create_worktree(
        &self,
        workspace: &str,
        branch: &str,
        base: Option<&str>,
        label: Option<&str>,
    ) -> Result<WorkspaceCreated> {
        let mut argv: Vec<&str> = vec![
            "worktree",
            "create",
            "--workspace",
            workspace,
            "--branch",
            branch,
            "--no-focus",
        ];
        if let Some(b) = base {
            argv.push("--base");
            argv.push(b);
        }
        if let Some(l) = label {
            argv.push("--label");
            argv.push(l);
        }
        self.call_created(&argv)
    }

    fn notify(&self, title: &str, body: Option<&str>, sound: Sound) -> Result<()> {
        let mut argv: Vec<&str> = vec!["notification", "show", title];
        if let Some(b) = body {
            argv.push("--body");
            argv.push(b);
        }
        argv.push("--sound");
        argv.push(sound.as_str());
        self.call_raw(&argv).map(|_| ())
    }

    fn report_pane_tokens(&self, pane_id: &str, source: &str, tokens: &Tokens) -> Result<()> {
        let mut argv: Vec<String> = vec![
            "pane".into(),
            "report-metadata".into(),
            pane_id.into(),
            "--source".into(),
            source.into(),
        ];
        push_tokens(&mut argv, tokens);
        self.call_raw(&argv.iter().map(String::as_str).collect::<Vec<_>>())
            .map(|_| ())
    }

    fn report_workspace_tokens(
        &self,
        workspace_id: &str,
        source: &str,
        tokens: &Tokens,
    ) -> Result<()> {
        let mut argv: Vec<String> = vec![
            "workspace".into(),
            "report-metadata".into(),
            workspace_id.into(),
            "--source".into(),
            source.into(),
        ];
        push_tokens(&mut argv, tokens);
        self.call_raw(&argv.iter().map(String::as_str).collect::<Vec<_>>())
            .map(|_| ())
    }
}

/// An empty value means "remove this token", which is a different flag.
fn push_tokens(argv: &mut Vec<String>, tokens: &Tokens) {
    for (name, value) in tokens {
        if value.is_empty() {
            argv.push("--clear-token".into());
            argv.push(name.clone());
        } else {
            argv.push("--token".into());
            argv.push(format!("{name}={value}"));
        }
    }
}
