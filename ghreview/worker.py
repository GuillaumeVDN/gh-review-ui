"""Background worker: runs blocking `gh` jobs off the UI thread.

Jobs and results are plain tuples passed over queues. ``worker_loop`` translates
a job tuple into an API call and posts a result tuple; :mod:`ghreview.controller`
owns the meaning of both.
"""
from . import api


def worker_loop(jobs, results):
    while True:
        job = jobs.get()
        if job is None:
            return
        kind = job[0]
        try:
            if kind == "load_prs":
                results.put(("prs", api.load_prs()))
            elif kind == "load_active":
                _, owner, name, login, number = job
                if number is None:
                    results.put(("active", None, None, [], {}, {}, [], []))
                else:
                    pr_id, files = api.load_files(owner, name, number)
                    try:
                        commits = api.load_commits(number)
                    except Exception:
                        commits = []
                    # Prefer the local git range so we control context lines;
                    # fall back to `gh pr diff` (3-line context) when the commit
                    # list is unavailable. The worktree fetch already brought the
                    # PR objects into the repo, so the range resolves locally.
                    if commits:
                        diff, info = api.load_diff_range(commits[-1].oid, commits[0].oid)
                    else:
                        diff, info = api.load_diff(number)
                    try:
                        pending = api.load_pending_comments(owner, name, number, login) if login else []
                    except Exception:
                        pending = []
                    results.put(("active", number, pr_id, files, diff, info, pending, commits))
            elif kind == "load_commit_diff":
                _, first_oid, last_oid = job
                diff, info = api.load_diff_range(first_oid, last_oid)
                results.put(("commit_diff", diff, info))
            elif kind == "open_pr":
                _, repo_root, owner, name, number = job
                path = api.open_pr_worktree(repo_root, owner, name, number)
                results.put(("pr_opened", number, path))
            elif kind == "mark_viewed":
                _, pr_id, path, viewed = job
                api.mark_viewed_api(pr_id, path, viewed)
                results.put(("viewed_ok", [path], viewed))
            elif kind == "mark_viewed_bulk":
                _, pr_id, paths, viewed = job
                done, errs = api.mark_viewed_bulk_api(pr_id, paths, viewed)
                results.put(("viewed_bulk", done, viewed, errs))
            elif kind == "load_pr_details":
                _, number = job
                results.put(("pr_details", number, api.load_pr_details(number)))
            elif kind == "add_pending":
                _, owner, name, number, login, pr_id, comment = job
                api.add_pending_comment_api(owner, name, number, login, pr_id, comment)
                pending = api.load_pending_comments(owner, name, number, login)
                results.put(("pending_list", pending, "Comment added to pending review"))
            elif kind == "discard_pending":
                _, owner, name, number, login, comment_id = job
                if comment_id:
                    api.delete_pending_comment_api(comment_id)
                pending = api.load_pending_comments(owner, name, number, login)
                results.put(("pending_list", pending, "Discarded pending comment"))
            elif kind == "submit_review":
                _, owner, name, number, login, pr_id, event, body = job
                api.submit_review_api(owner, name, number, login, pr_id, event, body)
                results.put(("review_submitted", event))
        except Exception as e:
            results.put(("error", kind, str(e)))
