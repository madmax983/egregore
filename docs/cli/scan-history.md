# `eg scan-history` — Replay Git History, Optionally Through a Commit Window

**Issues:** history replay (MVP); #256 — _scope history replay to a commit
window, not always full history_

---

## Overview

`eg scan-history <repo> --out <history.jsonl>` replays a repository's Git
history into deterministic temporal graph records (`Repository`, `Commit`,
`Change`, …). With no window flags it replays **full history** — every commit
reachable from `HEAD` — exactly as before #256, byte-for-byte.

```sh
eg scan-history . --out history.jsonl
```

Three window flags scope the replay to a subset of commits. At most one
window form may be given:

```sh
# The 50 most-recent commits reachable from HEAD.
eg scan-history . --out recent.jsonl --max-commits 50

# Commits at or after an RFC 3339 instant (inclusive).
eg scan-history . --out since.jsonl --since 2026-01-04T00:00:00Z

# A revision range: <from> excluded, <to> included.
eg scan-history . --out range.jsonl --from v1.2.0 --to v1.3.0
```

The replay stays read-only: every command is Git plumbing run with
`GIT_OPTIONAL_LOCKS=0` — no checkout, no index writes, no working-tree
mutation (see [`history-safety.md`](history-safety.md)). Window bounds are
pushed down into `git rev-list` itself, so the walk cost scales with the
window rather than total history depth.

## Window forms

| Flag | Meaning | Defaults |
|------|---------|----------|
| `--max-commits N` | The N most-recent commits reachable from `HEAD` (N ≥ 1). Output stays oldest-to-newest. | — |
| `--since <RFC3339>` | Commits whose committer time is at or after the instant (**inclusive**). The instant must be valid RFC 3339 **with an explicit offset** — a bare date is rejected. Offsets are normalized to UTC (`2026-01-04T02:00:00+02:00` ≡ `2026-01-04T00:00:00Z`). | — |
| `--from <rev> [--to <rev>]` | Git-style range: `<from>` **excluded**, `<to>` **included**. | `--to` defaults to `HEAD` |
| `[--from <rev>] --to <rev>` | With only `--to`: everything reachable from that revision. | — |

`--max-commits`, `--since`, and the `--from`/`--to` pair conflict pairwise:
giving more than one window form is an error.

## The `HistoryReplayWindow` node

Every **windowed** replay records its resolved window as a stated,
deterministic, queryable graph fact — one `HistoryReplayWindow` node,
attached to its `Repository` by a `CONTAINS` edge (citable, never an orphan,
accepted by `eg validate`). Unwindowed (full-history) replays emit **no**
window node: their output is byte-identical to pre-#256 replays, and a
windowed store can never be mistaken for full history.

```json
{"record_type":"node","kind":"HistoryReplayWindow","id":"codegraph:v10:…",
 "history_replay_window":{
   "window":"count",
   "selected_commit_count":50,
   "max_commits":50,
   "oldest_commit_sha":"<sha>",
   "newest_commit_sha":"<sha>"
 },
 "schema_version":10, …}
```

Only the fields that apply to the window form are serialized — the other
optional fields are **omitted**, not `null` (a `since` window carries
`since_instant` instead of `max_commits`; a `range` window carries
`from_rev`/`to_rev`/`from_sha`/`to_sha`).

Field notes:

* `window` is one of `"count"`, `"since"`, `"range"`.
* `since_instant` is the `--since` value normalized to UTC `Z` form.
* `from_rev` / `to_rev` are the operator-supplied revs as given (`to_rev`
  is `"HEAD"` when `--from` was given alone); `from_sha` / `to_sha` are the
  resolved commit SHAs.
* `selected_commit_count` is always ≥ 1 — an empty window is rejected before
  any record is emitted.

The payload carries no paths or PII (window kind, counts, SHAs, revs, one
UTC instant), so the node is redaction-exempt deterministic code-graph data,
classified `SourceDerived` for trust and `CodeGraph` for blind-spot analysis.

## Failure diagnostics

A conflicting, empty, or unparseable window fails with **non-zero exit**,
**exactly one** machine-readable JSON line on **stderr**, and **no partial
output** — the `--out` file is only written after the complete graph
serializes successfully.

```sh
$ eg scan-history . --out out.jsonl --max-commits 2 --since 2026-01-04T00:00:00Z
{"code":"conflicting_window","message":"only one of --max-commits, --since, or --from/--to may be given"}
$ echo $?
2
```

| `code` | When |
|--------|------|
| `conflicting_window` | More than one window form given. |
| `invalid_window` | `--max-commits` is not a positive integer, or `--since` is not valid RFC 3339. |
| `unresolvable_rev` | `--from` / `--to` does not resolve to a commit. |
| `empty_window` | The window selects no commits (e.g. a `--since` in the future, or an empty range). |

The pre-existing `git_unavailable`, `not_a_git_repository`, and
`git_history_unreadable` diagnostics are unchanged.

## Determinism

A fixed repository plus a fixed window produces **byte-identical** output
across runs: commit order, node IDs, the window payload, and the transaction
time (derived from `HEAD`'s committer date, not wall-clock) are all stable.
