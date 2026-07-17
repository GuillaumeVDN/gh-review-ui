"""Thin wrappers around the `gh` CLI and its GraphQL endpoint.

Everything that shells out to `gh` (or `git`) goes through here, so the rest of
the code never touches ``subprocess`` directly.
"""
import json
import subprocess


def sh(args, check=True):
    """Run a command, returning its stdout. Raise on non-zero when ``check``."""
    r = subprocess.run(args, capture_output=True, text=True)
    if check and r.returncode != 0:
        err = (r.stderr or r.stdout or "").strip().replace("\n", " | ")
        raise RuntimeError(f"{args[0]} {args[1] if len(args) > 1 else ''}: {err[:400]}")
    return r.stdout


def gh_json(args):
    """Run `gh <args>` and parse its JSON stdout."""
    return json.loads(sh(["gh", *args]))


def gh_graphql(query, **variables):
    """Run a GraphQL query/mutation via `gh api graphql` with typed variables.

    Raises if the response carries a GraphQL ``errors`` payload, so a rejected
    mutation surfaces instead of silently returning empty data.
    """
    args = ["gh", "api", "graphql", "-f", f"query={query}"]
    for k, v in variables.items():
        if v is None:
            continue
        if isinstance(v, bool):
            args += ["-F", f"{k}={'true' if v else 'false'}"]
        elif isinstance(v, int):
            args += ["-F", f"{k}={v}"]
        else:
            args += ["-f", f"{k}={v}"]
    data = json.loads(sh(args))
    if isinstance(data, dict) and data.get("errors"):
        msgs = "; ".join(e.get("message", str(e)) for e in data["errors"])
        raise RuntimeError(f"graphql: {msgs[:400]}")
    return data
