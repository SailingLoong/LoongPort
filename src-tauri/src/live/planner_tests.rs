use super::*;
use crate::live::patch::{json::JsonPatch, Guarded, KeyPath, LiveWriteError, WholeFile};
use serde_json::json;

#[test]
fn planned_write_preserves_exact_preimage_and_explicit_privacy() {
    for file in [
        LiveFile::private("/synthetic/private"),
        LiveFile::shared("/synthetic/shared"),
    ] {
        let before = b"original\0bytes".to_vec();
        let planned = plan_from(
            &file,
            &WholeFile::Write(b"next".to_vec()),
            Some(before.clone()),
        )
        .unwrap();
        assert_eq!(planned.file, file);
        assert_eq!(planned.pre_bytes(), Some(before.as_slice()));
        assert_eq!(planned.pre, digest(Some(&before)));
        assert_eq!(planned.planned, digest(Some(b"next")));
        assert_eq!(planned.bytes.as_deref(), Some(b"next".as_slice()));
        assert!(!planned.is_noop());
    }
}

#[test]
fn planner_distinguishes_absent_empty_and_deleted_files() {
    let file = LiveFile::private("/synthetic/client");
    let absent = plan_from(&file, &WholeFile::Delete, None).unwrap();
    assert!(absent.is_noop());
    assert_eq!(absent.pre_bytes(), None);
    let created_empty = plan_from(&file, &WholeFile::Write(vec![]), None).unwrap();
    assert!(!created_empty.is_noop());
    assert_eq!(created_empty.pre, None);
    assert_eq!(created_empty.bytes.as_deref(), Some([].as_slice()));
    let deleted_empty = plan_from(&file, &WholeFile::Delete, Some(vec![])).unwrap();
    assert!(!deleted_empty.is_noop());
    assert_eq!(deleted_empty.pre_bytes(), Some([].as_slice()));
    assert_eq!(deleted_empty.planned, None);
    assert_eq!(deleted_empty.bytes, None);
}

#[test]
fn planner_detects_byte_identical_noops() {
    let planned = plan_from(
        &LiveFile::shared("/synthetic/client"),
        &WholeFile::Write(b"unchanged".to_vec()),
        Some(b"unchanged".to_vec()),
    )
    .unwrap();
    assert!(planned.is_noop());
}

#[test]
fn json_planning_preserves_unowned_values() {
    let before = br#"{"managed":"old","unknown":{"nested":[true,1,"keep"]}}"#;
    let patch = JsonPatch {
        set: vec![(KeyPath::new(&["managed"]), json!("new"))],
        ..Default::default()
    };
    let planned = plan_from(
        &LiveFile::private("/synthetic/client.json"),
        &patch,
        Some(before.to_vec()),
    )
    .unwrap();
    assert_eq!(planned.pre_bytes(), Some(before.as_slice()));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(planned.bytes.as_ref().unwrap()).unwrap(),
        json!({"managed":"new","unknown":{"nested":[true,1,"keep"]}})
    );
}

#[test]
fn planner_rejects_malformed_input_instead_of_starting_empty() {
    let patch = JsonPatch::default();
    let error = plan_from(
        &LiveFile::private("/synthetic/client.json"),
        &patch,
        Some(b"{broken".to_vec()),
    )
    .unwrap_err();
    assert!(matches!(error, LiveWriteError::Parse { .. }));
}

#[test]
fn guarded_plan_rejects_newer_credential_generation() {
    let patch = Guarded {
        expected_pre: digest(Some(b"old-generation")),
        then: WholeFile::Write(b"requested-generation".to_vec()),
    };
    let file = LiveFile::private("/synthetic/auth.json");
    assert!(matches!(
        plan_from(&file, &patch, Some(b"newer-generation".to_vec())),
        Err(LiveWriteError::Conflict { .. })
    ));
    assert!(plan_from(&file, &patch, Some(b"old-generation".to_vec())).is_ok());
}

#[test]
fn supplied_preimage_planning_never_touches_the_filesystem() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("actual-client");
    std::fs::write(&path, b"actual-file-must-remain").unwrap();
    let planned = plan_from(
        &LiveFile::private(&path),
        &WholeFile::Delete,
        Some(b"supplied-preimage".to_vec()),
    )
    .unwrap();
    assert_eq!(planned.pre_bytes(), Some(b"supplied-preimage".as_slice()));
    assert_eq!(std::fs::read(&path).unwrap(), b"actual-file-must-remain");
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    let absent = directory.path().join("missing-parent/client");
    plan_from(
        &LiveFile::shared(&absent),
        &WholeFile::Write(b"next".to_vec()),
        None,
    )
    .unwrap();
    assert!(!absent.parent().unwrap().exists());
}
