//! Untrusted archive member text must not escape through staging diagnostics.
use super::{captured_logs, init_logger, tree};
use crate::services::webdav_sync::archive::stage_skills_zip;
use std::{
    fs,
    io::{Cursor, Write},
};
use zip::write::SimpleFileOptions;

const CANARY: &str = "SECRET_CANARY_UNTRUSTED_ZIP_MEMBER";
fn archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, bytes) in entries {
        writer.start_file(*name, options).unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}
fn private_rejection(bytes: &[u8]) {
    init_logger();
    let live = tempfile::tempdir().unwrap();
    fs::create_dir(live.path().join("installed-skill")).unwrap();
    fs::write(
        live.path().join("installed-skill/SKILL.md"),
        b"existing live skill remains unchanged",
    )
    .unwrap();
    let before = tree(live.path());
    let original_input = bytes.to_vec();
    let start = captured_logs().len();
    log::info!(target: "remote_transport_test", "archive_stage_privacy_begin");
    let error = match stage_skills_zip(bytes) {
        Err(error) => error,
        Ok(_) => panic!("invalid archive unexpectedly staged"),
    };
    assert_eq!(tree(live.path()), before);
    assert_eq!(bytes, original_input);
    let material = format!(
        "{error} {error:?} {}",
        serde_json::to_string(&error).unwrap()
    );
    let captured = captured_logs();
    let logs = &captured[start..];
    assert!(
        logs.contains("archive_stage_privacy_begin"),
        "capture must be active"
    );
    assert!(
        !logs.contains(CANARY),
        "archive member leaked into application logs"
    );
    assert!(
        !material.contains(CANARY),
        "archive member leaked into returned staging error"
    );
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn member_file_directory_conflict_error_is_private() {
    let child = format!("{CANARY}/child.txt");
    let bytes = archive(&[
        (CANARY, b"first is a regular file"),
        (&child, b"untrusted child"),
    ]);
    private_rejection(&bytes);
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn corrupted_member_stream_error_is_private() {
    let mut bytes = archive(&[(CANARY, b"synthetic CRC payload")]);
    let offset = {
        let mut parsed = zip::ZipArchive::new(Cursor::new(bytes.clone())).unwrap();
        let offset = parsed.by_index(0).unwrap().data_start() as usize;
        offset
    };
    bytes[offset] ^= 1; // Valid ZIP directory/size, deliberately wrong member checksum.
    private_rejection(&bytes);
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn invalid_archive_structure_is_rejected_without_echoing_input() {
    private_rejection(CANARY.as_bytes());
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn safe_members_and_original_traversal_skip_rule_are_preserved() {
    let live = tempfile::tempdir().unwrap();
    let live_file = live.path().join("existing-skill.txt");
    fs::write(&live_file, b"live original").unwrap();
    let before = tree(live.path());
    let absolute = live_file.to_str().unwrap();
    let bytes = archive(&[
        ("safe/SKILL.md", b"safe staged skill"),
        (absolute, b"must not overwrite live"),
        ("../SECRET_CANARY_UNTRUSTED_ZIP_MEMBER", b"must not escape"),
    ]);
    let prepared = stage_skills_zip(&bytes).unwrap();
    assert_eq!(
        fs::read(prepared.source.join("safe/SKILL.md")).unwrap(),
        b"safe staged skill"
    );
    let files = tree(&prepared.source);
    assert_eq!(files.len(), 1);
    assert_eq!(tree(live.path()), before);
    assert!(!prepared.source.parent().unwrap().join(CANARY).exists());
}
#[cfg(feature = "test-hooks")]
pub(super) fn run_case(case: &str) {
    match case {
        "archive_member_conflict" => member_file_directory_conflict_error_is_private(),
        "archive_corrupt_stream" => corrupted_member_stream_error_is_private(),
        "archive_invalid_structure" => {
            invalid_archive_structure_is_rejected_without_echoing_input()
        }
        "archive_boundary_control" => safe_members_and_original_traversal_skip_rule_are_preserved(),
        _ => panic!("unknown archive stage privacy case"),
    }
}
