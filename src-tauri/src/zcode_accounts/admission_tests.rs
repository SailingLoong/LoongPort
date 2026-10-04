use super::*;

fn observation() -> ContextObservation {
    ContextObservation {
        install:BuildFingerprint {platform:Platform::MacOs,version:"synthetic-3.14.4".into(),build:"synthetic-build".into(),artifact_sha256:[7;32]},
        credential_root:PathBuf::from("/synthetic/data/.zcode/v2"),
        settings_file:PathBuf::from("/synthetic/home/.zcode/v2/setting.json"),
        root_identity:[1,2], settings_identity:[1,3],
        home:"/synthetic/home".into(),settings_home:"/synthetic/home".into(),bootstrap_home:"/synthetic/home".into(),username:"synthetic-user".into(),
        key_choice:KeyMode::Standard,writers:WriterState::Stopped,
        settings:br#"{"dataBaseDir":"/synthetic/data","providerFamilyDomain":"zai","providerFamilyConnectionSelections":{"zai":{"kind":"individual-coding-plan"}},"unknown":{"preserve":true}}"#.to_vec(),
    }
}
fn contracts() -> Vec<ContractEntry> {
    vec![ContractEntry {
        fingerprint: observation().install,
        native_gate_passed: true,
    }]
}
fn rejected(observation: ContextObservation, reason: BlockedReason) {
    assert!(
        matches!(VerifiedContext::assess(observation,&contracts()),Err(actual) if actual==reason)
    );
}
#[test]
fn distinct_settings_home_and_data_base_bind_one_explicit_standard_context() {
    let context = VerifiedContext::assess(observation(), &contracts()).unwrap();
    assert_eq!(
        context.native_root(),
        Path::new("/synthetic/data/.zcode/v2")
    );
    assert_eq!(context.family(), OAuthFamily::Zai);
    assert!(!context.context_id().contains("synthetic-user"));
    assert!(!context.context_id().contains("/synthetic/"));
    context.cipher().unwrap();
}
#[test]
fn data_base_path_uses_ecmascript_trim_exactly() {
    for (setting, effective) in [
        ("\u{feff}/synthetic/data\u{feff}", "/synthetic/data"),
        ("/synthetic/data\u{0085}", "/synthetic/data\u{0085}"),
    ] {
        let mut input = observation();
        input.credential_root = Path::new(effective).join(".zcode/v2");
        input.settings = serde_json::to_vec(&serde_json::json!({
            "dataBaseDir": setting,
            "providerFamilyDomain": "zai",
            "providerFamilyConnectionSelections": {"zai": {"kind": "individual-coding-plan"}}
        }))
        .unwrap();
        let context = VerifiedContext::assess(input, &contracts()).unwrap();
        assert_eq!(
            context.native_root(),
            Path::new(effective).join(".zcode/v2")
        );
    }
}
#[test]
fn both_individual_families_and_start_plan_are_supported_without_guessing() {
    for (family, name) in [
        (OAuthFamily::Zai, "zai"),
        (OAuthFamily::BigModel, "bigmodel"),
    ] {
        for kind in ["start-plan", "individual-coding-plan"] {
            let mut input = observation();
            input.settings=serde_json::to_vec(&serde_json::json!({"dataBaseDir":"/synthetic/data","providerFamilyDomain":name,"providerFamilyConnectionSelections":{name:{"kind":kind}}})).unwrap();
            assert_eq!(
                VerifiedContext::assess(input, &contracts())
                    .unwrap()
                    .family(),
                family
            );
        }
    }
}
#[test]
fn unknown_build_platform_and_unpassed_gate_are_rejected() {
    for platform in [Platform::Linux, Platform::Windows] {
        let mut input = observation();
        input.install.platform = platform;
        rejected(input, BlockedReason::UnsupportedPlatform);
    }
    let mut input = observation();
    input.install.artifact_sha256[0] ^= 1;
    rejected(input, BlockedReason::UnsupportedBuild);
    let mut input = observation();
    input.install.build.push('x');
    rejected(input, BlockedReason::UnsupportedBuild);
    let mut entries = contracts();
    entries[0].native_gate_passed = false;
    assert!(matches!(
        VerifiedContext::assess(observation(), &entries),
        Err(BlockedReason::NativeGatePending)
    ));
}
#[test]
fn root_and_key_context_sources_must_be_explicit_and_consistent() {
    let mut input = observation();
    input.credential_root = PathBuf::from("/synthetic/wrong/.zcode/v2");
    rejected(input, BlockedReason::RootUnverified);
    let mut input = observation();
    input.settings_home = "/other-home".into();
    rejected(input, BlockedReason::RootUnverified);
    let mut input = observation();
    input.bootstrap_home = "/other-home".into();
    rejected(input, BlockedReason::RootUnverified);
    let mut input = observation();
    input.key_choice = KeyMode::Unknown;
    rejected(input, BlockedReason::KeyContextUnknown);
    let mut input = observation();
    input.key_choice = KeyMode::Custom;
    rejected(input, BlockedReason::CustomKeyContext);
    let mut input = observation();
    input.username.clear();
    rejected(input, BlockedReason::KeyContextUnknown);
}
#[test]
fn settings_do_not_migrate_or_guess_legacy_missing_team_or_corrupt_selection() {
    for (settings, reason) in [
        (
            r#"{"providerFamilyDomain":"zai","modelProviderFamilyModes":{"zai":"account"}}"#,
            BlockedReason::LegacySelection,
        ),
        (
            r#"{"providerFamilyDomain":"zai","providerFamilyConnectionSelections":{}}"#,
            BlockedReason::SelectionMissing,
        ),
        (
            r#"{"providerFamilyDomain":"zai","providerFamilyConnectionSelections":{"zai":{"kind":"team-coding-plan","organizationId":"org","projectId":"project","productId":"product"}}}"#,
            BlockedReason::TeamUnsupported,
        ),
        (
            r#"{"providerFamilyDomain":"zai","providerFamilyConnectionSelections":{"zai":{"kind":"unknown-plan"}}}"#,
            BlockedReason::SelectionMissing,
        ),
        (
            r#"{"providerFamilyDomain":"zai","providerFamilyDomain":"bigmodel"}"#,
            BlockedReason::SettingsInvalid,
        ),
        (
            r#"{"providerFamilyDomain":"zai","providerFamilyConnectionSelections":{"zai":{"kind":"start-plan","kind":"team-coding-plan"}}}"#,
            BlockedReason::SettingsInvalid,
        ),
        (
            r#"{"dataBaseDir":" ","providerFamilyDomain":"zai","providerFamilyConnectionSelections":{"zai":{"kind":"start-plan"}}}"#,
            BlockedReason::SettingsInvalid,
        ),
        ("broken", BlockedReason::SettingsInvalid),
    ] {
        let mut input = observation();
        input.settings = settings.as_bytes().into();
        input.credential_root = PathBuf::from("/synthetic/home/.zcode/v2");
        rejected(input, reason);
    }
}
#[test]
fn fresh_writer_and_settings_observations_are_required_for_each_recheck() {
    let context = VerifiedContext::assess(observation(), &contracts()).unwrap();
    context.recheck(observation(), &contracts()).unwrap();
    let mut input = observation();
    input.writers = WriterState::Running;
    assert_eq!(
        context.recheck(input, &contracts()),
        Err(BlockedReason::AppRunning)
    );
    let mut input = observation();
    input.writers = WriterState::Unknown;
    assert_eq!(
        context.recheck(input, &contracts()),
        Err(BlockedReason::WriterStateUnknown)
    );
    let mut input = observation();
    input.settings.push(b' ');
    assert_eq!(
        context.recheck(input, &contracts()),
        Err(BlockedReason::ContextChanged)
    );
    let mut input = observation();
    input.settings_identity[1] += 1;
    assert_eq!(
        context.recheck(input, &contracts()),
        Err(BlockedReason::ContextChanged)
    );
    let mut input = observation();
    input.root_identity[1] += 1;
    assert_eq!(
        context.recheck(input, &contracts()),
        Err(BlockedReason::ContextChanged)
    );
}
#[test]
fn blocked_reasons_give_specific_nonsecret_codes_and_next_actions() {
    assert_eq!(
        BlockedReason::AppRunning.remedy(),
        Remedy::QuitNativeWriters
    );
    assert_eq!(
        BlockedReason::KeyContextUnknown.remedy(),
        Remedy::UseStandardContext
    );
    assert_eq!(
        BlockedReason::LegacySelection.remedy(),
        Remedy::OpenNativeSettings
    );
    assert_eq!(
        BlockedReason::UnsupportedBuild.remedy(),
        Remedy::UseVerifiedBuild
    );
    assert_ne!(
        BlockedReason::AppRunning.code(),
        BlockedReason::WriterStateUnknown.code()
    );
}

#[test]
fn admission_does_not_retain_native_settings_body_and_uses_exact_node_fallback() {
    let context = VerifiedContext::assess(observation(), &contracts()).unwrap();
    assert!(context.observation.settings.is_empty());
    // Independent Node crypto fixture: platform must be darwin, not Rust's macos.
    let golden="enc:v1:AAECAwQFBgcICQoL.S6gMI3s7kCj45-xBCrUuHw.cluasfKzjsFSqJnNkVDnLJCwP57BlBmDqsfui9h8y8Cp";
    assert_eq!(
        context.cipher().unwrap().decrypt(golden).unwrap().as_str(),
        "SYNTHETIC_STANDARD_CONTEXT_CANARY"
    );
    let mut other = observation();
    other.username.push(' ');
    assert!(VerifiedContext::assess(other, &contracts())
        .unwrap()
        .cipher()
        .unwrap()
        .decrypt(golden)
        .is_err());
}
#[test]
fn stable_context_identity_excludes_active_family_but_binds_home_user_and_data_root() {
    let a = VerifiedContext::assess(observation(), &contracts()).unwrap();
    let mut other = observation();
    other.settings=br#"{"dataBaseDir":"/synthetic/data","providerFamilyDomain":"bigmodel","providerFamilyConnectionSelections":{"bigmodel":{"kind":"start-plan"}}}"#.to_vec();
    let b = VerifiedContext::assess(other, &contracts()).unwrap();
    assert_eq!(a.context_id(), b.context_id());
    let mut other = observation();
    other.username = "another-user".into();
    assert_ne!(
        a.context_id(),
        VerifiedContext::assess(other, &contracts())
            .unwrap()
            .context_id()
    );
    let mut other = observation();
    other.settings.resize(1024 * 1024 + 1, b' ');
    rejected(other, BlockedReason::SettingsInvalid);
    assert_eq!(BlockedReason::SelectContext.remedy(), Remedy::ChooseContext);
}

#[test]
fn admitted_context_rejects_target_from_another_family_or_storage_scope() {
    let context = VerifiedContext::assess(observation(), &contracts()).unwrap();
    context
        .accept_target(
            &AccountIdentity::new(context.context_id(), OAuthFamily::Zai, "synthetic-a").unwrap(),
        )
        .unwrap();
    for target in [
        AccountIdentity::new(context.context_id(), OAuthFamily::BigModel, "synthetic-a").unwrap(),
        AccountIdentity::new("other-context", OAuthFamily::Zai, "synthetic-a").unwrap(),
    ] {
        assert_eq!(
            context.accept_target(&target),
            Err(BlockedReason::TargetScopeMismatch)
        );
    }
}

#[test]
fn admission_must_bind_the_root_opened_by_the_transaction() {
    let context = VerifiedContext::assess(observation(), &contracts()).unwrap();
    context.confirm_root([1, 2]).unwrap();
    assert_eq!(
        context.confirm_root([1, 999]),
        Err(BlockedReason::ContextChanged)
    );
}

#[test]
fn explicit_standard_selection_does_not_claim_or_require_historical_launch_provenance() {
    assert!(VerifiedContext::assess(observation(), &contracts()).is_ok());
}

#[test]
fn context_revision_binds_build_settings_root_and_family_without_exposing_sources() {
    let original = VerifiedContext::assess(observation(), &contracts()).unwrap();
    assert_eq!(
        original.context_revision(),
        VerifiedContext::assess(observation(), &contracts())
            .unwrap()
            .context_revision()
    );
    assert!(!original.context_revision().contains("synthetic"));
    for change in 0..5 {
        let mut input = observation();
        let mut entries = contracts();
        match change {0=>input.root_identity[1]+=1,1=>input.settings_identity[1]+=1,2=>input.settings.push(b' '),3=>{input.install.artifact_sha256[0]^=1;entries[0].fingerprint=input.install.clone();},_=>input.settings=br#"{"dataBaseDir":"/synthetic/data","providerFamilyDomain":"bigmodel","providerFamilyConnectionSelections":{"bigmodel":{"kind":"start-plan"}}}"#.to_vec()}
        assert_ne!(
            original.context_revision(),
            VerifiedContext::assess(input, &entries)
                .unwrap()
                .context_revision()
        );
    }
}
