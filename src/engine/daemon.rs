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

/// Run the daemon until the process is killed.
///
/// Only one daemon may exist per board; a second one exits quietly rather than
/// doubling every timer.
pub fn run(paths: &Paths, config: &Config) -> Result<()> {
    let Some(_singleton) = DispatchLock::try_acquire(&paths.engine_lock())? else {
        eprintln!("herdr-code-board: a timer daemon is already running");
        return Ok(());
    };

    let herdr: Arc<dyn HerdrApi> = Arc::new(CliHerdr::new());
    loop {
        if let Err(e) = sweep_once(paths, config, herdr.clone()) {
            eprintln!("herdr-code-board: sweep failed: {e:#}");
        }

        let next = {
            let store = Store::open(&paths.database())?;
            let exec = Executor::new(store, herdr.clone(), config.clone());
            exec.next_deadline().unwrap_or(None)
        };
        std::thread::sleep(sleep_for(next, now(), config.engine_tick_seconds));
    }
}

/// Start the daemon in the background if one is not already running.
///
/// Called from the plugin's startup hook. Returns whether a daemon was spawned.
pub fn ensure_running(paths: &Paths) -> Result<bool> {
    // If we can take the singleton lock, nobody holds it, so nobody is running.
    // Release it immediately and let the child claim it for real.
    match DispatchLock::try_acquire(&paths.engine_lock())? {
        None => return Ok(false),
        Some(probe) => drop(probe),
    }

    let exe = std::env::current_exe().context("locating our own binary")?;
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
