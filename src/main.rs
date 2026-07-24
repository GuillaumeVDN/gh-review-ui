//! `gh-review-ui` entry point.

use std::process::ExitCode;

fn main() -> ExitCode {
    // Fail fast if gh isn't authenticated (same guard as the old CLI).
    if ghreview::gh::sh(&["gh", "auth", "status"]).is_err() {
        eprintln!("gh is not authenticated. Run `gh auth login` first.");
        return ExitCode::FAILURE;
    }
    match ghreview::app::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}
