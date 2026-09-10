//! Thin wrappers around the `gh` CLI / GraphQL and `git`.

use std::process::Command;

use anyhow::{anyhow, Result};
use serde_json::Value;

/// A typed GraphQL variable (mirrors gh's -f / -F distinction).
pub enum Var {
    Str(String),
    Int(i64),
    Bool(bool),
}

impl From<&str> for Var {
    fn from(s: &str) -> Self {
        Var::Str(s.to_string())
    }
}
impl From<String> for Var {
    fn from(s: String) -> Self {
        Var::Str(s)
    }
}
impl From<i64> for Var {
    fn from(n: i64) -> Self {
        Var::Int(n)
    }
}

/// Run a command, returning stdout; error on non-zero exit.
pub fn sh(args: &[&str]) -> Result<String> {
    sh_cwd("", args)
}

/// Like [`sh`], but run the command in `dir` (empty = inherit the cwd).
pub fn sh_cwd(dir: &str, args: &[&str]) -> Result<String> {
    let mut cmd = Command::new(args[0]);
    cmd.args(&args[1..]);
    if !dir.is_empty() {
        cmd.current_dir(dir);
    }
    let out = cmd.output()?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(if out.stderr.is_empty() { &out.stdout } else { &out.stderr });
        let msg: String = err.trim().replace('\n', " | ").chars().take(400).collect();
        return Err(anyhow!("{} {}: {}", args[0], args.get(1).unwrap_or(&""), msg));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Why `gh` cannot reach GitHub right now, if it cannot.
///
/// `gh auth status` reports a spent rate limit as "the token in keyring is
/// invalid", which sends the reader to `gh auth login` for something a login
/// cannot fix. The verdict comes from the API's own response instead.
pub fn auth_problem() -> Option<String> {
    match Command::new("gh").args(["auth", "status"]).output() {
        Err(e) => return Some(format!("gh not found: {e}")),
        Ok(o) if o.status.success() => return None,
        Ok(_) => {}
    }
    let out = Command::new("gh").args(["api", "-i", "user"]).output().ok()?;
    let head = String::from_utf8_lossy(&out.stdout);
    Some(api_problem(&head).unwrap_or_else(|| "gh is not authenticated. Run `gh auth login` first.".into()))
}

/// Pure half of [`auth_problem`]: the verdict for one HTTP response head.
pub fn api_problem(head: &str) -> Option<String> {
    let status = head.lines().next().and_then(|l| l.split_whitespace().nth(1)).and_then(|c| c.parse::<u16>().ok())?;
    let header = |name: &str| {
        head.lines()
            .find(|l| l.to_ascii_lowercase().starts_with(name))
            .and_then(|l| l.split_once(':'))
            .map(|(_, v)| v.trim().to_string())
    };
    if status == 403 && header("x-ratelimit-remaining").as_deref() == Some("0") {
        let limit = header("x-ratelimit-limit").unwrap_or_else(|| "?".into());
        return Some(format!(
            "GitHub rate limit spent ({limit}/h, counted per account). The gh login is fine: \
             `gh auth status` reports a spent quota as an invalid token."
        ));
    }
    if status == 401 {
        return Some("gh is not authenticated. Run `gh auth login` first.".into());
    }
    if status >= 400 {
        return Some(format!("github answered {status} to `gh api user`."));
    }
    None
}

/// Run `gh <args>` and parse its JSON stdout.
pub fn gh_json(args: &[&str]) -> Result<Value> {
    let mut full = vec!["gh"];
    full.extend_from_slice(args);
    Ok(serde_json::from_str(&sh(&full)?)?)
}

/// Run a GraphQL query/mutation via `gh api graphql`, raising on a GraphQL
/// `errors` payload so a rejected mutation surfaces instead of returning empty.
pub fn gh_graphql(query: &str, vars: &[(&str, Var)]) -> Result<Value> {
    let mut args: Vec<String> = vec!["api".into(), "graphql".into(), "-f".into(), format!("query={query}")];
    for (k, v) in vars {
        match v {
            Var::Str(s) => {
                args.push("-f".into());
                args.push(format!("{k}={s}"));
            }
            Var::Int(n) => {
                args.push("-F".into());
                args.push(format!("{k}={n}"));
            }
            Var::Bool(b) => {
                args.push("-F".into());
                args.push(format!("{k}={}", if *b { "true" } else { "false" }));
            }
        }
    }
    let mut full = vec!["gh".to_string()];
    full.extend(args);
    let refs: Vec<&str> = full.iter().map(String::as_str).collect();
    let data: Value = serde_json::from_str(&sh(&refs)?)?;
    if let Some(errs) = data.get("errors").and_then(|e| e.as_array()) {
        if !errs.is_empty() {
            let msgs: Vec<String> = errs
                .iter()
                .map(|e| e.get("message").and_then(|m| m.as_str()).unwrap_or("?").to_string())
                .collect();
            return Err(anyhow!("graphql: {}", msgs.join("; ").chars().take(400).collect::<String>()));
        }
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A spent quota answers 403 with the counters, not 401. Read as a dead
    /// token it sends the reader to `gh auth login`, which then answers that
    /// they were already logged in.
    #[test]
    fn a_spent_rate_limit_is_not_a_broken_login() {
        let head = "HTTP/2 403\r\nx-ratelimit-limit: 5000\r\nx-ratelimit-remaining: 0\r\n\r\n";
        let p = api_problem(head).expect("it has a verdict");
        assert!(p.contains("rate limit spent (5000/h"), "{p}");
        assert!(p.contains("login is fine"), "{p}");
    }

    #[test]
    fn a_dead_token_still_reads_as_a_login_problem() {
        let head = "HTTP/2 401\r\nx-ratelimit-remaining: 4999\r\n\r\n";
        assert!(api_problem(head).unwrap().contains("gh auth login"));
    }

    #[test]
    fn a_working_call_explains_nothing() {
        assert_eq!(api_problem("HTTP/2 200\r\n\r\n"), None);
        assert_eq!(api_problem(""), None);
    }
}
