//! A per-checkout socket so another tool can say "show me this file".
//!
//! dashdoc-manager opens a card's worktree and wants Enter on an uncommitted
//! file to land in the Pending-edits pane here. Launching a second instance
//! for that would leave a window per file, so a running one is reused: it
//! listens on a socket named after its checkout, and a client that finds a
//! live socket hands over the path and exits.
//!
//! The socket is named after the checkout rather than the process, so a client
//! can find it without knowing anything about us.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

/// A stable id for a checkout, matching the one the editor sockets use.
pub fn checkout_id(root: &str) -> String {
    let comps: Vec<&str> = root.trim_end_matches('/').rsplit('/').take(2).collect();
    let raw = format!(
        "{}__{}",
        comps.get(1).unwrap_or(&""),
        comps.first().unwrap_or(&"")
    );
    raw.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

pub fn socket_path(root: &str) -> PathBuf {
    PathBuf::from(format!("/tmp/ghr-ipc-{}.sock", checkout_id(root)))
}

/// The socket dashdoc-manager listens on for a checkout.
///
/// It exists exactly while a card is open on that worktree, so its presence
/// answers "is there already a Claude working here?".
pub fn manager_socket_path(root: &str) -> PathBuf {
    PathBuf::from(format!("/tmp/ddm-ipc-{}.sock", checkout_id(root)))
}

/// Whether a dashdoc-manager has a card open on `root`.
pub fn manager_listening(root: &str) -> bool {
    let path = manager_socket_path(root);
    match UnixStream::connect(&path) {
        Ok(_) => true,
        Err(_) => {
            // Left by a crashed run; clearing it stops every later send from
            // trying to talk to nothing.
            if path.exists() {
                let _ = std::fs::remove_file(&path);
            }
            false
        }
    }
}

/// Hand a prompt to the manager's chat, by the path of a file holding it.
///
/// A path rather than the text: the protocol is line-based and a review's
/// comments are many lines long.
pub fn send_prompt(root: &str, file: &str) -> bool {
    send_line_to(&manager_socket_path(root), &format!("prompt {file}"))
}

/// Ask a running instance to show the commit `sha`.
///
/// The commit pane already reviews a range of the PR's commits, so showing one
/// is a matter of telling it which.
pub fn send_commit(root: &str, shas: &[String]) -> bool {
    if shas.is_empty() {
        return false;
    }
    send_line(root, &format!("commit {}", shas.join(" ")))
}

/// Ask a running instance to show `file`.
///
/// Returns false when nobody is listening, which is the caller's cue to start
/// one. A socket left behind by a crashed instance fails to connect and is
/// removed, so it cannot wedge every later attempt.
pub fn send_open(root: &str, file: &str) -> bool {
    send_line(root, &format!("open {file}"))
}

/// Ask a running instance to show this checkout's local changes.
pub fn send_edits(root: &str) -> bool {
    send_line(root, "edits")
}

/// Ask a running instance to bring itself forward.
///
/// It raises its own window by pid, which works whatever its title is — the
/// caller cannot match on a title it did not set, and an instance started by
/// hand has one of its own.
pub fn send_focus(root: &str) -> bool {
    send_line(root, "focus")
}

/// Ask a running instance to quit, so it closes its own child windows.
pub fn send_quit(root: &str) -> bool {
    send_line(root, "quit")
}

fn send_line(root: &str, line: &str) -> bool {
    send_line_to(&socket_path(root), line)
}

fn send_line_to(path: &Path, line: &str) -> bool {
    if !path.exists() {
        return false;
    }
    match UnixStream::connect(path) {
        Ok(mut stream) => stream.write_all(format!("{line}\n").as_bytes()).is_ok(),
        Err(_) => {
            let _ = std::fs::remove_file(path);
            false
        }
    }
}

/// What a client asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Open(String),
    /// Select these commits and review them, as one range.
    Commit(Vec<String>),
    /// Bring our window forward.
    Focus,
    /// Open this checkout's PR and land on the Pending-edits pane.
    Edits,
    /// Quit as if the user had pressed `q`.
    ///
    /// Killing our window instead would skip the cleanup, leaving the Neovim
    /// and Claude windows this session opened behind with nobody to close
    /// them.
    Quit,
}

/// Parse one line of the protocol. Deliberately tiny: a verb and at most one
/// argument.
pub fn parse_request(line: &str) -> Option<Request> {
    let line = line.trim();
    if line == "quit" {
        return Some(Request::Quit);
    }
    if line == "focus" {
        return Some(Request::Focus);
    }
    if line == "edits" {
        return Some(Request::Edits);
    }
    if let Some(rest) = line.strip_prefix("commit ") {
        // One or several: the manager sends the range its pane has selected.
        let shas: Vec<String> = rest.split_whitespace().map(str::to_string).collect();
        if shas.is_empty() {
            return None;
        }
        return Some(Request::Commit(shas));
    }
    let rest = line.strip_prefix("open ")?;
    if rest.is_empty() {
        return None;
    }
    Some(Request::Open(rest.to_string()))
}

/// A listener that removes its socket when dropped.
pub struct Listener {
    path: PathBuf,
}

impl Drop for Listener {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Start listening for `root`, forwarding requests to `tx`.
///
/// Returns `None` when the socket cannot be bound — another instance already
/// owns this checkout, which is not an error worth failing to start over.
pub fn listen(root: &str, tx: Sender<Request>) -> Option<Listener> {
    let path = socket_path(root);
    // A stale socket from a crashed instance would block the bind, so it is
    // cleared once we know nobody answers on it.
    if path.exists() && UnixStream::connect(&path).is_err() {
        let _ = std::fs::remove_file(&path);
    }
    let listener = UnixListener::bind(&path).ok()?;

    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut line = String::new();
            if BufReader::new(stream).read_line(&mut line).is_ok() {
                if let Some(req) = parse_request(&line) {
                    if tx.send(req).is_err() {
                        break; // the app is gone
                    }
                }
            }
        }
    });
    Some(Listener { path })
}

/// Make a path relative to the checkout, since that is how the edit list names
/// its entries.
pub fn relative_to(root: &str, file: &str) -> String {
    let file = Path::new(file);
    match file.strip_prefix(root) {
        Ok(rel) => rel.to_string_lossy().into_owned(),
        Err(_) => file.to_string_lossy().into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_socket_is_named_after_the_checkout() {
        assert_eq!(
            checkout_id("/x/worktrees/owner__repo/pr-35"),
            "owner__repo__pr-35"
        );
        assert_eq!(
            checkout_id("/home/u/Projects/dashdoc-worktrees/feat-x"),
            "dashdoc-worktrees__feat-x"
        );
        assert!(socket_path("/a/b")
            .to_string_lossy()
            .starts_with("/tmp/ghr-ipc-"));
    }

    /// Two checkouts must not share a socket, or one would answer for the
    /// other's files.
    #[test]
    fn two_checkouts_get_two_sockets() {
        assert_ne!(socket_path("/x/feat-a"), socket_path("/x/feat-b"));
    }

    #[test]
    fn the_protocol_is_one_verb_and_one_argument() {
        assert_eq!(
            parse_request("open src/a.rs\n"),
            Some(Request::Open("src/a.rs".into()))
        );
        assert_eq!(
            parse_request("  open src/a.rs  "),
            Some(Request::Open("src/a.rs".into()))
        );
        assert_eq!(
            parse_request("open "),
            None,
            "an empty path is not a request"
        );
        assert_eq!(parse_request("commit abc1234"), Some(Request::Commit(vec!["abc1234".into()])));
        assert_eq!(
            parse_request("commit abc1234 def5678"),
            Some(Request::Commit(vec!["abc1234".into(), "def5678".into()])),
            "a range travels whole"
        );
        assert_eq!(parse_request("commit "), None);
        assert_eq!(parse_request("focus"), Some(Request::Focus));
        assert_eq!(parse_request("edits"), Some(Request::Edits));
        assert_eq!(parse_request("quit"), Some(Request::Quit));
        assert_eq!(parse_request("  quit \n"), Some(Request::Quit));
        assert_eq!(parse_request(""), None);
        assert_eq!(parse_request("shutdown"), None);
    }

    /// A path with spaces must survive, since the verb is the only delimiter.
    #[test]
    fn a_path_with_spaces_is_kept_whole() {
        assert_eq!(
            parse_request("open some dir/a file.rs"),
            Some(Request::Open("some dir/a file.rs".into()))
        );
    }

    #[test]
    fn a_path_is_made_relative_to_the_checkout() {
        assert_eq!(relative_to("/x/wt", "/x/wt/src/a.rs"), "src/a.rs");
        assert_eq!(
            relative_to("/x/wt", "src/a.rs"),
            "src/a.rs",
            "already relative"
        );
        assert_eq!(relative_to("/x/wt", "/elsewhere/a.rs"), "/elsewhere/a.rs");
    }

    #[test]
    fn sending_to_nobody_says_so_rather_than_failing() {
        assert!(!send_open("/nonexistent/checkout-xyz", "a.rs"));
        assert!(!send_quit("/nonexistent/checkout-xyz"));
    }

    /// End to end: a listener receives what a client sends.
    #[test]
    fn a_client_reaches_a_running_listener() {
        let root = format!("/tmp/ghr-ipc-test-{}", std::process::id());
        let (tx, rx) = std::sync::mpsc::channel();
        let _listener = listen(&root, tx).expect("bound");

        assert!(send_open(&root, "src/a.rs"));
        let got = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("delivered");
        assert_eq!(got, Request::Open("src/a.rs".into()));

        // Quitting travels the same way, so a card being closed can shut the
        // review down cleanly rather than killing its window.
        assert!(send_quit(&root));
        let got = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("delivered");
        assert_eq!(got, Request::Quit);
    }

    /// The manager's socket is a different file from ours: the two tools
    /// listen for different things on the same checkout.
    #[test]
    fn the_managers_socket_is_not_ours() {
        assert_ne!(socket_path("/x/wt"), manager_socket_path("/x/wt"));
        assert!(manager_socket_path("/x/wt")
            .to_string_lossy()
            .starts_with("/tmp/ddm-ipc-"));
        assert!(!manager_listening("/nonexistent/checkout-xyz"));
    }

    /// A socket left by a crashed instance must not wedge every later start.
    #[test]
    fn a_stale_socket_is_reclaimed() {
        let root = format!("/tmp/ghr-ipc-stale-{}", std::process::id());
        let path = socket_path(&root);
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, b"not a socket").unwrap();

        let (tx, _rx) = std::sync::mpsc::channel();
        assert!(listen(&root, tx).is_some(), "it took the socket over");
        let _ = std::fs::remove_file(&path);
    }

    /// Dropping the listener leaves nothing behind for the next run to clear.
    #[test]
    fn the_socket_goes_when_the_listener_does() {
        let root = format!("/tmp/ghr-ipc-drop-{}", std::process::id());
        let path = socket_path(&root);
        {
            let (tx, _rx) = std::sync::mpsc::channel();
            let _listener = listen(&root, tx).expect("bound");
            assert!(path.exists());
        }
        assert!(!path.exists());
    }
}
