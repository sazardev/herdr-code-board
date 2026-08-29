//! The timer daemon.
//!
//! Most of the board is event-driven: herdr runs a `herdr-code-board event` hook
//! whenever an agent changes state, and that hook does the work itself. The
//! daemon exists for the one thing hooks cannot provide — rules that fire because
//! *nothing* happened, like "if this card has been waiting for 15 minutes, poke
//! it". It also re-sweeps the ready queue so a card enqueued while every slot was
//! full still gets picked up.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};

use super::dispatch::Executor;
use super::lock::DispatchLock;
use crate::config::{Config, Paths};
use crate::herdr::client::CliHerdr;
use crate::herdr::HerdrApi;
use crate::store::{now, Store};

/// How long a hook or sweep waits for the dispatch lock before giving up.
/// A skipped sweep is harmless; the next one is seconds away.
pub const LOCK_TIMEOUT: Duration = Duration::from_secs(20);

/// Run one sweep: fire due timers, then start whatever fits.
///
/// Returns `false` when the dispatch lock was busy and the sweep was skipped.
pub fn sweep_once(paths: &Paths, config: &Config, herdr: Arc<dyn HerdrApi>) -> Result<bool> {
    let Some(_lock) = DispatchLock::acquire(&paths.dispatch_lock(), LOCK_TIMEOUT)? else {
        return Ok(false);
    };
    let store = Store::open(&paths.database())?;
    let mut exec = Executor::new(store, herdr, config.clone());
    exec.tick(now())?;
    exec.dispatch_ready()?;
    // Once per sweep, after everything settled.
    exec.present()?;
    Ok(true)
}

/// How long to sleep before the next sweep, given the nearest rule deadline.
///
/// Bounded below so a deadline in the past cannot spin, and above by the
/// configured tick so a newly enqueued card is never stranded.
pub fn sleep_for(next_deadline: Option<i64>, now: i64, tick_seconds: u64) -> Duration {
    let tick = tick_seconds.max(1);
    let secs = match next_deadline {
        Some(at) => {
            let delta = at.saturating_sub(now);
            delta.clamp(1, tick as i64) as u64
        }
        None => tick,
    };
    Duration::from_secs(secs)
}

/// Which binary the board wants running as its daemon.
///
/// Upgrading the plugin does not restart herdr, so the startup hook does not
/// run and the daemon from the *previous* build keeps the lock — running the
/// previous logic, which is exactly the code you just replaced. This key is how
/// the incumbent learns it has been superseded.
const WANTED_EXE: &str = "engine.exe";

fn our_exe() -> String {
    std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .map(|p| p.display().to_string())
        .unwrap_or_default()
}

/// Run the daemon until the process is killed, or a newer build takes over.
///
/// Only one daemon may exist per board; a second one exits quietly rather than
/// doubling every timer.
pub fn run(paths: &Paths, config: &Config) -> Result<()> {
    let Some(_singleton) = DispatchLock::try_acquire(&paths.engine_lock())? else {
        eprintln!("herdr-code-board: a timer daemon is already running");
        return Ok(());
    };
    let me = our_exe();
    Store::open(&paths.database())?.kv_set(WANTED_EXE, &me)?;

    let herdr: Arc<dyn HerdrApi> = Arc::new(CliHerdr::new());
    loop {
        if let Err(e) = sweep_once(paths, config, herdr.clone()) {
            eprintln!("herdr-code-board: sweep failed: {e:#}");
        }

        let (next, wanted) = {
            let store = Store::open(&paths.database())?;
            let wanted = store.kv_get(WANTED_EXE)?;
            let exec = Executor::new(store, herdr.clone(), config.clone());
            (exec.next_deadline().unwrap_or(None), wanted)
        };

        // A newer build asked for the job. Release the lock and hand it over,
        // rather than keeping the old logic alive until herdr restarts.
        if let Some(wanted) = wanted {
            if !wanted.is_empty() && wanted != me {
                eprintln!("herdr-code-board: handing the daemon over to {wanted}");
                drop(_singleton);
                let _ = spawn_detached(paths, std::path::Path::new(&wanted));
                return Ok(());
            }
        }

        std::thread::sleep(sleep_for(next, now(), config.engine_tick_seconds));
    }
}

fn spawn_detached(paths: &Paths, exe: &std::path::Path) -> Result<()> {
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths.engine_log())
        .with_context(|| format!("opening {}", paths.engine_log().display()))?;
    std::process::Command::new(exe)
        .arg("daemon")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(log.try_clone()?))
        .stderr(std::process::Stdio::from(log))
        .spawn()
        .context("spawning the timer daemon")?;
    Ok(())
}

/// Start the daemon in the background if one is not already running.
///
/// Called from the plugin's startup hook. Returns whether a daemon was spawned.
pub fn ensure_running(paths: &Paths) -> Result<bool> {
    let exe = std::env::current_exe().context("locating our own binary")?;
    // Declare which binary should hold the job. An older daemon reads this on
    // its next tick and hands over; if none is running, this is just a note.
    Store::open(&paths.database())?.kv_set(WANTED_EXE, &our_exe())?;

    // If we can take the singleton lock, nobody holds it, so nobody is running.
    // Release it immediately and let the child claim it for real.
    match DispatchLock::try_acquire(&paths.engine_lock())? {
        None => return Ok(false),
        Some(probe) => drop(probe),
    }

    spawn_detached(paths, &exe)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sleep_waits_exactly_until_the_next_deadline() {
        assert_eq!(sleep_for(Some(1_060), 1_000, 30), Duration::from_secs(30));
        assert_eq!(sleep_for(Some(1_010), 1_000, 30), Duration::from_secs(10));
    }

    #[test]
    fn a_deadline_in_the_past_still_sleeps_a_beat_instead_of_spinning() {
        assert_eq!(sleep_for(Some(900), 1_000, 30), Duration::from_secs(1));
        assert_eq!(sleep_for(Some(1_000), 1_000, 30), Duration::from_secs(1));
    }

    #[test]
    fn with_no_deadline_the_configured_tick_still_bounds_the_wait() {
        assert_eq!(sleep_for(None, 1_000, 30), Duration::from_secs(30));
        // A zero tick would busy-loop, so it is clamped.
        assert_eq!(sleep_for(None, 1_000, 0), Duration::from_secs(1));
    }
}

#[cfg(test)]
mod handover_tests {
    use super::*;
    use crate::config::Paths;

    fn paths(dir: &std::path::Path) -> Paths {
        Paths {
            config_dir: dir.to_path_buf(),
            state_dir: dir.to_path_buf(),
            from_herdr: true,
        }
    }

    /// Upgrading the plugin does not restart herdr, so the daemon from the
    /// previous build keeps the lock and keeps running the previous logic.
    /// `ensure_running` has to leave a note the incumbent will act on.
    #[test]
    fn asking_for_the_job_records_which_binary_should_hold_it() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(dir.path());
        Store::open(&paths.database()).unwrap();

        // Somebody else is holding the lock, pretending to be an old build.
        let _held = DispatchLock::try_acquire(&paths.engine_lock())
            .unwrap()
            .unwrap();
        let store = Store::open(&paths.database()).unwrap();
        store
            .kv_set(WANTED_EXE, "/old/build/herdr-code-board")
            .unwrap();

        let spawned = ensure_running(&paths).unwrap();
        assert!(!spawned, "it must not start a second daemon");

        let wanted = Store::open(&paths.database())
            .unwrap()
            .kv_get(WANTED_EXE)
            .unwrap()
            .unwrap();
        assert_eq!(wanted, our_exe(), "the incumbent is told to stand down");
        assert_ne!(wanted, "/old/build/herdr-code-board");
    }

    #[test]
    fn with_no_daemon_running_it_starts_one_and_claims_the_job() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(dir.path());
        Store::open(&paths.database()).unwrap();

        // Nothing holds the lock, so this really does spawn a child — which under
        // `cargo test` is the test binary itself, reading `daemon` as a filter,
        // matching nothing and exiting. Nothing is left running.
        let spawned = ensure_running(&paths);
        assert!(spawned.is_ok());
        assert_eq!(
            Store::open(&paths.database())
                .unwrap()
                .kv_get(WANTED_EXE)
                .unwrap()
                .as_deref(),
            Some(our_exe().as_str())
        );
    }
}
