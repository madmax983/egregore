use super::*;

/// `eg backup` — write a byte-level `.albk` backup artifact of an embedded
/// store (issue #493).
///
/// This is the byte-level counterpart to `eg export`'s logical JSONL dump.
/// The artifact captures everything the store holds — current nodes/edges,
/// full version history, the string interner, declared schema constraints,
/// and the unique-constraint registry — through `AletheiaDB::backup`'s
/// consistent point-in-time snapshot.
///
/// PRIVACY: the artifact carries `eg forget` / `eg forget-repo` suppressed
/// record bodies verbatim. It is a disaster-recovery artifact, never a
/// sharing format — never hand an `.albk` to a party that was only cleared
/// for an `eg export`. See `docs/cli/store-backup.md` for the full
/// export-vs-backup distinction.
pub(crate) fn backup_cmd(data_dir: &Path, out: &Path) -> Result<()> {
    backup_embedded_store(data_dir, out)
}

/// Backs up an embedded `--data-dir` store to a `.albk` artifact (issue #493).
///
/// Read surface. Mirrors `eg export`: validate the data dir, copy the store
/// to the throwaway read-only location `readonly_audit_store` makes, open the
/// copy unleased, and run the backup against the copy. The live store is
/// never opened and stays byte-for-byte untouched; no write lease is taken,
/// so this works while a daemon or another writer holds the lease
/// (`docs/cli/embedded-concurrency.md`, `docs/cli/store-backup.md`).
///
/// The artifact is written atomically (temp → rename) by the engine, so an
/// interrupted backup never leaves a partial file at `out`. A missing /
/// empty / non-store `--data-dir` fails with a diagnostic naming the path
/// before any write.
#[cfg(feature = "embedded-aletheiadb")]
pub(crate) fn backup_embedded_store(data_dir: &Path, out: &Path) -> Result<()> {
    // Validate BEFORE opening the engine or touching the output file: a
    // missing, empty, or unreadable data dir fails naming the path and
    // writes nothing.
    validate_existing_embedded_store(data_dir)?;

    let (store_root, _readonly_guard) = readonly_audit_store(data_dir)?;
    let sink = EmbeddedAletheiaSink::open_unleased(&store_root)
        .with_context(|| format!("failed to open embedded store {}", data_dir.display()))?;
    let summary = sink.backup_store(out).map_err(|error| {
        anyhow::anyhow!(
            "failed to back up embedded store {}: {error}",
            data_dir.display()
        )
    })?;

    println!(
        "{}",
        serde_json::json!({
            "ok": true,
            "command": "backup",
            "data_dir": data_dir.display().to_string(),
            "artifact": out.display().to_string(),
            "node_versions": summary.node_versions,
            "edge_versions": summary.edge_versions,
            "current_nodes": summary.current_node_count,
            "current_edges": summary.current_edge_count,
            "bytes_written": summary.bytes_written,
            "source_lsn": summary.source_lsn,
        })
    );
    Ok(())
}

/// Feature-off stub: embedded-store backup needs the embedded adapter.
#[cfg(not(feature = "embedded-aletheiadb"))]
pub(crate) fn backup_embedded_store(data_dir: &Path, _out: &Path) -> Result<()> {
    anyhow::bail!(
        "backing up {} requires the 'embedded-aletheiadb' feature",
        data_dir.display()
    )
}

/// `eg restore` — restore a `.albk` backup artifact into a fresh embedded
/// store (issue #493).
///
/// The target `--data-dir` must be missing or empty: restore is fail-closed
/// and never merges into or overwrites a live store (Egregore enforces this
/// before the engine's own `TargetNotEmpty` guard). Replacing a live store is
/// a deliberate operator swap — move or delete it first, then restore.
///
/// After the restore the fresh store is reopened through the adapter to prove
/// the durable layout is usable, and the record count is reported.
pub(crate) fn restore_cmd(from: &Path, data_dir: &Path) -> Result<()> {
    restore_embedded_store(from, data_dir)
}

/// Restores a `.albk` artifact into a missing-or-empty `--data-dir` (issue
/// #493).
///
/// Fail-closed target check. Upstream's `restore_to_data_dir` refuses a target
/// that already holds an index manifest (`TargetNotEmpty`), but a directory
/// of unrelated files would pass that check — so Egregore refuses any target
/// that exists and is non-empty before the engine touches it. A missing
/// target is created by the restore itself.
///
/// A live writer holding the target's write lease (issue #200) is refused
/// before any byte is written, the same as every other embedded writer.
///
/// The restored store is then opened through the ordinary adapter path
/// (write lease on the fresh directory, string-interner cap, repair gate)
/// and its record inventory counted, proving the artifact restored into a
/// working store rather than a directory of bytes.
#[cfg(feature = "embedded-aletheiadb")]
pub(crate) fn restore_embedded_store(from: &Path, data_dir: &Path) -> Result<()> {
    if !from.is_file() {
        anyhow::bail!(
            "error: backup artifact not found at {} - refusing to restore",
            from.display()
        );
    }
    match fs::read_dir(data_dir) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Ok(mut entries) => {
            if entries.next().is_some() {
                anyhow::bail!(
                    "error: refusing to restore into non-empty directory {} - \
                     restore only targets a missing or empty directory; move or delete \
                     the existing store first, then retry",
                    data_dir.display()
                );
            }
        }
        Err(error) => {
            anyhow::bail!(
                "error: cannot read restore target {}: {error}",
                data_dir.display()
            );
        }
    }

    // Fail-closed lease check BEFORE the engine writes a byte: a live writer
    // holding the lease for this target (e.g. a daemon started against the
    // still-empty directory) must refuse the restore, the same way every
    // other embedded writer is refused (issue #200).
    let lease = match crate::daemon::StoreLease::try_acquire(data_dir) {
        Ok(Some(lease)) => lease,
        Ok(None) => {
            return Err(crate::adapters::AdapterError::Contended {
                data_dir: data_dir.display().to_string(),
                message: format!(
                    "another live writer holds the exclusive embedded write lease for restore target {dir}; \
                     no write was performed. Remedy: stop it (`eg daemon stop --data-dir {dir}`) and retry",
                    dir = data_dir.display()
                ),
            }
            .into());
        }
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "failed to acquire write lease for restore target {}",
                    data_dir.display()
                )
            });
        }
    };

    EmbeddedAletheiaSink::restore_store(from, data_dir).map_err(|error| {
        anyhow::anyhow!(
            "failed to restore backup {} into {}: {error}",
            from.display(),
            data_dir.display()
        )
    })?;

    // Prove the restored directory reopens as a working store through the
    // ordinary adapter path (lease, interner cap, repair gate) and report
    // its inventory. The lease is released first: the adapter acquires the
    // same process-exclusive flock, and a second lock from this process
    // would self-contend.
    drop(lease);

    let sink = EmbeddedAletheiaSink::open(data_dir)
        .with_context(|| format!("failed to reopen restored store {}", data_dir.display()))?;
    let report = sink.inspect_all_records().map_err(|error| {
        anyhow::anyhow!(
            "restored store {} failed verification read: {error}",
            data_dir.display()
        )
    })?;
    let record_count = report.records.len() + report.unknown_schema_versions.len();

    println!(
        "{}",
        serde_json::json!({
            "ok": true,
            "command": "restore",
            "artifact": from.display().to_string(),
            "data_dir": data_dir.display().to_string(),
            "records_restored": record_count,
        })
    );
    Ok(())
}

/// Feature-off stub: embedded-store restore needs the embedded adapter.
#[cfg(not(feature = "embedded-aletheiadb"))]
pub(crate) fn restore_embedded_store(from: &Path, data_dir: &Path) -> Result<()> {
    anyhow::bail!(
        "restoring {} into {} requires the 'embedded-aletheiadb' feature",
        from.display(),
        data_dir.display()
    )
}
