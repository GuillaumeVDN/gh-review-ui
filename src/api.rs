//! GitHub domain API — PRs, files, diffs, viewed-state, reviews, worktrees.
//! Built on [`crate::gh`]; blocking, meant to run on the worker thread.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use crate::diff::parse_diff;
use crate::gh::{gh_graphql, gh_json, sh, sh_cwd, Var};
use crate::models::{Category, Commit, FileEntry, LineInfo, Pr, PendingComment};

/// Lines of context around each hunk (git's default is 3).
pub const DIFF_CONTEXT: usize = 8;

type Diff = HashMap<String, Vec<String>>;
type Info = HashMap<String, Vec<LineInfo>>;

// ---- repo / identity ----

pub fn detect_repo() -> Result<(String, String)> {
    let d = gh_json(&["repo", "view", "--json", "owner,name"])?;
    let owner = d["owner"]["login"].as_str().ok_or_else(|| anyhow!("no owner"))?;
    let name = d["name"].as_str().ok_or_else(|| anyhow!("no name"))?;
    Ok((owner.to_string(), name.to_string()))
}

pub fn get_viewer_login() -> String {
    gh_json(&["api", "user"])
        .ok()
        .and_then(|d| d["login"].as_str().map(str::to_string))
        .unwrap_or_default()
}

pub fn get_repo_root() -> String {
    sh(&["git", "rev-parse", "--show-toplevel"])
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default())
}

// ---- PRs / files / diff / commits ----

pub fn load_prs() -> Result<Vec<Pr>> {
    let searches = [
        ("is:open author:@me", Category::Mine),
        ("is:open review-requested:@me", Category::Review),
        ("is:open reviewed-by:@me", Category::Review),
    ];
    let mut seen: HashMap<i64, Pr> = HashMap::new();
    let mut order: Vec<i64> = Vec::new();
    for (search, category) in searches {
        let data = gh_json(&[
            "pr", "list", "--limit", "50", "--search", search, "--json",
            "number,title,headRefName,author,id,createdAt,updatedAt",
        ])
        .unwrap_or(Value::Array(vec![]));
        if let Some(arr) = data.as_array() {
            for p in arr {
                let number = p["number"].as_i64().unwrap_or(0);
                if seen.contains_key(&number) {
                    continue;
                }
                order.push(number);
                seen.insert(
                    number,
                    Pr {
                        number,
                        title: p["title"].as_str().unwrap_or("").to_string(),
                        head: p["headRefName"].as_str().unwrap_or("").to_string(),
                        author: p["author"]["login"].as_str().unwrap_or("?").to_string(),
                        node_id: p["id"].as_str().unwrap_or("").to_string(),
                        category,
                        created_at: p["createdAt"].as_str().unwrap_or("").to_string(),
                        updated_at: p["updatedAt"].as_str().unwrap_or("").to_string(),
                    },
                );
            }
        }
    }
    let mut prs: Vec<Pr> = order.into_iter().filter_map(|n| seen.remove(&n)).collect();
    // "review" group first (others' PRs), then "mine". Within review: oldest
    // review request first (by PR creation date, ascending). Within mine: most
    // recently updated first (descending).
    let rank = |p: &Pr| if p.category == Category::Review { 0 } else { 1 };
    prs.sort_by(|a, b| {
        rank(a).cmp(&rank(b)).then_with(|| match a.category {
            Category::Review => a.created_at.cmp(&b.created_at),
            Category::Mine => b.updated_at.cmp(&a.updated_at),
        })
    });
    Ok(prs)
}

pub fn load_files(owner: &str, name: &str, number: i64) -> Result<(String, Vec<FileEntry>)> {
    let q = r#"
    query($owner:String!, $name:String!, $number:Int!, $after:String) {
      repository(owner:$owner, name:$name) {
        pullRequest(number:$number) {
          id
          files(first:100, after:$after) {
            nodes { path viewerViewedState }
            pageInfo { hasNextPage endCursor }
          }
        }
      }
    }"#;
    let mut entries = Vec::new();
    let mut pr_id: Option<String> = None;
    let mut after: Option<String> = None;
    loop {
        let mut vars: Vec<(&str, Var)> = vec![
            ("owner", owner.into()),
            ("name", name.into()),
            ("number", number.into()),
        ];
        if let Some(a) = &after {
            vars.push(("after", a.clone().into()));
        }
        let data = gh_graphql(q, &vars)?;
        let pr = &data["data"]["repository"]["pullRequest"];
        if pr_id.is_none() {
            pr_id = Some(pr["id"].as_str().unwrap_or("").to_string());
        }
        let files = &pr["files"];
        if let Some(nodes) = files["nodes"].as_array() {
            for n in nodes {
                entries.push(FileEntry {
                    path: n["path"].as_str().unwrap_or("").to_string(),
                    viewed: n["viewerViewedState"].as_str() == Some("VIEWED"),
                });
            }
        }
        if files["pageInfo"]["hasNextPage"].as_bool() != Some(true) {
            break;
        }
        after = files["pageInfo"]["endCursor"].as_str().map(str::to_string);
    }
    Ok((pr_id.unwrap_or_default(), entries))
}

pub fn load_diff(number: i64) -> Result<(Diff, Info)> {
    let raw = sh(&["gh", "pr", "diff", &number.to_string()])?;
    Ok(parse_diff(&raw))
}

pub fn load_diff_range(first_oid: &str, last_oid: &str) -> Result<(Diff, Info)> {
    let raw = sh(&[
        "git", "diff",
        &format!("-U{DIFF_CONTEXT}"),
        "--src-prefix=a/", "--dst-prefix=b/",
        &format!("{first_oid}^..{last_oid}"),
    ])?;
    Ok(parse_diff(&raw))
}

pub fn load_commits(number: i64) -> Result<Vec<Commit>> {
    let d = gh_json(&["pr", "view", &number.to_string(), "--json", "commits"])?;
    let mut out = Vec::new();
    if let Some(arr) = d["commits"].as_array() {
        for c in arr {
            let author = c["authors"]
                .as_array()
                .and_then(|a| a.first())
                .map(|a| {
                    a["login"]
                        .as_str()
                        .or_else(|| a["name"].as_str())
                        .unwrap_or("")
                        .to_string()
                })
                .unwrap_or_default();
            out.push(Commit {
                oid: c["oid"].as_str().unwrap_or("").to_string(),
                headline: c["messageHeadline"].as_str().unwrap_or("").to_string(),
                body: c["messageBody"].as_str().unwrap_or("").to_string(),
                author,
                date: c["authoredDate"].as_str().unwrap_or("").to_string(),
            });
        }
    }
    out.reverse(); // gh returns oldest first; show newest at the top
    Ok(out)
}

pub fn load_pr_details(number: i64) -> Result<Value> {
    gh_json(&[
        "pr", "view", &number.to_string(), "--json",
        "title,body,author,state,url,createdAt,reviewDecision,comments,reviews",
    ])
}

// ---- worktrees ----

pub fn base_remote(repo_root: &str, owner: &str, name: &str) -> String {
    if let Ok(out) = sh(&["git", "-C", repo_root, "remote", "-v"]) {
        let needle = format!("{owner}/{name}");
        for line in out.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 && parts[1].contains(&needle) && line.contains("fetch") {
                return parts[0].to_string();
            }
        }
    }
    "origin".to_string()
}

pub fn worktree_path(owner: &str, name: &str, number: i64) -> PathBuf {
    let cache = std::env::var("XDG_CACHE_HOME")
        .unwrap_or_else(|_| format!("{}/.cache", std::env::var("HOME").unwrap_or_default()));
    PathBuf::from(cache)
        .join("gh-review-ui")
        .join("worktrees")
        .join(format!("{owner}__{name}"))
        .join(format!("pr-{number}"))
}

fn is_worktree(path: &str) -> bool {
    std::path::Path::new(path).is_dir()
        && sh(&["git", "-C", path, "rev-parse", "--is-inside-work-tree"]).is_ok()
}

/// Fetch the PR head and check it out in a dedicated worktree; returns its path.
pub fn open_pr_worktree(repo_root: &str, owner: &str, name: &str, number: i64) -> Result<String> {
    let remote = base_remote(repo_root, owner, name);
    let refspec = format!("+refs/pull/{number}/head:refs/gh-review-ui/pr-{number}");
    let branch = format!("gh-review-ui/pr-{number}");
    sh(&["git", "-C", repo_root, "fetch", &remote, &refspec])?;
    let sha = sh(&["git", "-C", repo_root, "rev-parse", &format!("refs/gh-review-ui/pr-{number}")])?
        .trim()
        .to_string();
    let path = worktree_path(owner, name, number);
    let path_str = path.display().to_string();
    if is_worktree(&path_str) {
        sh(&["git", "-C", &path_str, "checkout", "-B", &branch, &sha])?;
    } else {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        sh(&["git", "-C", repo_root, "worktree", "prune"]).ok();
        sh(&["git", "-C", repo_root, "worktree", "add", "--force", "-B", &branch, &path_str, &sha])?;
    }
    Ok(path_str)
}

// ---- pending edits (local worktree changes) ----

/// A unified diff of the worktree against its checked-out commit, including
/// untracked files (shown as additions) and deletions. The index is left clean.
///
/// `add -N` makes untracked files visible to `git diff` without staging content;
/// `reset -q` then drops those intent-to-add marks so a later commit stages
/// exactly the paths we ask for.
pub fn worktree_diff(wt: &str) -> String {
    let _ = sh(&["git", "-C", wt, "add", "-N", "."]);
    // Force standard a/ b/ prefixes; a user's diff.mnemonicPrefix / diff.noprefix
    // would otherwise emit i/ w/ (or none), which the parser can't key on.
    let raw = sh(&["git", "-C", wt, "diff", "--src-prefix=a/", "--dst-prefix=b/"]).unwrap_or_default();
    let _ = sh(&["git", "-C", wt, "reset", "-q"]);
    raw
}

/// The changed files in `wt` (sorted, with kinds), their per-file diff, and the
/// per-file line info (used to overlay local edits onto the PR diff).
pub fn load_edits(wt: &str) -> (Vec<crate::models::EditEntry>, Diff, Info) {
    let raw = worktree_diff(wt);
    let edits = crate::diff::classify_edits(&raw);
    let (diff, info) = parse_diff(&raw);
    (edits, diff, info)
}

/// Revert a single file's local change: delete an untracked new file, else
/// restore the tracked file (or a deletion) from the checked-out commit.
pub fn discard_edit(wt: &str, path: &str, added: bool) -> Result<()> {
    if added {
        std::fs::remove_file(std::path::Path::new(wt).join(path)).ok();
        sh(&["git", "-C", wt, "reset", "-q", "--", path]).ok();
        Ok(())
    } else {
        sh(&["git", "-C", wt, "checkout", "--", path]).map(|_| ())
    }
}

/// Stage exactly `paths`, commit with `message`, then push (non-force) to the
/// PR head `branch` on `remote`. Returns `false` (no-op) when `paths` is empty.
pub fn commit_edit_files(
    wt: &str,
    remote: &str,
    branch: &str,
    message: &str,
    paths: &[String],
) -> Result<bool> {
    if paths.is_empty() {
        return Ok(false);
    }
    if branch.is_empty() {
        return Err(anyhow!("unknown PR head branch — cannot push"));
    }
    // Refuse to push unless the PR head branch already exists on this remote, so
    // a non-force push updates the PR and never creates a stray branch (e.g. for
    // a fork PR whose head lives on another remote). Checked before committing.
    let remote_ref = sh(&["git", "-C", wt, "ls-remote", "--heads", remote, branch]).unwrap_or_default();
    if remote_ref.trim().is_empty() {
        return Err(anyhow!(
            "branch '{branch}' not found on '{remote}' (fork PR?) — nothing committed or pushed"
        ));
    }
    let mut add_args: Vec<&str> = vec!["git", "-C", wt, "add", "-A", "--"];
    for p in paths {
        add_args.push(p.as_str());
    }
    sh(&add_args)?;
    // Skip pre-commit / commit-msg / pre-push hooks — review fixups shouldn't be
    // blocked by the project's local hooks.
    sh(&["git", "-C", wt, "commit", "--no-verify", "-m", message])?;
    let refspec = format!("HEAD:refs/heads/{branch}");
    sh(&["git", "-C", wt, "push", "--no-verify", remote, &refspec])?;
    Ok(true)
}

/// Check out PR `number` into the local dev checkout `dir` (e.g. ~/Projects/<repo>),
/// but only if its working tree is clean. Uses `gh pr checkout` so the branch is
/// created/updated to the PR head.
pub fn checkout_pr_local(dir: &str, owner: &str, name: &str, number: i64) -> Result<String> {
    if !std::path::Path::new(dir).is_dir() {
        return Err(anyhow!("{dir} does not exist"));
    }
    sh_cwd(dir, &["git", "rev-parse", "--is-inside-work-tree"])
        .map_err(|_| anyhow!("{dir} is not a git repository"))?;
    let status = sh_cwd(dir, &["git", "status", "--porcelain"])?;
    if !status.trim().is_empty() {
        return Err(anyhow!("{dir}: working tree not clean — commit or stash first"));
    }
    let repo = format!("{owner}/{name}");
    sh_cwd(dir, &["gh", "-R", &repo, "pr", "checkout", &number.to_string()])?;
    Ok(format!("Checked out #{number} in {dir}"))
}

// ---- viewed state ----

pub fn mark_viewed_api(pr_id: &str, path: &str, viewed: bool) -> Result<()> {
    let m = if viewed { "markFileAsViewed" } else { "unmarkFileAsViewed" };
    let q = format!(
        "mutation($pr:ID!, $path:String!) {{ {m}(input:{{pullRequestId:$pr, path:$path}}) \
         {{ pullRequest {{ id }} }} }}"
    );
    gh_graphql(&q, &[("pr", pr_id.into()), ("path", path.into())])?;
    Ok(())
}

/// Mark/unmark many files with one aliased GraphQL request per chunk; a failed
/// batch falls back to per-file. Returns `(done, errors)`.
pub fn mark_viewed_bulk_api(pr_id: &str, paths: &[String], viewed: bool) -> (Vec<String>, Vec<String>) {
    let mutation = if viewed { "markFileAsViewed" } else { "unmarkFileAsViewed" };
    let mut done = Vec::new();
    let mut errs = Vec::new();
    for batch in paths.chunks(100) {
        let mut decls = vec!["$pr:ID!".to_string()];
        let mut body = String::new();
        let keys: Vec<String> = (0..batch.len()).map(|j| format!("p{j}")).collect();
        for (j, _) in batch.iter().enumerate() {
            decls.push(format!("$p{j}:String!"));
            body.push_str(&format!(
                "  f{j}: {mutation}(input:{{pullRequestId:$pr, path:$p{j}}}) {{ clientMutationId }}\n"
            ));
        }
        let q = format!("mutation({}) {{\n{body}}}", decls.join(", "));
        let mut var_refs: Vec<(&str, Var)> = vec![("pr", pr_id.into())];
        for (j, p) in batch.iter().enumerate() {
            var_refs.push((keys[j].as_str(), p.clone().into()));
        }
        match gh_graphql(&q, &var_refs) {
            Ok(_) => done.extend(batch.iter().cloned()),
            Err(_) => {
                for p in batch {
                    match mark_viewed_api(pr_id, p, viewed) {
                        Ok(()) => done.push(p.clone()),
                        Err(e) => errs.push(format!("{p}: {e}")),
                    }
                }
            }
        }
    }
    (done, errs)
}

// ---- pending review ----

pub fn find_pending_review(owner: &str, name: &str, number: i64, login: &str) -> Result<(String, Option<String>)> {
    let q = r#"
    query($owner:String!, $name:String!, $number:Int!, $login:String!) {
      repository(owner:$owner, name:$name) {
        pullRequest(number:$number) {
          id
          reviews(first:1, author:$login, states:[PENDING]) { nodes { id } }
        }
      }
    }"#;
    let d = gh_graphql(
        q,
        &[("owner", owner.into()), ("name", name.into()), ("number", number.into()), ("login", login.into())],
    )?;
    let pr = &d["data"]["repository"]["pullRequest"];
    let pr_id = pr["id"].as_str().unwrap_or("").to_string();
    let rid = pr["reviews"]["nodes"].as_array().and_then(|n| n.first()).and_then(|n| n["id"].as_str()).map(str::to_string);
    Ok((pr_id, rid))
}

fn ensure_pending_review(owner: &str, name: &str, number: i64, login: &str, pr_id: &str) -> Result<String> {
    if let (_, Some(rid)) = find_pending_review(owner, name, number, login)? {
        return Ok(rid);
    }
    let q = "mutation($pr:ID!) { addPullRequestReview(input:{pullRequestId:$pr}) { pullRequestReview { id } } }";
    let d = gh_graphql(q, &[("pr", pr_id.into())])?;
    Ok(d["data"]["addPullRequestReview"]["pullRequestReview"]["id"].as_str().unwrap_or("").to_string())
}

pub fn load_pending_comments(owner: &str, name: &str, number: i64, login: &str) -> Result<Vec<PendingComment>> {
    let q = r#"
    query($owner:String!, $name:String!, $number:Int!, $login:String!) {
      repository(owner:$owner, name:$name) {
        pullRequest(number:$number) {
          reviews(first:10, author:$login, states:[PENDING]) {
            nodes { comments(first:100) { nodes { id path body line originalLine startLine originalStartLine } } }
          }
        }
      }
    }"#;
    let d = gh_graphql(
        q,
        &[("owner", owner.into()), ("name", name.into()), ("number", number.into()), ("login", login.into())],
    )?;
    let mut out = Vec::new();
    if let Some(reviews) = d["data"]["repository"]["pullRequest"]["reviews"]["nodes"].as_array() {
        for r in reviews {
            if let Some(comments) = r["comments"]["nodes"].as_array() {
                for c in comments {
                    let line = c["line"].as_i64().or_else(|| c["originalLine"].as_i64()).unwrap_or(0);
                    let start_line = c["startLine"].as_i64().or_else(|| c["originalStartLine"].as_i64());
                    // Only a real multi-line range (start differs from end).
                    let start_line = start_line.filter(|&s| s != line);
                    out.push(PendingComment {
                        path: c["path"].as_str().unwrap_or("").to_string(),
                        body: c["body"].as_str().unwrap_or("").to_string(),
                        line,
                        side: "RIGHT".to_string(),
                        comment_id: c["id"].as_str().unwrap_or("").to_string(),
                        start_side: if start_line.is_some() { "RIGHT".to_string() } else { String::new() },
                        start_line,
                    });
                }
            }
        }
    }
    Ok(out)
}

pub fn add_pending_comment_api(owner: &str, name: &str, number: i64, login: &str, pr_id: &str, c: &PendingComment) -> Result<()> {
    let review_id = ensure_pending_review(owner, name, number, login, pr_id)?;
    let mut decls = "$r:ID!, $path:String!, $line:Int!, $body:String!, $side:DiffSide!".to_string();
    let mut fields = "pullRequestReviewId:$r, path:$path, line:$line, body:$body, side:$side".to_string();
    let mut vars: Vec<(&str, Var)> = vec![
        ("r", review_id.clone().into()),
        ("path", c.path.clone().into()),
        ("line", c.line.into()),
        ("body", c.body.clone().into()),
        ("side", c.side.clone().into()),
    ];
    if let (Some(sl), false) = (c.start_line, c.start_side.is_empty()) {
        decls.push_str(", $startLine:Int!, $startSide:DiffSide!");
        fields.push_str(", startLine:$startLine, startSide:$startSide");
        vars.push(("startLine", sl.into()));
        vars.push(("startSide", c.start_side.clone().into()));
    }
    let q = format!("mutation({decls}) {{ addPullRequestReviewThread(input:{{{fields}}}) {{ thread {{ id }} }} }}");
    gh_graphql(&q, &vars)?;
    Ok(())
}

pub fn delete_pending_comment_api(comment_id: &str) -> Result<()> {
    let q = "mutation($id:ID!) { deletePullRequestReviewComment(input:{id:$id}) { clientMutationId } }";
    gh_graphql(q, &[("id", comment_id.into())])?;
    Ok(())
}

pub fn update_pending_comment_api(comment_id: &str, body: &str) -> Result<()> {
    let q = "mutation($id:ID!, $body:String!) { updatePullRequestReviewComment(input:{pullRequestReviewCommentId:$id, body:$body}) { pullRequestReviewComment { id } } }";
    gh_graphql(q, &[("id", comment_id.into()), ("body", body.into())])?;
    Ok(())
}

pub fn submit_review_api(owner: &str, name: &str, number: i64, login: &str, pr_id: &str, event: &str, body: &str) -> Result<()> {
    let review_id = ensure_pending_review(owner, name, number, login, pr_id)?;
    let q = "mutation($r:ID!, $event:PullRequestReviewEvent!, $body:String) { submitPullRequestReview(input:{pullRequestReviewId:$r, event:$event, body:$body}) { pullRequestReview { id } } }";
    let mut vars: Vec<(&str, Var)> = vec![("r", review_id.into()), ("event", event.into())];
    if !body.is_empty() {
        vars.push(("body", body.into()));
    }
    gh_graphql(q, &vars)?;
    Ok(())
}

// ---- session persistence ----

fn session_file() -> PathBuf {
    let cache = std::env::var("XDG_CACHE_HOME")
        .unwrap_or_else(|_| format!("{}/.cache", std::env::var("HOME").unwrap_or_default()));
    PathBuf::from(cache).join("gh-review-ui").join("last-pr.json")
}

pub fn save_last_pr(owner: &str, name: &str, number: i64) {
    let path = session_file();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let mut data: Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({}));
    if let Some(map) = data.as_object_mut() {
        map.insert(format!("{owner}/{name}"), json!(number));
    }
    std::fs::write(&path, data.to_string()).ok();
}

pub fn load_last_pr(owner: &str, name: &str) -> Option<i64> {
    let data: Value = serde_json::from_str(&std::fs::read_to_string(session_file()).ok()?).ok()?;
    data.get(format!("{owner}/{name}")).and_then(|v| v.as_i64())
}
