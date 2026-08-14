//! `gh-review-ui` entry point.

use std::process::ExitCode;

const USAGE: &str = "\
gh-review-ui — review GitHub PRs in the terminal

  gh-review-ui                  review the PRs of the repo you are in
  gh-review-ui --file <path>    open a worktree file in the Pending-edits pane

`--file` hands the path to an instance already running on this checkout when
there is one, so a second window is not opened for it.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let open_file = match args.first().map(String::as_str) {
        Some("--help" | "-h") => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Some("--file") => match args.get(1) {
            Some(f) => Some(f.clone()),
            None => {
                eprintln!("usage: gh-review-ui --file <path>");
                return ExitCode::FAILURE;
            }
        },
        Some(other) => {
            eprintln!("unknown argument {other:?}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
        None => None,
    };

    // Hand off to a running instance before doing anything expensive: it has
    // the repo loaded already, and a window per file is what this avoids.
    if let Some(file) = &open_file {
        let root = std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        if ghreview::ipc::send_open(&root, file) {
            return ExitCode::SUCCESS;
        }
    }

    // Fail fast if gh isn't authenticated (same guard as the old CLI).
    if ghreview::gh::sh(&["gh", "auth", "status"]).is_err() {
        eprintln!("gh is not authenticated. Run `gh auth login` first.");
        return ExitCode::FAILURE;
    }
    match ghreview::app::run(open_file) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}
