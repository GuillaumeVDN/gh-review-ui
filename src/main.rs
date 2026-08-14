//! `gh-review-ui` entry point.

use std::process::ExitCode;

const USAGE: &str = "\
gh-review-ui — review GitHub PRs in the terminal

  gh-review-ui                  review the PRs of the repo you are in
  gh-review-ui --file <path>    open a worktree file in the Pending-edits pane
  gh-review-ui --commit <sha>   review one commit of the open PR
  gh-review-ui --edits          open this checkout's PR on its local changes

Both hand the argument to an instance already running on this checkout when
there is one, so a second window is not opened for it.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mut open_file, mut open_commit) = (None, None);
    let mut open_edits = false;
    match args.first().map(String::as_str) {
        Some("--help" | "-h") => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Some(flag @ ("--file" | "--commit")) => match args.get(1) {
            Some(v) if flag == "--file" => open_file = Some(v.clone()),
            Some(v) => open_commit = Some(v.clone()),
            None => {
                eprintln!("usage: gh-review-ui {flag} <value>");
                return ExitCode::FAILURE;
            }
        },
        Some("--edits") => open_edits = true,
        Some(other) => {
            eprintln!("unknown argument {other:?}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
        None => {}
    }

    // Hand off to a running instance before doing anything expensive: it has
    // the repo loaded already, and a window per file is what this avoids.
    if open_file.is_some() || open_commit.is_some() || open_edits {
        let root = std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let sent = match (&open_file, &open_commit) {
            (Some(file), _) => ghreview::ipc::send_open(&root, file),
            (_, Some(sha)) => ghreview::ipc::send_commit(&root, sha),
            _ => ghreview::ipc::send_edits(&root),
        };
        if sent {
            return ExitCode::SUCCESS;
        }
    }

    // Fail fast if gh isn't authenticated (same guard as the old CLI).
    if ghreview::gh::sh(&["gh", "auth", "status"]).is_err() {
        eprintln!("gh is not authenticated. Run `gh auth login` first.");
        return ExitCode::FAILURE;
    }
    match ghreview::app::run(open_file, open_commit, open_edits) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}
