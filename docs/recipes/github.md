# Recipe: GitHub (CI status, pull requests, re-runs)

"Did the build pass?", "what pull requests are open?", "run the failed jobs again". Two routes; use one.

| Route | Files | Needs |
|---|---|---|
| **REST through HTTP tools** (default) | [`github-rest.json`](../../examples/recipes/github-rest.json) | A token in `GITHUB_TOKEN` |
| **The official MCP server** | [`github-mcp.json`](../../examples/recipes/github-mcp.json) | The same token; the server's remote URL |

A `gh`-backed route is **not** offered: the core never starts a process (REQ-EXT-001), so `gh` would need your own small webhook in front of it, which is more to maintain than the REST calls above.

## Token

Create a **fine-grained personal access token** limited to the repositories you name, with the least rights the tools need: *Actions: read* and *Pull requests: read* for the read tools, and *Actions: write* only if you enable `github_rerun_failed`. Put it in `GITHUB_TOKEN`. The tiers below hold a call for your yes, but the token is the real limit on what a bug or a hijacked model could do.

## REST tools

| Tool | Call | Tier |
|---|---|---|
| `github_list_runs` | `GET /repos/{owner}/{repo}/actions/runs?per_page=5` | `read_only` |
| `github_list_prs` | `GET /repos/{owner}/{repo}/pulls?state=open&per_page=5` | `read_only` |
| `github_rerun_failed` | `POST /repos/{owner}/{repo}/actions/runs/{run_id}/rerun-failed-jobs` | `costly`: it spends build minutes and shows on the repository. Read back with the run number; runs after the yes. |

Run names, titles and branch names are outside text, and the descriptions say so. `owner` and `repo` are percent-encoded into the path, so a value such as `x/../../orgs` stays inside one path segment (tested). The files set `X-GitHub-Api-Version: 2022-11-28`; the core adds `User-Agent: clawdot`, which GitHub requires, and `Authorization: Bearer`. The `Accept` header is left to the core's default; GitHub's documentation recommends `application/vnd.github+json` but the default `application/json` was not tried, so if GitHub answers `415` or an odd format add a header to the file.

Opening a pull request is available through the MCP route (`create_pull_request`, `costly`). It is not in the REST file because its body (title, head, base, body text) is a larger surface for a mistake.

## MCP route

The official server is at `https://api.githubcopilot.com/mcp/`, authenticated with the same token as a bearer (its README, read 2026-11-09). The example offers five tools by the server's own names and sets their tiers itself: `list_pull_requests`, `get_pull_request`, `actions_list` (its `list_workflow_runs` method), `list_issues` are `read_only`; `create_pull_request` is `costly`. Tool names were taken from the README and **not** from a live `tools/list`; run the portal's Test on the connection to see the real names, and correct the file if they differ. `actions_list` takes a `method` argument, so one tool covers several read operations; keep it `read_only` only because the token you gave it is read-only for Actions.

The README also documents `--read-only` and `--toolsets` for the server you run yourself. Whether the remote endpoint speaks Streamable HTTP at the revision the core pins (`2025-06-18`) was **not verified**; the README's configuration uses `"type": "http"`.

## Failure modes

| Symptom | Cause |
|---|---|
| `401` | Token wrong, expired or revoked. |
| `403` / `404` on a private repository | The token cannot see it. A fine-grained token must list the repository. |
| `403` with a rate-limit message | The unauthenticated or authenticated limit was reached; GitHub says when it resets. |
| `403` on `rerun-failed-jobs` | The token lacks *Actions: write*. Good: the read tools still work. |
| `422` | The run is not finished or has nothing failed to re-run. |
| MCP connection skipped at start-up | The server negotiated another revision, or the URL or token is wrong. The warning names both revisions. |

## Verify

* `scripts/cargo.sh test -p clawdot-core --test recipes` checks each request line, the version header and the path encoding.
* Replay eval `rcp-ci-rerun-needs-a-yes`: a re-run is read back with its run number and runs only after a yes.
* `python3 examples/recipes/fake_server.py`; or the real API with `--dry-run`, where the two reads run and the re-run is only described.

## Sources (read 2026-11-09)

* The official server: <https://github.com/github/github-mcp-server>

**Not verified:** GitHub REST pages were not read for this recipe (the paths and the version header are written from the author's knowledge of the API), nor the remote server's transport and revision, nor any live call.
