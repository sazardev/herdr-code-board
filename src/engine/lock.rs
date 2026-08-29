//! A cross-process mutex around dispatch.
//!
//! Herdr fires an event hook per pane state change, so several one-shot
//! `herdr-code-board event` processes can run at the same moment, alongside the
//! timer daemon. They all want to start agents. An advisory file lock makes the
//! dispatch section single-threaded across processes, so two of them can never
//! claim the same pane or start the same card twice.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

pub struct DispatchLock {
    file: File,
}

impl DispatchLock {
    /// Block until the lock is free, or give up after `timeout`.
    pub fn acquire(path: &Path, timeout: Duration) -> Result<Option<Self>> {
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)
            .with_context(|| format!("opening lock file {}", path.display()))?;

        let deadline = Instant::now() + timeout;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Some(Self { file })),
                Err(TryLockError::WouldBlock) => {
                    if Instant::now() >= deadline {
                        return Ok(None);
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(TryLockError::Error(e)) => return Err(e).context("locking the dispatch lock"),
            }
        }
    }

    /// Take the lock only if it is free right now. Used by the daemon to decide
    /// whether another instance already owns the board.
    pub fn try_acquire(path: &Path) -> Result<Option<Self>> {
        Self::acquire(path, Duration::ZERO)
    }
}

impl Drop for DispatchLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_holder_is_refused_while_the_first_lives() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dispatch.lock");

        let first = DispatchLock::try_acquire(&path).unwrap();
        assert!(first.is_some());
        assert!(
            DispatchLock::try_acquire(&path).unwrap().is_none(),
            "the lock must not be handed out twice"
        );

        drop(first);
        assert!(DispatchLock::try_acquire(&path).unwrap().is_some());
    }

    #[test]
    fn waiting_gives_up_instead_of_hanging_forever() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dispatch.lock");
        let _held = DispatchLock::try_acquire(&path).unwrap().unwrap();

        let start = Instant::now();
        let got = DispatchLock::acquire(&path, Duration::from_millis(200)).unwrap();
        assert!(got.is_none());
        assert!(start.elapsed() >= Duration::from_millis(150));
    }
}
