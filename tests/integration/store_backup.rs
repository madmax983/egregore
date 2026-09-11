//! `eg backup --data-dir <dir> --out <file.albk>` and
//! `eg restore --from <file.albk> --data-dir <dir>` — byte-level `.albk`
//! backup and restore of an embedded store (issue #493).
//!
//! These fixtures prove the acceptance criteria from the issue evaluation
//! (`docs/cli/store-backup.md`): the backup is strictly read-only (the live
//! store is byte-for-byte untouched and no write lease is taken, so it works
//! while a writer holds the lease); backup → restore into a fresh directory
//! reproduces the `eg inspect` totals; restore is fail-closed on a non-empty
//! target and on missing / corrupt artifacts; and the `.albk` carries
//! `eg forget`-suppressed bodies byte-for-byte (so it is a disaster-recovery
//! artifact, never an export substitute) while the restored store still
//! honours the suppression on every read surface.

#![allow(missing_docs)]

use assert_cmd::Command;
use predicates::prelude::*;

#[cfg(feature = "embedded-aletheiadb")]
use std::{fs, path::Path};

#[cfg(feature = "embedded-aletheiadb")]
use aletheia_egregore::{
    GraphRecord, NodeKind, SCHEMA_VERSION,
    adapters::{EmbeddedAletheiaSink, records_from_jsonl},
    ir::{AGENT_MEMORY_SCHEMA_VERSION, PROJECT_SCHEMA_VERSION},
};

/// Sentinel body that `eg forget` must keep out of any export — but that a
/// byte-level `.albk` backup *does* carry (issue #493 Q1).
#[cfg(feature = "embedded-aletheiadb")]
const SECRET_BODY: &str = "SECRET-CUSTOMER-NAME-DO-NOT-EXPORT";

#[cfg(feature = "embedded-aletheiadb")]
fn node_with_version(
    id: &str,
    kind: NodeKind,
    schema_version: u32,
    name: &str,
    summary: &str,
) -> GraphRecord {
    let mut record = GraphRecord::node(
        id.to_owned(),
        kind,
        None,
        None,
        Some(name.to_owned()),
        summary.to_owned(),
    );
    if let GraphRecord::Node {
        schema_version: version,
        ..
    } = &mut record
    {
        *version = schema_version;
    }
    record
}

/// A small graph spanning three domains: codegraph Repository + File,
/// `agent_memory` Observation, project Task.
#[cfg(feature = "embedded-aletheiadb")]
fn mixed_domain_jsonl() -> String {
    let records = vec![
        node_with_version(
            "codegraph:v5:test-repo",
            NodeKind::Repository,
            SCHEMA_VERSION,
            "test-repo",
            "test repo",
        ),
        node_with_version(
            "codegraph:v5:test-file",
            NodeKind::File,
            SCHEMA_VERSION,
            "src/lib.rs",
            "file record",
        ),
        node_with_version(
            "agent_memory:v1:obs-1",
            NodeKind::Observation,
            AGENT_MEMORY_SCHEMA_VERSION,
            "obs1",
            "observation record",
        ),
        node_with_version(
            "project:v1:task-1",
            NodeKind::Task,
            PROJECT_SCHEMA_VERSION,
            "task1",
            "project task record",
        ),
    ];
    let mut jsonl = String::new();
    for record in &records {
        jsonl.push_str(&serde_json::to_string(record).expect("record should serialize"));
        jsonl.push('\n');
    }
    jsonl
}

/// A forgettable agent-memory Observation carrying a sensitive body, plus a
/// harmless one that must survive.
#[cfg(feature = "embedded-aletheiadb")]
fn forgettable_jsonl() -> String {
    let mut secret = node_with_version(
        "agent_memory:v1:obs-forget-me",
        NodeKind::Observation,
        AGENT_MEMORY_SCHEMA_VERSION,
        "secret",
        "Observation by agent:sess",
    );
    if let GraphRecord::Node { text, agent_id, .. } = &mut secret {
        *text = Some(SECRET_BODY.to_owned());
        *agent_id = Some("agent-1".to_owned());
    }
    let keep = node_with_version(
        "agent_memory:v1:obs-keep-me",
        NodeKind::Observation,
        AGENT_MEMORY_SCHEMA_VERSION,
        "keep",
        "Observation by agent:sess",
    );
    let mut jsonl = String::new();
    for record in [&secret, &keep] {
        jsonl.push_str(&serde_json::to_string(record).expect("record should serialize"));
        jsonl.push('\n');
    }
    jsonl
}

#[cfg(feature = "embedded-aletheiadb")]
fn ingest_embedded(graph_path: &Path, data_dir: &Path) {
    Command::cargo_bin("egregore")
        .expect("binary should run")
        .arg("ingest")
        .arg(graph_path)
        .arg("--adapter")
        .arg("embedded")
        .arg("--data-dir")
        .arg(data_dir)
        .assert()
        .success();
}

#[cfg(feature = "embedded-aletheiadb")]
fn backup_to(data_dir: &Path, out: &Path) -> serde_json::Value {
    let stdout = Command::cargo_bin("egregore")
        .expect("binary should run")
        .arg("backup")
        .arg("--data-dir")
        .arg(data_dir)
        .arg("--out")
        .arg(out)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    serde_json::from_str(String::from_utf8(stdout).expect("UTF-8").trim())
        .expect("backup output must be JSON")
}

#[cfg(feature = "embedded-aletheiadb")]
fn restore_to(from: &Path, data_dir: &Path) -> serde_json::Value {
    let stdout = Command::cargo_bin("egregore")
        .expect("binary should run")
        .arg("restore")
        .arg("--from")
        .arg(from)
        .arg("--data-dir")
        .arg(data_dir)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    serde_json::from_str(String::from_utf8(stdout).expect("UTF-8").trim())
        .expect("restore output must be JSON")
}

#[cfg(feature = "embedded-aletheiadb")]
fn inspect_json(data_dir: &Path) -> serde_json::Value {
    let out = Command::cargo_bin("egregore")
        .expect("binary should run")
        .arg("inspect")
        .arg("--data-dir")
        .arg(data_dir)
        .arg("--format")
        .arg("json")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    serde_json::from_str(String::from_utf8(out).expect("UTF-8").trim())
        .expect("inspect output must be JSON")
}

/// Sorted `(relative path, bytes)` fingerprint of every file under `root`.
#[cfg(feature = "embedded-aletheiadb")]
fn dir_fingerprint(root: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(dir: &Path, base: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        let mut entries: Vec<_> = fs::read_dir(dir).unwrap().map(|e| e.unwrap()).collect();
        entries.sort_by_key(std::fs::DirEntry::path);
        for entry in entries {
            let ft = entry.file_type().unwrap();
            let path = entry.path();
            if ft.is_dir() {
                walk(&path, base, out);
            } else if ft.is_file() {
                let rel = path
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                out.push((rel, fs::read(&path).unwrap()));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out
}

#[cfg(feature = "embedded-aletheiadb")]
#[test]
fn backup_writes_artifact_and_leaves_live_store_untouched() {
    let temp = tempfile::tempdir().expect("temp dir");
    let graph_path = temp.path().join("graph.jsonl");
    let data_dir = temp.path().join("store");
    let artifact = temp.path().join("store.albk");
    fs::write(&graph_path, mixed_domain_jsonl()).expect("fixture writes");
    ingest_embedded(&graph_path, &data_dir);

    let before = dir_fingerprint(&data_dir);
    let summary = backup_to(&data_dir, &artifact);
    let after = dir_fingerprint(&data_dir);

    // The live store is byte-for-byte untouched: the read-only guarantee.
    assert_eq!(
        before, after,
        "eg backup must not modify the live store (read-only guarantee)"
    );
    // The artifact exists, is non-empty, and the summary reports the backup.
    let bytes = fs::metadata(&artifact).expect("artifact exists").len();
    assert!(bytes > 0, "backup artifact must be non-empty");
    assert_eq!(summary["ok"], serde_json::json!(true));
    assert_eq!(summary["command"], serde_json::json!("backup"));
    assert!(
        summary["current_nodes"].as_u64().expect("count") >= 4,
        "summary should report the ingested nodes: {summary}"
    );
    assert_eq!(
        summary["bytes_written"].as_u64().expect("bytes"),
        bytes,
        "summary bytes_written must match the artifact on disk"
    );
}

#[cfg(feature = "embedded-aletheiadb")]
#[test]
fn backup_runs_while_a_writer_holds_the_write_lease() {
    let temp = tempfile::tempdir().expect("temp dir");
    let graph_path = temp.path().join("graph.jsonl");
    let data_dir = temp.path().join("store");
    let artifact = temp.path().join("store.albk");
    fs::write(&graph_path, mixed_domain_jsonl()).expect("fixture writes");
    ingest_embedded(&graph_path, &data_dir);

    // Hold the exclusive write lease in this process for the whole backup, the
    // way a live writer (embedded peer or daemon) would. `eg backup` takes no
    // lease — it reads the throwaway copy — so it must succeed regardless.
    let _lease_holder =
        EmbeddedAletheiaSink::open(&data_dir).expect("lease holder should open the store");

    let summary = backup_to(&data_dir, &artifact);

    assert_eq!(summary["ok"], serde_json::json!(true));
    assert!(
        fs::metadata(&artifact).expect("artifact exists").len() > 0,
        "backup must produce an artifact while the lease is held"
    );
}

#[cfg(feature = "embedded-aletheiadb")]
#[test]
fn backup_restore_round_trip_preserves_inspect_totals() {
    let temp = tempfile::tempdir().expect("temp dir");
    let graph_path = temp.path().join("graph.jsonl");
    let data_dir = temp.path().join("store");
    let restored_dir = temp.path().join("restored");
    let artifact = temp.path().join("store.albk");
    fs::write(&graph_path, mixed_domain_jsonl()).expect("fixture writes");
    ingest_embedded(&graph_path, &data_dir);

    let mut before = inspect_json(&data_dir);
    backup_to(&data_dir, &artifact);
    let restore_summary = restore_to(&artifact, &restored_dir);
    let mut after = inspect_json(&restored_dir);

    assert_eq!(restore_summary["ok"], serde_json::json!(true));
    assert_eq!(restore_summary["command"], serde_json::json!("restore"));
    // The restored store holds the same inventory as the original. The
    // `source` envelope names the inspected `--data-dir`, which differs by
    // construction (original vs restored path), so it is excluded from the
    // comparison — everything else must be identical.
    for report in [&mut before, &mut after] {
        report
            .as_object_mut()
            .expect("inspect output is a JSON object")
            .remove("source");
    }
    assert_eq!(
        before, after,
        "backup → restore must reproduce the inspect totals"
    );
}

#[cfg(feature = "embedded-aletheiadb")]
#[test]
fn restore_refuses_non_empty_target() {
    let temp = tempfile::tempdir().expect("temp dir");
    let graph_path = temp.path().join("graph.jsonl");
    let data_dir = temp.path().join("store");
    let artifact = temp.path().join("store.albk");
    fs::write(&graph_path, mixed_domain_jsonl()).expect("fixture writes");
    ingest_embedded(&graph_path, &data_dir);
    backup_to(&data_dir, &artifact);

    // A target holding unrelated files must be refused — the engine's own
    // TargetNotEmpty check only looks for its index manifest.
    let occupied = temp.path().join("occupied");
    fs::create_dir(&occupied).expect("target dir");
    fs::write(occupied.join("unrelated.txt"), b"not a store").expect("fixture writes");

    Command::cargo_bin("egregore")
        .expect("binary should run")
        .arg("restore")
        .arg("--from")
        .arg(&artifact)
        .arg("--data-dir")
        .arg(&occupied)
        .assert()
        .failure()
        .stderr(predicate::str::contains("non-empty"));

    // A target holding a live store must be refused too.
    Command::cargo_bin("egregore")
        .expect("binary should run")
        .arg("restore")
        .arg("--from")
        .arg(&artifact)
        .arg("--data-dir")
        .arg(&data_dir)
        .assert()
        .failure()
        .stderr(predicate::str::contains("non-empty"));

    // Nothing was written into the refused targets.
    assert!(
        !occupied.join("indexes").exists(),
        "refused restore must not materialise store files"
    );
}

#[cfg(feature = "embedded-aletheiadb")]
#[test]
fn restore_refuses_missing_and_corrupt_artifacts() {
    let temp = tempfile::tempdir().expect("temp dir");

    // Missing artifact: fail naming the path, before touching the target.
    let missing_target = temp.path().join("restored-missing");
    Command::cargo_bin("egregore")
        .expect("binary should run")
        .arg("restore")
        .arg("--from")
        .arg(temp.path().join("nope.albk"))
        .arg("--data-dir")
        .arg(&missing_target)
        .assert()
        .failure()
        .stderr(predicate::str::contains("not found"));
    assert!(
        !missing_target.exists(),
        "a missing artifact must not create the target"
    );

    // Corrupt artifact: the engine rejects it before creating the target, so
    // no residue is left behind.
    let corrupt = temp.path().join("corrupt.albk");
    fs::write(&corrupt, b"this is not a backup artifact").expect("fixture writes");
    let corrupt_target = temp.path().join("restored-corrupt");
    Command::cargo_bin("egregore")
        .expect("binary should run")
        .arg("restore")
        .arg("--from")
        .arg(&corrupt)
        .arg("--data-dir")
        .arg(&corrupt_target)
        .assert()
        .failure();
    assert!(
        !corrupt_target.exists(),
        "a corrupt artifact must not create the restore target"
    );
}

#[cfg(feature = "embedded-aletheiadb")]
#[test]
fn backup_missing_store_fails_naming_the_path_and_writes_no_file() {
    let temp = tempfile::tempdir().expect("temp dir");
    let missing = temp.path().join("no-store");
    let artifact = temp.path().join("store.albk");

    Command::cargo_bin("egregore")
        .expect("binary should run")
        .arg("backup")
        .arg("--data-dir")
        .arg(&missing)
        .arg("--out")
        .arg(&artifact)
        .assert()
        .failure()
        .stderr(predicate::str::contains(missing.to_string_lossy().as_ref()));
    assert!(
        !artifact.exists(),
        "a failed backup must not leave an artifact behind"
    );
}

#[cfg(feature = "embedded-aletheiadb")]
#[test]
fn backup_carries_suppressed_bodies_while_restore_keeps_suppression_policy() {
    let temp = tempfile::tempdir().expect("temp dir");
    let graph_path = temp.path().join("graph.jsonl");
    let data_dir = temp.path().join("store");
    let restored_dir = temp.path().join("restored");
    let artifact = temp.path().join("store.albk");
    let export_out = temp.path().join("restored-export.jsonl");
    fs::write(&graph_path, forgettable_jsonl()).expect("fixture writes");
    ingest_embedded(&graph_path, &data_dir);

    Command::cargo_bin("egregore")
        .expect("binary should run")
        .arg("forget")
        .arg("agent_memory:v1:obs-forget-me")
        .arg("--data-dir")
        .arg(&data_dir)
        .arg("--reason")
        .arg("leaked customer name")
        .assert()
        .success();

    backup_to(&data_dir, &artifact);
    restore_to(&artifact, &restored_dir);

    let sink = EmbeddedAletheiaSink::open(&restored_dir).expect("restored store opens");

    // The byte-level artifact carried the suppressed record: the physical
    // inventory of the restored store still holds its body verbatim. This is
    // correct for a backup — and exactly why an `.albk` must never be shared
    // as an export substitute.
    let physical = sink
        .inspect_all_records()
        .expect("physical inventory reads");
    assert!(
        physical.records.iter().any(|record| match record {
            GraphRecord::Node { text, .. } => text
                .as_deref()
                .is_some_and(|body| body.contains(SECRET_BODY)),
            _ => false,
        }),
        "the restored store's physical inventory must still hold the suppressed body verbatim"
    );

    // ...while every current-state read surface still suppresses it: the
    // retraction tombstone round-tripped with the data.
    let current = sink.read_all_records().expect("current view reads");
    assert!(
        current
            .iter()
            .all(|record| record.id() != "agent_memory:v1:obs-forget-me"),
        "the restored store's current view must still suppress the forgotten record"
    );

    // And the privacy-safe handoff is unchanged: an export of the restored
    // store drops the suppressed body, fail-closed.
    Command::cargo_bin("egregore")
        .expect("binary should run")
        .arg("export")
        .arg("--data-dir")
        .arg(&restored_dir)
        .arg("--out")
        .arg(&export_out)
        .assert()
        .success();
    let exported = fs::read_to_string(&export_out).expect("export readable");
    assert!(
        !exported.contains(SECRET_BODY),
        "export of the restored store must still drop the forget-suppressed body"
    );
    let records = records_from_jsonl(&exported).expect("export parses");
    assert!(
        records
            .iter()
            .any(|record| record.id() == "agent_memory:v1:obs-keep-me"),
        "non-retracted records must survive the round trip"
    );
}

#[cfg(feature = "embedded-aletheiadb")]
#[test]
fn restore_refuses_target_whose_lease_is_held() {
    let temp = tempfile::tempdir().expect("temp dir");
    let graph_path = temp.path().join("graph.jsonl");
    let data_dir = temp.path().join("store");
    let artifact = temp.path().join("store.albk");
    fs::write(&graph_path, mixed_domain_jsonl()).expect("fixture writes");
    ingest_embedded(&graph_path, &data_dir);
    backup_to(&data_dir, &artifact);

    // Hold the target's write lease in-process. The lock lives in the
    // sidecar runtime directory *next to* the target, so the target itself
    // stays empty and the only refusal the restore can hit is the lease.
    let target = temp.path().join("restored");
    fs::create_dir(&target).expect("empty target");
    let lease = aletheia_egregore::daemon::StoreLease::acquire(&target)
        .expect("test should hold the target lease");

    Command::cargo_bin("egregore")
        .expect("binary should run")
        .arg("restore")
        .arg("--from")
        .arg(&artifact)
        .arg("--data-dir")
        .arg(&target)
        .assert()
        .failure()
        .stderr(predicate::str::contains("store_contended"));

    drop(lease);
    assert!(
        !target.join("indexes").exists(),
        "a lease-refused restore must not materialise store files"
    );
}

#[cfg(not(feature = "embedded-aletheiadb"))]
#[test]
fn backup_requires_embedded_feature() {
    let temp = tempfile::tempdir().expect("temp dir");
    Command::cargo_bin("egregore")
        .expect("binary should run")
        .arg("backup")
        .arg("--data-dir")
        .arg(temp.path())
        .arg("--out")
        .arg(temp.path().join("store.albk"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("embedded-aletheiadb"));
}

#[cfg(not(feature = "embedded-aletheiadb"))]
#[test]
fn restore_requires_embedded_feature() {
    let temp = tempfile::tempdir().expect("temp dir");
    Command::cargo_bin("egregore")
        .expect("binary should run")
        .arg("restore")
        .arg("--from")
        .arg(temp.path().join("store.albk"))
        .arg("--data-dir")
        .arg(temp.path().join("restored"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("embedded-aletheiadb"));
}
