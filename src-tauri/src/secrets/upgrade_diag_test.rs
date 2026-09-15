//! 临时诊断：在真实升级数据（隔离副本）上复刻 initialize_runtime 的核心序列，
//! 定位 beta.1 真机首启卡死的失败点。数据准备：C:\loongport-diag\home（Windows）
//! 或 /tmp/loongport-diag/home（macOS），需为 v6.24 明文形状、无 vault.json。

#[test]
#[serial_test::serial]
fn diag_real_upgrade_repro() {
    let home = if cfg!(windows) {
        std::path::PathBuf::from("C:\\loongport-diag\\home")
    } else {
        std::path::PathBuf::from("/tmp/loongport-diag/home")
    };
    assert!(
        home.join("loongport.db").exists(),
        "diag home with real upgrade data must exist at {}",
        home.display()
    );
    let previous = std::env::var_os("CC_SWITCH_TEST_HOME");
    std::env::set_var("CC_SWITCH_TEST_HOME", &home);
    let root = crate::config::get_app_config_dir();
    eprintln!("DIAG root = {}", root.display());

    let store = super::testing::MemoryKeyStore::default();
    let session = match super::session::SecretSession::open(&root, &store, None) {
        Ok(session) => {
            eprintln!("DIAG session open ok");
            session
        }
        Err(error) => {
            eprintln!("DIAG session open FAILED: {error:?}");
            panic!("session open failed");
        }
    };
    let legacy = session.migration_pending().expect("migration state");
    eprintln!("DIAG legacy_allowed = {legacy}");

    match super::migration::prepare_files(&session, legacy) {
        Ok(()) => eprintln!("DIAG prepare_files ok"),
        Err(error) => {
            eprintln!("DIAG prepare_files FAILED: {error:?}");
            panic!("prepare_files failed");
        }
    }

    match crate::settings::unlock_settings_for_test(session.clone()) {
        Ok(()) => eprintln!("DIAG unlock_settings ok"),
        Err(error) => {
            eprintln!("DIAG unlock_settings FAILED: {error:?}");
            panic!("unlock_settings failed");
        }
    }

    match crate::database::Database::init_with_secrets(session.clone()) {
        Ok(db) => eprintln!("DIAG db init ok ({} providers)", {
            let conn = db.conn.lock().expect("conn");
            conn.query_row("SELECT COUNT(*) FROM providers", [], |r| r.get::<_, i64>(0))
                .unwrap_or(-1)
        }),
        Err(error) => {
            eprintln!("DIAG db init FAILED: {error:?}");
            panic!("db init failed");
        }
    }

    match previous {
        Some(value) => std::env::set_var("CC_SWITCH_TEST_HOME", value),
        None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
    }
}
