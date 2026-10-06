//! Approved R3 field and byte-preservation contracts; synthetic in-memory inputs only.
use super::engine::{digest, sha256_hex};
use super::patch::dotenv::DotenvPatch;
use super::patch::json::JsonPatch;
use super::patch::toml::TomlPatch;
use super::patch::{Guarded, KeyPath, LivePatch, LiveWriteError, WholeFile};
use serde_json::json;
use std::path::Path;

#[test]
fn json_model_change_preserves_unowned_nested_values_and_key_order() {
    let before = br#"{"model":"old","custom":{"x":[1,"value",null]},"mcpServers":{"own":{"args":["x"]}},"hooks":[]}"#;
    let patch = JsonPatch {
        set: vec![(KeyPath::new(&["model"]), json!("new"))],
        ..Default::default()
    };
    let bytes = patch
        .apply(Path::new("settings.json"), Some(before))
        .unwrap();
    let after: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let original: serde_json::Value = serde_json::from_slice(before).unwrap();
    assert_eq!(after["model"], "new");
    for key in ["custom", "mcpServers", "hooks"] {
        assert_eq!(after[key], original[key]);
    }
    assert_eq!(
        after.as_object().unwrap().keys().collect::<Vec<_>>(),
        original.as_object().unwrap().keys().collect::<Vec<_>>()
    );
}

#[test]
fn toml_model_change_keeps_untouched_comments_and_mcp_bytes() {
    let before = b"# user header\nmodel = 'old' # own model\n\n[mcp_servers.custom]\ncommand = '/bin/tool'  # keep spaces\nargs = [ 'a', 'b' ]\n";
    let patch = TomlPatch {
        set: vec![(KeyPath::new(&["model"]), toml_edit::value("new"))],
        ..Default::default()
    };
    let bytes = patch.apply(Path::new("config.toml"), Some(before)).unwrap();
    let after = std::str::from_utf8(&bytes).unwrap();
    assert!(after.starts_with("# user header\n"));
    assert!(after.ends_with(
        "\n[mcp_servers.custom]\ncommand = '/bin/tool'  # keep spaces\nargs = [ 'a', 'b' ]\n"
    ));
    assert!(after.contains("# own model"));
}

#[test]
fn dotenv_change_preserves_each_untouched_line_ending_and_trailing_bytes() {
    let before = b"# CRLF comment\r\nGEMINI_MODEL=old\nUSER_FLAG='keep'\r\n# final without newline";
    let patch = DotenvPatch {
        set: vec![("GEMINI_MODEL".into(), "new".into())],
        ..Default::default()
    };
    let after = patch.apply(Path::new(".env"), Some(before)).unwrap();
    assert_eq!(
        after,
        b"# CRLF comment\r\nGEMINI_MODEL=new\nUSER_FLAG='keep'\r\n# final without newline"
    );
}

#[test]
fn dotenv_noop_preserves_mixed_newlines_and_bare_carriage_return() {
    for before in [
        b"A=1\r\n# comment\nB=2\r".as_slice(),
        b"\r\n\n\r".as_slice(),
    ] {
        assert_eq!(
            DotenvPatch::default()
                .apply(Path::new(".env"), Some(before))
                .unwrap(),
            before
        );
    }
}

#[test]
fn malformed_and_invalid_utf8_documents_do_not_become_empty_configuration() {
    let path = Path::new("synthetic");
    for (patch, bytes) in [
        (
            &JsonPatch::default() as &dyn LivePatch,
            b"{broken".as_slice(),
        ),
        (&TomlPatch::default(), b"model = [".as_slice()),
        (&DotenvPatch::default(), b"\xff".as_slice()),
    ] {
        assert!(matches!(
            patch.apply(path, Some(bytes)),
            Err(LiveWriteError::Parse { .. })
        ));
    }
}

#[test]
fn guarded_write_and_delete_reject_stale_bytes_including_missing_vs_empty() {
    let path = Path::new("synthetic");
    for then in [WholeFile::Write(b"next".to_vec()), WholeFile::Delete] {
        let patch = Guarded {
            expected_pre: digest(Some(b"old")),
            then: then.clone(),
        };
        assert!(matches!(
            patch.apply_file(path, Some(b"newer")),
            Err(LiveWriteError::Conflict { .. })
        ));
        assert_eq!(
            patch.apply_file(path, Some(b"old")).unwrap(),
            then.apply_file(path, Some(b"old")).unwrap()
        );
        let absent = Guarded {
            expected_pre: None,
            then,
        };
        assert!(matches!(
            absent.apply_file(path, Some(b"")),
            Err(LiveWriteError::Conflict { .. })
        ));
    }
}

#[test]
fn content_revision_hashes_are_exact_and_missing_is_not_empty() {
    assert_eq!(digest(None), None);
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        digest(Some(b"")),
        Some("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".into())
    );
}

#[test]
fn toml_noop_preserves_mixed_line_endings_in_untouched_content() {
    let before = b"# CRLF comment\r\nmodel = 'old'\n\r\n[mcp_servers.custom]\r\ncommand = 'tool'\n";
    assert_eq!(
        TomlPatch::default()
            .apply(Path::new("config.toml"), Some(before))
            .unwrap(),
        before
    );
}

#[test]
fn toml_change_preserves_mixed_line_endings_outside_the_changed_value() {
    let before = b"# CRLF comment\r\nmodel = 'old'\n\r\n[mcp_servers.custom]\r\ncommand = 'tool'\n";
    let patch = TomlPatch {
        set: vec![(KeyPath::new(&["model"]), toml_edit::value("new"))],
        ..Default::default()
    };
    assert_eq!(
        patch.apply(Path::new("config.toml"), Some(before)).unwrap(),
        b"# CRLF comment\r\nmodel = \"new\"\n\r\n[mcp_servers.custom]\r\ncommand = 'tool'\n"
    );
}

#[test]
fn dotenv_removal_and_append_preserve_surviving_line_bytes() {
    let path = Path::new(".env");
    let patch = DotenvPatch {
        remove: vec!["DELETE".into()],
        ..Default::default()
    };
    assert_eq!(
        patch
            .apply(path, Some(b"# keep\r\nKEEP=1\nDELETE=last"))
            .unwrap(),
        b"# keep\r\nKEEP=1\n"
    );
    let patch = DotenvPatch {
        set: vec![("NEW".into(), "value".into())],
        ..Default::default()
    };
    assert_eq!(
        patch
            .apply(path, Some(b"# keep\r\nKEEP=1\n# tail"))
            .unwrap(),
        b"# keep\r\nKEEP=1\n# tail\r\nNEW=value"
    );
}

#[test]
fn toml_preserves_missing_final_newline_when_last_statement_survives() {
    let before = b"model = 'old'\r\n# keep\r\ncustom = 'last'";
    let patch = TomlPatch {
        set: vec![(KeyPath::new(&["model"]), toml_edit::value("new"))],
        ..Default::default()
    };
    assert_eq!(
        patch.apply(Path::new("config.toml"), Some(before)).unwrap(),
        b"model = \"new\"\r\n# keep\r\ncustom = 'last'"
    );
}

#[test]
fn toml_preserves_multiline_literal_bytes_and_individual_blank_line_endings() {
    let before = b"model = 'old'\r\n\n\r\ncustom = '''first\r\nsecond\nthird'''\r\n";
    let patch = TomlPatch {
        set: vec![(KeyPath::new(&["model"]), toml_edit::value("new"))],
        ..Default::default()
    };
    assert_eq!(
        patch.apply(Path::new("config.toml"), Some(before)).unwrap(),
        b"model = \"new\"\r\n\n\r\ncustom = '''first\r\nsecond\nthird'''\r\n"
    );
}

#[test]
fn toml_array_table_removal_retains_the_surviving_tables_own_line_endings() {
    struct RemoveFirst;
    impl super::patch::toml::TomlDocPatch for RemoveFirst {
        fn apply_to(
            &self,
            _path: &Path,
            doc: &mut toml_edit::DocumentMut,
        ) -> Result<(), LiveWriteError> {
            doc["items"].as_array_of_tables_mut().unwrap().remove(0);
            Ok(())
        }
    }
    let before = b"[[items]]\nname = 'first'\n\r\n[[items]]\r\nname = 'second'\r\n";
    let bytes = RemoveFirst
        .apply(Path::new("config.toml"), Some(before))
        .unwrap();
    let after = std::str::from_utf8(&bytes).unwrap();
    assert!(!after.contains("first"));
    assert!(after.ends_with("[[items]]\r\nname = 'second'\r\n"));
}

#[test]
fn dotenv_assignment_like_text_inside_multiline_quotes_is_not_a_target() {
    let before = b"USER_TEXT=\"line one\r\nGEMINI_MODEL=inside\nline three\"\r\nGEMINI_MODEL=old\n";
    let patch = DotenvPatch {
        set: vec![("GEMINI_MODEL".into(), "new".into())],
        ..Default::default()
    };
    assert_eq!(
        patch.apply(Path::new(".env"), Some(before)).unwrap(),
        b"USER_TEXT=\"line one\r\nGEMINI_MODEL=inside\nline three\"\r\nGEMINI_MODEL=new\n"
    );
}

#[test]
fn dotenv_replacing_a_multiline_target_does_not_leave_its_old_tail() {
    let before = b"GEMINI_MODEL='first\nsecond'\r\nUSER_FLAG=keep\n";
    let patch = DotenvPatch {
        set: vec![("GEMINI_MODEL".into(), "new".into())],
        ..Default::default()
    };
    assert_eq!(
        patch.apply(Path::new(".env"), Some(before)).unwrap(),
        b"GEMINI_MODEL=new\r\nUSER_FLAG=keep\n"
    );
}

#[test]
fn dotenv_unterminated_quote_refuses_output_instead_of_interpreting_inner_keys() {
    let before = b"USER_TEXT=\"unterminated\nGEMINI_MODEL=inside\n";
    let patch = DotenvPatch {
        set: vec![("GEMINI_MODEL".into(), "new".into())],
        ..Default::default()
    };
    assert!(matches!(
        patch.apply(Path::new(".env"), Some(before)),
        Err(LiveWriteError::Parse { .. })
    ));
}

#[test]
fn dotenv_quoted_blocks_preserve_escaped_quotes_and_broader_unowned_keys() {
    let patch = DotenvPatch {
        set: vec![("GEMINI_MODEL".into(), "new".into())],
        ..Default::default()
    };
    for quote in ['\'', '"', '`'] {
        let block = format!("export\tUSER.TEXT={quote}first\\{quote}\r\nGEMINI_MODEL=inside\nlast{quote} # keep\r\n");
        let before = format!("{block}GEMINI_MODEL=old\n");
        let expected = format!("{block}GEMINI_MODEL=new\n");
        assert_eq!(
            patch
                .apply(Path::new(".env"), Some(before.as_bytes()))
                .unwrap(),
            expected.as_bytes()
        );
    }
}

#[test]
fn dotenv_quote_starting_after_assignment_newline_stays_one_unowned_value() {
    let before = b"USER_TEXT=\r\n\"first\nGEMINI_MODEL=inside\nlast\"\r\nGEMINI_MODEL=old\n";
    let patch = DotenvPatch {
        set: vec![("GEMINI_MODEL".into(), "new".into())],
        ..Default::default()
    };
    assert_eq!(
        patch.apply(Path::new(".env"), Some(before)).unwrap(),
        b"USER_TEXT=\r\n\"first\nGEMINI_MODEL=inside\nlast\"\r\nGEMINI_MODEL=new\n"
    );
}

#[test]
fn dotenv_entries_do_not_expose_inner_assignment_text() {
    let before = "USER_TEXT=\"first\nGEMINI_MODEL=inside\nlast\"\nGEMINI_MODEL=real\n";
    assert_eq!(
        super::patch::dotenv::entries(before),
        vec![
            (
                "USER_TEXT".into(),
                "\"first\nGEMINI_MODEL=inside\nlast\"".into()
            ),
            ("GEMINI_MODEL".into(), "real".into())
        ]
    );
}
