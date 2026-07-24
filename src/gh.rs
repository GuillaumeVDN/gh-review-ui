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
    let out = Command::new(args[0]).args(&args[1..]).output()?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(if out.stderr.is_empty() { &out.stdout } else { &out.stderr });
        let msg: String = err.trim().replace('\n', " | ").chars().take(400).collect();
        return Err(anyhow!("{} {}: {}", args[0], args.get(1).unwrap_or(&""), msg));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
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
