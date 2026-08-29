use anyhow::Result;
use clap::Parser;

use herdr_code_board::app;
use herdr_code_board::cli::Cli;

/// Rust ignores SIGPIPE so that writes to a closed pipe surface as errors, but
/// `println!` turns those into a panic. For a CLI that is wrong: `board ls | head`
/// should exit quietly, not print a backtrace. Restore the default handler.
#[cfg(unix)]
fn restore_sigpipe() {
    // SAFETY: setting a signal disposition to the default is async-signal-safe
    // and happens before any other thread exists.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

#[cfg(not(unix))]
fn restore_sigpipe() {}

fn main() -> Result<()> {
    restore_sigpipe();
    app::run(Cli::parse())
}
