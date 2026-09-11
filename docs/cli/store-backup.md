# Store backup and restore (issue #493)

**Status:** evaluation complete and `eg backup` / `eg restore` ship as of
2026-09-10. The evaluation below is the Q4 answer the issue required; the
[Q1 safety answers](#q1-safety-answers-from-the-published-aletheiadb-020-crate)
gate the commands, and the [export-vs-backup distinction](#export-vs-backup--the-distinction-stated-loudly)
is the documentation the issue required before either shipped — it is repeated
in the commands' own help text, not just here.

`AletheiaDB` 0.2.0 introduced the `.albk` backup artifact: `backup()` writes
format v7 (the reader accepts v1–v7), folding in the unique-constraint registry,
schema constraints, and the crypto-shred keyring / subject-designation registry
(which is empty for Egregore — the `audit-export` feature that populates it is
not enabled, and the schema-constraint / namespace / property-index subsystems
are not enabled either; see `docs/cli/store-upgrade.md`). 0.1.x had no
`backup()` at all, which is why the 0.2.0 upgrade itself has no backup/restore
off-ramp. Going forward, we can.

## Commands

```sh
eg backup --data-dir .egregore --out egregore-2026-09-10.albk
eg restore --from egregore-2026-09-10.albk --data-dir .egregore-restored
```

`eg backup` is strictly read-only: it copies the store to a throwaway
temporary directory (the same `readonly_audit_store` path `eg export` and
`eg inspect --data-dir` use), opens the copy without the write lease, and runs
the engine's consistent point-in-time snapshot against it. The live store stays
byte-for-byte untouched and the command works while a daemon or another writer
holds the lease. The artifact is written atomically (temp → rename). On
success it prints a JSON summary (`node_versions`, `edge_versions`,
`current_nodes`, `current_edges`, `bytes_written`, `source_lsn`).

`eg restore` is fail-closed: the target `--data-dir` must be missing or empty.
Egregore refuses a non-empty target before the engine's own `TargetNotEmpty`
guard runs (the engine only checks for its index manifest, so a directory of
unrelated files would otherwise pass). The target's write lease is also
acquired before the engine writes a byte, so a live writer — e.g. a daemon
still serving the directory being swapped out — refuses the restore with the
same `store_contended` error every other embedded writer raises (issue #200).
There is no merge and no in-place overwrite. Replacing a live store is a
deliberate operator swap:

```sh
eg backup --data-dir .egregore --out egregore.albk
eg daemon stop --data-dir .egregore         # release the write lease first
mv .egregore .egregore-prev                 # operator swap, deliberate
eg restore --from egregore.albk --data-dir .egregore
```

After materialising the durable layout, restore reopens the fresh store through
the ordinary adapter path (write lease, string-interner cap, repair gate) and
counts its record inventory, proving the artifact restored into a working
store. On success it prints a JSON summary (`records_restored`).

What is **not** covered: the `.albk` holds the AletheiaDB data dir only.
`eg protected` raw-artifact blobs live in a separate directory
(`<store>/blobs/` + `manifest.jsonl`, structurally outside `--data-dir`) and
are **not** in the artifact — they are an adjacent backup concern with their
own authorization model. The crypto-shred keyring sidecar is empty for
Egregore (the `audit-export` feature is not enabled).

## Q4: what in a store cannot be regenerated from Git + re-import

The set is **non-empty**. The store is not a pure cache — the graph is derived
from Git, but the store also holds content whose only durable copy is the store
itself:

| Content (node kinds / records) | Source | Why Git + re-import cannot regenerate it |
|---|---|---|
| Agent memory: `Agent`, `AgentSession`, `AgentRun`, `AgentTurn`, `ToolCall`, `CommandRun`, `FileEdit`, `PatchArtifact`, `Failure`, `Observation`, `Decision` | Transcript imports (`import-claude-code`, `import-codex`, `import-antigravity`, `import-traj`) | Source transcripts are local and ephemeral (e.g. `~/.claude/projects` session files); they are not in Git. |
| Project state: `Task`, `AcceptanceCriterion`, `GitHubIssue`, `PR`, `Review`, `ReviewStateTransition`, `ExternalIdentity`, `ExternalLink`, `LocalTask`, `Plan`, `Product`, `Project` | `eg import github` (GitHub REST API), `eg import-local-tasks` | GitHub is a separate source of truth: re-import needs network + token and the state may have changed since. Local task files under `.egregore/tasks` are not guaranteed to be Git-tracked. |
| User-context policy: `Preference`, `WorkflowRule`, `NamingDecision`, `Constraint`, plus append-only `PromotionPrompt` / `PromotionDecision` audit records | Operator approval flow (authorization-derived) | These are operator decisions, not derived facts. There is no other copy. |
| Verification & runtime observations: `Verification`, `TestRun`, `CommandEvidence`, `CoverageReport`, `BenchmarkRun`, `ProofResult`, scan-logs records (`LogOccurrenceBucket`, `ErrorSignature`, …) | Test runs, CI output, `eg scan-logs` | Ephemeral by nature: logs rotate, runs are not re-runnable from Git alone. |
| Evidence-link edges: `MENTIONS_SYMBOL`, `TOUCHED_FILE`, `FAILED_ON`, `TRANSITIONS_REVIEW`, `EXTERNAL_HANDLE`, `REFERENCES_TASK` | `eg link-evidence` over memory + code graph | Derived from agent memory; lost when memory is lost. |
| **Privacy state: `Retraction` event nodes + retraction / eviction tombstones** (`eg forget`, `eg forget-repo`) | Operator privacy actions | Re-importing the original JSONL would **resurrect** the suppressed records — suppression is not in Git. Losing this state is a privacy incident, not a cache miss. |
| Bi-temporal coordinates | Ingest transaction time | Re-import rewrites transaction time to "now"; history is no longer byte-identical. |

Honest limits on this scope:

- **Protected raw artifacts are not in the store.** `eg protected` keeps blobs
  in `<store>/blobs/` plus `manifest.jsonl` in a directory that is
  structurally separate from the AletheiaDB `--data-dir`
  (`docs/cli/protected-artifacts.md`). An `.albk` of the data dir does **not**
  include them; they are an adjacent backup concern with their own
  authorization model, not covered by this evaluation.
- Daemon / runtime sidecar state (`egregored.lock`, runtime metadata) is
  ephemeral and regenerable; it is not backup-worthy.

## Q1: safety answers (from the published `aletheiadb` 0.2.0 crate)

All answers below are verified against the published 0.2.0 API
(`AletheiaDB::backup`, `AletheiaDB::restore`, `AletheiaDB::restore_to_data_dir`
in `src/db/backup.rs` of the 0.2.0 sources).

1. **Does `.albk` round-trip the privacy suppressions? No — and that is the
   important one.** `backup()` captures *all* current nodes/edges, *all*
   version history (including cold-tier versions), and the string interner —
   it is byte-level. Egregore's `forget` / `forget-repo` suppression is
   logical (enforced at the export and serving layers, `docs/cli/export.md`);
   the crypto-shred `audit-export` feature that would physically erase bytes
   is not enabled. So a `.albk` **carries the suppressed bodies**. The
   `Retraction` / tombstone records round-trip too, so after a restore the
   suppression *policy* is intact — but the artifact file itself is not safe
   to hand to someone who was only cleared to receive an export. This is
   *correct* for a backup and *dangerous* for sharing; it must be stated in
   any command's help text, not just here.
2. **Write lease: not required for backup.** `backup(&self)` takes a
   point-in-time Arc-COW snapshot at the current WAL LSN under the commit
   clock; it is explicitly designed for concurrent writers (no writer can
   appear partially in the artifact). At the Egregore level a backup command
   would be a read-only command, and read-only commands never take the write
   lease (`docs/cli/embedded-concurrency.md`).
3. **Live store: yes, via the throwaway-copy read path.** The command would
   follow the `eg export` / `eg inspect --data-dir` pattern
   (`readonly_audit_store`): copy the store to a temporary directory, open the
   copy unleased, `backup()` the copy. The original stays byte-for-byte
   untouched and the command works while a live writer holds the lease. The
   backup is a consistent snapshot of the copy (recovered through normal WAL
   replay on open, exactly like crash recovery).
4. **Throwaway-copy interaction: none.** Backup composes with the existing
   read-only path unchanged; no new locking or copy semantics are needed.
5. **Restore is fail-closed.** `restore_to_data_dir` refuses a non-empty
   target (`TargetNotEmpty`) — it restores only into an empty directory and
   then reopens it through the canonical durable config. There is no merge and
   no in-place overwrite: replacing a live store requires the operator to
   move or delete it first, deliberately. (`restore()` into an ephemeral
   in-memory DB exists but is not the operator path.)
6. **Feature matrix:** the backup/restore API is not feature-gated (only the
   keyring sidecar is `audit-export`-gated), so it is available under
   `--no-default-features`, `nova`, and `--all-features` alike.

## Export vs backup — the distinction, stated loudly

| | `eg export --data-dir --out <file.jsonl>` | `.albk` |
|---|---|---|
| Fidelity | Canonical-JSONL **logical**: records reconstructed through the ingest/export shape | **Byte-level** store artifact: nodes, edges, full version history, interner |
| Readable / diffable | Yes | No (opaque binary) |
| Privacy suppression honoured | **Yes** — drops every physical version of a suppressed id and edges stranded on one, fail-closed (`docs/cli/export.md`) | **No** — carries suppressed bodies byte-for-byte; safe for disaster recovery, **not** safe to share as an export substitute |
| Portable across store versions | Yes (re-ingest anywhere) | Reader window v1–v7; writes v7 |
| Transaction time | Rewritten to "now" on re-ingest | Preserved exactly |
| Covers protected-store blobs | No (separate directory by design) | No (same — outside the AletheiaDB data dir) |

**Rule of thumb:** `eg export` is for sharing, auditing, and diffing — it is
the privacy-safe handoff. `.albk` is for disaster recovery of the full store —
byte-exact, including everything, including what `eg forget` hid. Never confuse
the two, and never hand an `.albk` to a party that was only cleared for an
export.

## Gating contract — satisfied by `eg backup` / `eg restore`

The issue gated any user-facing command on the contract below. Each item is
now met; the tests live in `tests/integration/store_backup.rs`.

1. **Read-only backup path:** throwaway copy + unleased open, mirroring
   `eg export`'s read-only guarantee; the original store is byte-for-byte
   unchanged (a `dir_fingerprint` before/after test, as export has). No write
   lease taken. The command also runs while a writer holds the lease.
2. **Restore targets a missing or empty directory only**, per the upstream
   `TargetNotEmpty` fail-closed contract — strengthened in Egregore, which
   refuses any non-empty target (the engine only checks its own index
   manifest). The operator performs the swap; the command documents the swap
   step explicitly. Never in-place, never a merge.
3. **The privacy warning lives in the command's help text**, not just in
   docs: `eg backup --help` states that an `.albk` contains
   `forget`-suppressed bodies and must not be shared as an export substitute.
4. **Atomic artifact write** (upstream temp → rename; kept).
5. **Tests for every behavior**, per repo rules: round-trip through a
   throwaway store (backup → restore to empty dir → `eg inspect` totals
   match), backup while a writer holds the lease, restore-into-non-empty
   refusal, restore of a corrupt artifact, and suppressed-record presence in
   the restored store's physical inventory vs absence in the export.
