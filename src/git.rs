//! Finding repositories and reading their branches.
//!
//! The board should not make you type paths. It walks your home directory once,
//! finds every checkout, and puts them in a picker with their current branch —
//! and when you are already standing in a repo, it just uses that one.
//!
//! Everything here avoids `git` subprocesses on the hot path: discovery reads
//! `.git/HEAD` directly, which is one small file per repo. Only the branch list
//! for a single chosen repo shells out.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Directory names never worth descending into. Package caches and build output
/// hold thousands of directories and occasionally a vendored checkout, and none
/// of it is a repo you would queue work against.
const SKIP: &[&str] = &[
    "node_modules",
    "target",
    "vendor",
    "dist",
    "build",
    "out",
    "venv",
    "__pycache__",
    "Pods",
    "DerivedData",
    "Library",
    "Games",
    "snap",
    "go",
];

/// A checkout found on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FoundRepo {
    pub path: PathBuf,
    pub name: String,
    pub branch: Option<String>,
    /// Modification time of `.git`, in unix seconds. Recently touched repos are
    /// the ones you are working in, so the picker leads with them.
    pub touched_at: i64,
}

/// A place to look, and how deep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanRoot {
    pub path: PathBuf,
    pub depth: usize,
}

/// Where to look when the user has not said.
///
/// The home directory covers `~/Documents/thing`, `~/src/org/thing` and the like
/// in one pass; `~/.config` is listed separately because dot-directories are
/// skipped during the walk but that is where editor and dotfile repos live.
pub fn default_roots(home: &Path, depth: usize) -> Vec<ScanRoot> {
    let mut roots = vec![ScanRoot {
        path: home.to_path_buf(),
        depth,
    }];
    let config = home.join(".config");
    if config.is_dir() {
        roots.push(ScanRoot {
            path: config,
            depth: 2,
        });
    }
    roots
}

fn is_repo(dir: &Path) -> bool {
    dir.join(".git").exists()
}

/// Branch name from `.git/HEAD`, without invoking git.
///
/// Returns `None` for a detached HEAD, which has no branch to name.
pub fn head_branch(repo: &Path) -> Option<String> {
    let git = repo.join(".git");
    // A linked worktree has a `.git` *file* pointing at the real gitdir.
    let head = if git.is_dir() {
        git.join("HEAD")
    } else {
        let body = std::fs::read_to_string(&git).ok()?;
        let dir = body.strip_prefix("gitdir:")?.trim();
        PathBuf::from(dir).join("HEAD")
    };
    let body = std::fs::read_to_string(head).ok()?;
    let name = body.trim().strip_prefix("ref: refs/heads/")?;
    Some(name.to_string())
}

fn touched_at(repo: &Path) -> i64 {
    std::fs::metadata(repo.join(".git"))
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn record(dir: &Path) -> FoundRepo {
    FoundRepo {
        name: dir
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| dir.display().to_string()),
        branch: head_branch(dir),
        touched_at: touched_at(dir),
        path: dir.to_path_buf(),
    }
}

/// Walk `roots` and return every checkout found, most recently touched first.
///
/// A repository is never descended into, so a submodule or a nested worktree
/// does not turn one project into twenty entries.
pub fn discover(roots: &[ScanRoot]) -> Vec<FoundRepo> {
    let mut found = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();

    for root in roots {
        let mut frontier = vec![(root.path.clone(), 0usize)];
        while let Some((dir, depth)) = frontier.pop() {
            let canonical = dir.canonicalize().unwrap_or_else(|_| dir.clone());
            if !seen.insert(canonical) {
                continue;
            }
            if is_repo(&dir) {
                found.push(record(&dir));
                continue;
            }
            if depth >= root.depth {
                continue;
            }
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                // Do not follow symlinks: they are how a walk ends up in a loop
                // or halfway across the filesystem.
                if !kind.is_dir() {
                    continue;
                }
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with('.') || SKIP.contains(&name.as_ref()) {
                    continue;
                }
                frontier.push((entry.path(), depth + 1));
            }
        }
    }

    found.sort_by(|a, b| b.touched_at.cmp(&a.touched_at).then(a.name.cmp(&b.name)));
    found
}

/// Local branch names, current one first.
///
/// This is the only place that shells out to git, and only for one repo the user
/// has already chosen.
pub fn branches(repo: &Path) -> Result<Vec<String>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "branch",
            "--format=%(refname:short)",
            "--sort=-committerdate",
        ])
        .output()?;
    if !out.status.success() {
        return Ok(head_branch(repo).into_iter().collect());
    }
    let mut names: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();

    if let Some(current) = head_branch(repo) {
        names.retain(|b| b != &current);
        names.insert(0, current);
    }
    Ok(names)
}

/// Walk up from `start` to the checkout that contains it.
pub fn root_of(start: &Path) -> Option<PathBuf> {
    let mut cur = start.canonicalize().ok()?;
    loop {
        if is_repo(&cur) {
            return Some(cur);
        }
        cur = cur.parent()?.to_path_buf();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_repo(at: &Path, branch: &str) {
        std::fs::create_dir_all(at.join(".git")).unwrap();
        std::fs::write(at.join(".git/HEAD"), format!("ref: refs/heads/{branch}\n")).unwrap();
    }

    #[test]
    fn discovery_finds_nested_checkouts_within_the_depth_limit() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        make_repo(&home.join("Documents/alpha"), "main");
        make_repo(&home.join("src/org/beta"), "develop");
        std::fs::create_dir_all(home.join("src/org/deep/deeper/gamma/.git")).unwrap();

        let found = discover(&[ScanRoot {
            path: home.to_path_buf(),
            depth: 3,
        }]);
        let names: HashSet<&str> = found.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains("alpha"));
        assert!(names.contains("beta"));
        assert!(
            !names.contains("gamma"),
            "a repo past the depth limit must not be reported"
        );
    }

    #[test]
    fn the_branch_comes_from_head_without_running_git() {
        let dir = tempfile::tempdir().unwrap();
        make_repo(dir.path(), "feature/x");
        assert_eq!(head_branch(dir.path()).as_deref(), Some("feature/x"));

        // A detached HEAD names a commit, not a branch.
        std::fs::write(dir.path().join(".git/HEAD"), "9f1c2b3\n").unwrap();
        assert_eq!(head_branch(dir.path()), None);
    }

    #[test]
    fn a_linked_worktree_resolves_through_its_gitdir_file() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real/.git/worktrees/wt");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("HEAD"), "ref: refs/heads/board/x\n").unwrap();

        let wt = dir.path().join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(wt.join(".git"), format!("gitdir: {}\n", real.display())).unwrap();

        assert_eq!(head_branch(&wt).as_deref(), Some("board/x"));
    }

    #[test]
    fn a_repo_is_never_descended_into() {
        let dir = tempfile::tempdir().unwrap();
        make_repo(&dir.path().join("outer"), "main");
        make_repo(&dir.path().join("outer/nested"), "main");

        let found = discover(&[ScanRoot {
            path: dir.path().to_path_buf(),
            depth: 5,
        }]);
        assert_eq!(found.len(), 1, "a submodule must not become its own entry");
        assert_eq!(found[0].name, "outer");
    }

    #[test]
    fn heavy_and_hidden_directories_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        make_repo(&dir.path().join("node_modules/pkg"), "main");
        make_repo(&dir.path().join("target/thing"), "main");
        make_repo(&dir.path().join(".cache/thing"), "main");
        make_repo(&dir.path().join("real"), "main");

        let found = discover(&[ScanRoot {
            path: dir.path().to_path_buf(),
            depth: 4,
        }]);
        assert_eq!(
            found.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            vec!["real"]
        );
    }

    #[test]
    fn results_lead_with_the_most_recently_touched_repo() {
        let dir = tempfile::tempdir().unwrap();
        make_repo(&dir.path().join("old"), "main");
        make_repo(&dir.path().join("new"), "main");
        // Age the first one by a day.
        let old = dir.path().join("old/.git");
        let then = std::time::SystemTime::now() - std::time::Duration::from_secs(86_400);
        filetime_set(&old, then);

        let found = discover(&[ScanRoot {
            path: dir.path().to_path_buf(),
            depth: 2,
        }]);
        assert_eq!(found[0].name, "new");
        assert_eq!(found[1].name, "old");
    }

    /// Set an mtime without pulling in a crate for it.
    fn filetime_set(path: &Path, when: std::time::SystemTime) {
        let file = std::fs::File::open(path).unwrap();
        file.set_times(
            std::fs::FileTimes::new()
                .set_accessed(when)
                .set_modified(when),
        )
        .unwrap();
    }

    #[test]
    fn root_of_walks_up_and_gives_up_outside_a_checkout() {
        let dir = tempfile::tempdir().unwrap();
        make_repo(dir.path(), "main");
        let deep = dir.path().join("a/b/c");
        std::fs::create_dir_all(&deep).unwrap();
        assert_eq!(root_of(&deep).unwrap(), dir.path().canonicalize().unwrap());

        let bare = tempfile::tempdir().unwrap();
        assert_eq!(root_of(bare.path()), None);
    }

    #[test]
    fn default_roots_cover_home_and_the_dotfiles_people_keep_in_config() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".config")).unwrap();
        let roots = default_roots(dir.path(), 4);
        assert_eq!(roots[0].depth, 4);
        assert_eq!(roots[1].path, dir.path().join(".config"));
    }
}
