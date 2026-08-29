//! Wiring the board into herdr's own config.
//!
//! Two things live in `~/.config/herdr/config.toml`: the sidebar rows that render
//! our `$board_*` tokens, and the leader keybindings that open the board. Both
//! are the user's file, shared with other plugins — this machine already has
//! herdr-agent-quota's five sidebar rows in it — so the edits here are textual
//! and surgical rather than a TOML round-trip, which would reformat the whole
//! file and drop every comment in it.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

/// Everything this plugin writes carries this marker, so uninstall can find it
/// again and nothing else gets touched.
pub const MARKER: &str = "herdr-code-board";

/// Our row for the Agent sidebar. Tokens collapse when empty, so a pane with no
/// card on it shows nothing at all.
const AGENT_ROW: &str = r#"[{ token = "$board_card", bold = true }, { token = "$board_next" }, { token = "$board_meta", dim = true }]"#;

/// Our row for the Spaces sidebar.
const SPACE_ROW: &str = r#"[{ token = "$board_space", dim = true }]"#;

/// Herdr's defaults, used when a section does not define `rows` at all.
const AGENT_DEFAULT: &str = r#"[["state_icon", "workspace", "tab"], ["agent"]]"#;
const SPACE_DEFAULT: &str = r#"[["state_icon", "workspace"], ["branch", "git_status"]]"#;

const KEYS: &[(&str, &str, &str)] = &[
    ("prefix+b", "open", "code board"),
    ("prefix+shift+b", "quick", "queue a prompt"),
    ("prefix+alt+b", "sync", "re-import board cards"),
];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    pub agents_row: bool,
    pub spaces_row: bool,
    pub keys: bool,
    pub cli_on_path: bool,
}

impl Status {
    pub fn complete(&self) -> bool {
        self.agents_row && self.spaces_row && self.keys && self.cli_on_path
    }
}

/// Where a `herdr plugin install` leaves nothing you can type.
///
/// Herdr only needs the binary inside the plugin directory, so after installing
/// from GitHub there is no `herdr-code-board` on `PATH` and nothing says where
/// it went. Linking it is part of wiring the plugin in.
pub fn cli_link_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(Path::new(&home).join(".local/bin").join(MARKER))
}

/// The binary we are running from, resolved through symlinks.
fn our_binary() -> Option<PathBuf> {
    std::env::current_exe().ok()?.canonicalize().ok()
}

fn cli_is_linked() -> bool {
    let (Some(link), Some(me)) = (cli_link_path(), our_binary()) else {
        return false;
    };
    link.canonicalize().map(|t| t == me).unwrap_or(false)
}

/// Put the CLI on `PATH`, without clobbering someone else's file.
pub fn link_cli() -> Result<PathBuf> {
    let link = cli_link_path().context("$HOME is not set")?;
    let me = our_binary().context("cannot resolve our own path")?;

    if let Some(dir) = link.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    if link.exists() {
        let existing = link.canonicalize().unwrap_or_else(|_| link.clone());
        if existing != me && !link.is_symlink() {
            bail!(
                "{} already exists and is not ours; move it aside first",
                link.display()
            );
        }
        std::fs::remove_file(&link).ok();
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(&me, &link)
        .with_context(|| format!("linking {} -> {}", link.display(), me.display()))?;
    Ok(link)
}

/// Remove the link, but only if it still points at us.
pub fn unlink_cli() -> Option<PathBuf> {
    let link = cli_link_path()?;
    if !cli_is_linked() {
        return None;
    }
    std::fs::remove_file(&link).ok()?;
    Some(link)
}

/// Read herdr's config, treating a file that does not exist yet as empty.
/// A fresh herdr may not have written one, and that is not a reason to refuse
/// to wire the plugin in.
pub fn read_config(path: &Path) -> Result<String> {
    match std::fs::read_to_string(path) {
        Ok(body) => Ok(body),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Where herdr keeps its config.
pub fn config_path() -> Result<PathBuf> {
    if let Some(explicit) = std::env::var_os("HERDR_CONFIG_PATH") {
        return Ok(PathBuf::from(explicit));
    }
    let home = std::env::var_os("HOME").context("$HOME is not set")?;
    Ok(Path::new(&home).join(".config/herdr/config.toml"))
}

pub fn inspect(body: &str) -> Status {
    Status {
        agents_row: rows_contain(body, "ui.sidebar.agents", "$board_card"),
        spaces_row: rows_contain(body, "ui.sidebar.spaces", "$board_space"),
        keys: body.contains(&format!("\"{MARKER}.")),
        cli_on_path: cli_is_linked(),
    }
}

/// Add the sidebar rows and keybindings. Idempotent.
pub fn apply(body: &str) -> Result<String> {
    let mut out = body.to_string();
    out = ensure_row(
        &out,
        "ui.sidebar.agents",
        AGENT_DEFAULT,
        AGENT_ROW,
        "$board_card",
    )?;
    out = ensure_row(
        &out,
        "ui.sidebar.spaces",
        SPACE_DEFAULT,
        SPACE_ROW,
        "$board_space",
    )?;
    out = ensure_keys(&out);
    Ok(out)
}

/// Take back exactly what [`apply`] added.
pub fn uninstall(body: &str) -> Result<String> {
    let mut out = body.to_string();
    out = drop_rows(&out, "ui.sidebar.agents", "$board_")?;
    out = drop_rows(&out, "ui.sidebar.spaces", "$board_")?;
    out = drop_keys(&out);
    Ok(out)
}

// ---------------------------------------------------------------- rows

/// Byte range of the value of `rows = <value>` inside `[section]`, if present.
fn rows_span(body: &str, section: &str) -> Option<(usize, usize)> {
    let header = format!("[{section}]");
    let start = body.find(&header)? + header.len();
    // The section ends at the next table header at the start of a line.
    let rest = &body[start..];
    let end = rest
        .match_indices("\n[")
        .map(|(i, _)| start + i + 1)
        .next()
        .unwrap_or(body.len());

    let section_body = &body[start..end];
    let key = section_body
        .match_indices("rows")
        .find(|(i, _)| {
            let before = section_body[..*i].chars().next_back();
            let after = section_body[i + 4..].trim_start().chars().next();
            before.map(|c| c == '\n').unwrap_or(true) && after == Some('=')
        })
        .map(|(i, _)| start + i)?;

    let open = body[key..end].find('[')? + key;
    let mut depth = 0usize;
    let mut in_string = false;
    for (offset, ch) in body[open..end].char_indices() {
        match ch {
            '"' => in_string = !in_string,
            '[' if !in_string => depth += 1,
            ']' if !in_string => {
                depth -= 1;
                if depth == 0 {
                    return Some((open, open + offset + 1));
                }
            }
            _ => {}
        }
    }
    None
}

fn rows_contain(body: &str, section: &str, needle: &str) -> bool {
    match rows_span(body, section) {
        Some((a, b)) => body[a..b].contains(needle),
        None => false,
    }
}

/// Append `row` to the section's `rows` array, creating the section or the key
/// when either is missing. Everything else in the file is left byte for byte.
fn ensure_row(body: &str, section: &str, default: &str, row: &str, marker: &str) -> Result<String> {
    if rows_contain(body, section, marker) {
        return Ok(body.to_string());
    }

    if let Some((start, end)) = rows_span(body, section) {
        let current = &body[start..end];
        let inner = current
            .strip_prefix('[')
            .and_then(|s| s.strip_suffix(']'))
            .context("the rows value is not an array")?;
        let trimmed = inner.trim_end();
        let joined = if trimmed.trim().is_empty() {
            format!("[{row}]")
        } else {
            format!("[{trimmed}, {row}]")
        };
        let mut out = String::with_capacity(body.len() + joined.len());
        out.push_str(&body[..start]);
        out.push_str(&joined);
        out.push_str(&body[end..]);
        return Ok(out);
    }

    let header = format!("[{section}]");
    let assignment = format!("rows = [{}, {row}] # {MARKER}\n", strip_brackets(default));
    if let Some(at) = body.find(&header) {
        // The section exists but has no `rows`; add it right under the header.
        let insert = at + header.len();
        let insert = body[insert..]
            .find('\n')
            .map(|i| insert + i + 1)
            .unwrap_or(body.len());
        let mut out = String::with_capacity(body.len() + assignment.len());
        out.push_str(&body[..insert]);
        out.push_str(&assignment);
        out.push_str(&body[insert..]);
        return Ok(out);
    }

    let mut out = body.trim_end().to_string();
    out.push_str(&format!("\n\n{header}\n{assignment}"));
    Ok(out)
}

fn strip_brackets(value: &str) -> &str {
    value
        .trim()
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(value)
}

/// Remove every element of the section's `rows` array that mentions `marker`.
fn drop_rows(body: &str, section: &str, marker: &str) -> Result<String> {
    let Some((start, end)) = rows_span(body, section) else {
        return Ok(body.to_string());
    };
    let value = &body[start..end];
    if !value.contains(marker) {
        return Ok(body.to_string());
    }

    let kept: Vec<&str> = split_elements(value)?
        .into_iter()
        .filter(|e| !e.contains(marker))
        .collect();
    let joined = format!("[{}]", kept.join(", "));

    let mut out = String::with_capacity(body.len());
    out.push_str(&body[..start]);
    out.push_str(&joined);
    out.push_str(&body[end..]);
    Ok(out)
}

/// Split a TOML array's top-level elements, respecting nesting and strings.
fn split_elements(array: &str) -> Result<Vec<&str>> {
    let inner = array
        .trim()
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .context("not an array")?;
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut start = 0usize;
    for (i, ch) in inner.char_indices() {
        match ch {
            '"' => in_string = !in_string,
            '[' | '{' if !in_string => depth += 1,
            ']' | '}' if !in_string => depth -= 1,
            ',' if !in_string && depth == 0 => {
                let piece = inner[start..i].trim();
                if !piece.is_empty() {
                    out.push(piece);
                }
                start = i + 1;
            }
            _ => {}
        }
    }
    let piece = inner[start..].trim();
    if !piece.is_empty() {
        out.push(piece);
    }
    Ok(out)
}

// ---------------------------------------------------------------- keys

fn ensure_keys(body: &str) -> String {
    if body.contains(&format!("\"{MARKER}.")) {
        return body.to_string();
    }
    let mut out = body.trim_end().to_string();
    out.push_str(&format!("\n\n# {MARKER}\n"));
    for (key, action, description) in KEYS {
        out.push_str(&format!(
            "[[keys.command]]\nkey = \"{key}\"\ntype = \"plugin_action\"\ncommand = \"{MARKER}.{action}\"\ndescription = \"{description}\" # {MARKER}\n\n"
        ));
    }
    out.trim_end().to_string() + "\n"
}

/// Delete the `[[keys.command]]` blocks whose command belongs to us, plus our
/// own section comment. Nothing else.
fn drop_keys(body: &str) -> String {
    let ours = format!("\"{MARKER}.");
    let lines: Vec<&str> = body.lines().collect();
    let mut keep: Vec<&str> = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if line.trim() == format!("# {MARKER}") {
            i += 1;
            continue;
        }
        if line.trim() == "[[keys.command]]" {
            // Look ahead to the end of the block.
            let mut end = i + 1;
            while end < lines.len() && !lines[end].trim_start().starts_with('[') {
                end += 1;
            }
            let block = &lines[i..end];
            if block.iter().any(|l| l.contains(&ours)) {
                i = end;
                continue;
            }
            keep.extend_from_slice(block);
            i = end;
            continue;
        }
        keep.push(line);
        i += 1;
    }
    let mut out = keep.join("\n");
    while out.ends_with("\n\n\n") {
        out.pop();
    }
    out.trim_end().to_string() + "\n"
}

// ---------------------------------------------------------------- file io

/// Back the config up before touching it, and return where the copy went.
pub fn backup(path: &Path) -> Result<Option<PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let to = path.with_extension(format!("toml.bak-{stamp}"));
    std::fs::copy(path, &to).with_context(|| format!("backing up {}", path.display()))?;
    Ok(Some(to))
}

/// Rewrite the config, refusing to write anything herdr could not parse.
pub fn write(path: &Path, body: &str) -> Result<()> {
    if let Err(e) = body.parse::<toml::Table>() {
        bail!("refusing to write a config herdr could not parse: {e}");
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, body).with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trimmed copy of a real config that already has herdr-agent-quota's rows
    /// and the user's own comments in it.
    const REAL: &str = r##"# mi config
agent_panel_sort = "spaces"

[ui]
show_agent_labels_on_pane_borders = true

[ui.sidebar]

[ui.sidebar.agents]
row_gap = 0 # herdr-agent-quota
rows = [["state_icon", "tab", { token = "$quota_provider_model", bold = true }], [{ token = "$quota_topic" }]]

[[keys.command]]
key = "prefix+q"
type = "plugin_action"
command = "herdr-agent-quota.refresh"
description = "refresh all agent quotas"
"##;

    #[test]
    fn applying_adds_our_row_and_keeps_the_other_plugins() {
        let out = apply(REAL).unwrap();
        assert!(
            out.contains("$quota_provider_model"),
            "agent-quota's row survived"
        );
        assert!(out.contains("$quota_topic"));
        assert!(out.contains("$board_card"));
        assert!(
            out.contains("row_gap = 0 # herdr-agent-quota"),
            "comment survived"
        );
        assert!(out.contains("# mi config"), "the user's comment survived");
        assert!(
            out.contains("herdr-agent-quota.refresh"),
            "their keybinding survived"
        );
        out.parse::<toml::Table>().expect("still valid TOML");
    }

    #[test]
    fn the_spaces_section_is_created_when_it_does_not_exist() {
        let out = apply(REAL).unwrap();
        assert!(out.contains("[ui.sidebar.spaces]"));
        assert!(out.contains("$board_space"));
        // The created section keeps herdr's own defaults rather than replacing them.
        assert!(out.contains("git_status"));
        out.parse::<toml::Table>().unwrap();
    }

    #[test]
    fn applying_twice_changes_nothing_the_second_time() {
        let once = apply(REAL).unwrap();
        let twice = apply(&once).unwrap();
        assert_eq!(once, twice);
        assert_eq!(once.matches("$board_card").count(), 1);
        assert_eq!(once.matches("prefix+b\"").count(), 1);
    }

    #[test]
    fn uninstalling_takes_back_exactly_what_we_added() {
        let installed = apply(REAL).unwrap();
        let removed = uninstall(&installed).unwrap();

        assert!(!removed.contains("$board_"));
        assert!(!removed.contains("herdr-code-board."));
        assert!(removed.contains("$quota_provider_model"));
        assert!(removed.contains("herdr-agent-quota.refresh"));
        assert!(removed.contains("row_gap = 0 # herdr-agent-quota"));
        removed.parse::<toml::Table>().unwrap();

        // And the agents row is back to exactly its two original entries.
        let (a, b) = rows_span(&removed, "ui.sidebar.agents").unwrap();
        assert_eq!(split_elements(&removed[a..b]).unwrap().len(), 2);
    }

    #[test]
    fn inspect_reports_what_is_wired_up() {
        let before = inspect(REAL);
        assert!(!before.agents_row && !before.spaces_row && !before.keys);
        let done = inspect(&apply(REAL).unwrap());
        assert!(done.agents_row && done.spaces_row && done.keys);
    }

    #[test]
    fn a_config_that_does_not_exist_yet_reads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("herdr/config.toml");
        assert_eq!(read_config(&path).unwrap(), "");
        assert!(backup(&path).unwrap().is_none());

        // And wiring in still works, creating the directory on the way.
        let out = apply(&read_config(&path).unwrap()).unwrap();
        write(&path, &out).unwrap();
        assert!(path.exists());
        let back = inspect(&read_config(&path).unwrap());
        assert!(back.agents_row && back.spaces_row && back.keys);
    }

    #[test]
    fn a_config_with_no_ui_section_at_all_still_works() {
        let bare = "agent_panel_sort = \"spaces\"\n";
        let out = apply(bare).unwrap();
        out.parse::<toml::Table>().unwrap();
        let st = inspect(&out);
        assert!(st.agents_row && st.spaces_row && st.keys);
        assert!(!uninstall(&out).unwrap().contains("$board_"));
    }

    #[test]
    fn a_section_that_exists_without_rows_gets_them_with_herdrs_defaults() {
        let body = "[ui.sidebar.agents]\nrow_gap = 1\n";
        let out = apply(body).unwrap();
        out.parse::<toml::Table>().unwrap();
        assert!(out.contains("row_gap = 1"));
        assert!(
            out.contains("state_icon"),
            "herdr's default row is preserved"
        );
        assert!(out.contains("$board_card"));
    }

    #[test]
    fn an_empty_rows_array_is_filled_rather_than_producing_a_stray_comma() {
        let body = "[ui.sidebar.agents]\nrows = []\n";
        let out = apply(body).unwrap();
        out.parse::<toml::Table>().unwrap();
        assert!(
            out.contains(r#"rows = [[{ token = "$board_card""#),
            "got: {out}"
        );
    }

    #[test]
    fn a_bracket_inside_a_string_does_not_confuse_the_scanner() {
        let body = "[ui.sidebar.agents]\nrows = [[{ token = \"$weird]\" }]]\n";
        let out = apply(body).unwrap();
        out.parse::<toml::Table>().unwrap();
        assert!(out.contains("$weird]"));
        assert!(out.contains("$board_card"));
    }

    #[test]
    fn keybindings_use_the_leader_and_name_real_actions() {
        let out = apply(REAL).unwrap();
        for (key, action, _) in KEYS {
            assert!(out.contains(&format!("key = \"{key}\"")));
            assert!(out.contains(&format!("command = \"{MARKER}.{action}\"")));
        }
        assert!(out.contains("type = \"plugin_action\""));
    }

    #[test]
    fn writing_refuses_output_herdr_could_not_parse() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "a = 1\n").unwrap();
        assert!(write(&path, "this is not = = toml").is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "a = 1\n");
    }
}
