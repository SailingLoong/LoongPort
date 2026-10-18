use std::io::{Read, Write};
type HttpReply = (u16, String, Vec<(String, String)>);
struct LocalHttp {
    origin: reqwest::Url,
    requests: Arc<Mutex<Vec<String>>>,
    task: Option<std::thread::JoinHandle<()>>,
}
impl LocalHttp {
    fn start(replies: Vec<HttpReply>) -> Self {
        Self::start_mode(replies, false)
    }
    fn start_mode(replies: Vec<HttpReply>, keep_alive: bool) -> Self {
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let origin = format!("http://{}/", listener.local_addr().unwrap())
            .parse()
            .unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let capture = requests.clone();
        let task = std::thread::spawn(move || {
            for (status, body, headers) in replies {
                let start = std::time::Instant::now();
                let mut stream = loop {
                    match listener.accept() {
                        Ok((s, _)) => break s,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(
                                start.elapsed() < std::time::Duration::from_secs(5),
                                "local HTTP request missing"
                            );
                            std::thread::sleep(std::time::Duration::from_millis(2));
                        }
                        Err(e) => panic!("{e}"),
                    }
                };
                // Accepted sockets may inherit the listener's nonblocking mode
                // on Unix platforms. Normalize the fixture, retaining the read timeout.
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut buf = [0u8; 4096];
                loop {
                    let n = stream.read(&mut buf).unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&buf[..n]);
                    assert!(bytes.len() < 65536);
                    if let Some(pos) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        let header = String::from_utf8_lossy(&bytes[..pos]);
                        let len = header
                            .lines()
                            .find_map(|l| {
                                l.to_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|n| n.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        if bytes.len() >= pos + 4 + len {
                            break;
                        }
                    }
                }
                capture
                    .lock()
                    .unwrap()
                    .push(String::from_utf8(bytes).unwrap());
                let mut output=format!("HTTP/1.1 {status} Synthetic\r\nContent-Length: {}\r\nConnection: {}\r\nContent-Type: application/json\r\n",body.len(),if keep_alive {"keep-alive"} else {"close"});
                for (k, v) in headers {
                    output.push_str(&format!("{k}: {v}\r\n"));
                }
                output.push_str("\r\n");
                // This fixture tests declared Content-Length rejection. Avoid
                // writing a body the client must reject without consuming.
                if body.len() <= 1024 * 1024 {
                    output.push_str(&body);
                }
                stream.write_all(output.as_bytes()).unwrap();
                if keep_alive {
                    let mut end = [0u8; 1];
                    assert_eq!(
                        stream.read(&mut end).unwrap(),
                        0,
                        "client must retire idle connection before its short runtime ends"
                    );
                }
            }
        });
        Self {
            origin,
            requests,
            task: Some(task),
        }
    }
    fn finish(mut self) -> Vec<String> {
        self.task.take().unwrap().join().unwrap();
        Arc::try_unwrap(self.requests)
            .unwrap()
            .into_inner()
            .unwrap()
    }
}
fn reply(body: serde_json::Value) -> HttpReply {
    (
        200,
        body.to_string(),
        vec![("Date".into(), "Thu, 01 Jan 1970 00:02:30 GMT".into())],
    )
}
fn bootstrap_reply() -> HttpReply {
    reply(serde_json::json!({"uid":"synthetic-official-subject","campaigns":[]}))
}
fn exchange_reply() -> HttpReply {
    reply(serde_json::json!({"token":"synthetic-job-token"}))
}
fn list_reply() -> HttpReply {
    reply(
        serde_json::json!({"uid":"synthetic-official-subject","campaigns":[{"campaignId":"synthetic-daily-1","campaignKey":"explicit-supported-target","actionType":"CLAIM_BENEFIT","claimStatus":"CLAIMABLE","startAt":100,"endAt":200,"benefit":{"kind":"CREDITS","amount":100}}]}),
    )
}
fn claim_reply(replay: bool) -> HttpReply {
    reply(
        serde_json::json!({"status":"CLAIMED","campaignId":"synthetic-daily-1","campaignKey":"explicit-supported-target","grantId":"synthetic-grant","claimedAt":"1970-01-01T00:02:30Z","replayed":replay}),
    )
}
fn declaration(f: &Fixture, db: &Database) -> daily::http::PatDeclaration {
    db.save_provider("codex", &f.load(db)).unwrap();
    daily::http::PatDeclaration {
        provider_id: "synthetic-provider".into(),
        credential_generation: state(f, db).generation,
        source: daily::http::CredentialSource::InternationalQoderPat,
    }
}
#[test]
fn actual_local_http_exchange_subject_device_list_and_claim_share_response_owner() {
    let f = Fixture::new();
    let db = f.open();
    let d = declaration(&f, &db);
    let server = LocalHttp::start(vec![
        exchange_reply(),
        bootstrap_reply(),
        list_reply(),
        claim_reply(false),
    ]);
    let adapter = daily::http::synthetic_adapter(server.origin.clone(), Some(d), true);
    let result =
        daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &adapter).unwrap();
    assert!(matches!(
        result,
        ClaimResult::Observed {
            action: Action::Post,
            outcome: Outcome::NewlyClaimed,
            added_credits: Some(100),
            ..
        }
    ));
    let requests = server.finish();
    assert_eq!(requests.len(), 4);
    assert!(requests[0].starts_with("POST /api/v1/jobToken/exchange "));
    assert!(
        requests[0].contains("\"personal_token\":\"non-secret-fixture-synthetic-generation-1\"")
    );
    assert!(!requests[0].contains("authorization:"));
    assert!(requests[1].starts_with("GET /sash/api/v1/me/campaigns?clientType=10 "));
    assert!(requests[1].contains("authorization: Bearer synthetic-job-token"));
    assert!(!requests[1].contains("cosy-machinetoken"));
    assert!(requests[2].contains("cosy-machinetoken: synthetic-device-token"));
    assert!(requests[3].starts_with("POST /sash/api/v1/me/campaigns/synthetic-daily-1/claim "));
}
#[test]
fn actual_http_bad_post_json_reopens_only_reward_get() {
    let f = Fixture::new();
    let db = f.open();
    let d = declaration(&f, &db);
    let server = LocalHttp::start(vec![
        exchange_reply(),
        bootstrap_reply(),
        list_reply(),
        (200, "not-json".into(), vec![]),
        exchange_reply(),
        bootstrap_reply(),
        claim_reply(true),
    ]);
    let adapter = daily::http::synthetic_adapter(server.origin.clone(), Some(d), true);
    assert!(matches!(
        daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &adapter).unwrap(),
        ClaimResult::Observed {
            outcome: Outcome::Unknown,
            ..
        }
    ));
    assert!(matches!(
        daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &adapter).unwrap(),
        ClaimResult::Observed {
            action: Action::Get,
            outcome: Outcome::ClaimedDeltaUnknown,
            added_credits: None,
            ..
        }
    ));
    let requests = server.finish();
    assert_eq!(
        requests
            .iter()
            .filter(|r| r.starts_with("POST /sash/"))
            .count(),
        1
    );
    assert!(requests[6].starts_with("GET /sash/api/v1/me/campaigns/synthetic-daily-1/reward "));
}
#[test]
fn actual_http_missing_device_is_explicit_after_authenticated_subject_without_claim() {
    let f = Fixture::new();
    let db = f.open();
    let d = declaration(&f, &db);
    let server = LocalHttp::start(vec![exchange_reply(), bootstrap_reply()]);
    let adapter = daily::http::synthetic_adapter(server.origin.clone(), Some(d), false);
    assert_eq!(
        daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &adapter).unwrap(),
        ClaimResult::Unavailable(Dependency::OfficialDeviceUnavailable)
    );
    assert_eq!(server.finish().len(), 2);
}
#[test]
fn actual_http_redirect_is_not_followed_or_retried() {
    let f = Fixture::new();
    let db = f.open();
    let d = declaration(&f, &db);
    let server = LocalHttp::start(vec![(
        302,
        String::new(),
        vec![(
            "Location".into(),
            "https://foreign.invalid/must-never-follow".into(),
        )],
    )]);
    let adapter = daily::http::synthetic_adapter(server.origin.clone(), Some(d), true);
    assert_eq!(
        daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &adapter).unwrap(),
        ClaimResult::Unavailable(Dependency::AuthenticationFailed)
    );
    assert_eq!(server.finish().len(), 1);
}
#[test]
fn declaration_cannot_survive_credential_generation_aba_or_wrong_provider() {
    let f = Fixture::new();
    let db = f.open();
    let d = declaration(&f, &db);
    let initial = f.load(&db);
    db.update_provider_settings_config(
        "codex",
        "synthetic-provider",
        &provider("other-candidate", None).settings_config,
    )
    .unwrap();
    db.update_provider_settings_config("codex", "synthetic-provider", &initial.settings_config)
        .unwrap();
    let adapter =
        daily::http::synthetic_adapter("http://127.0.0.1:1".parse().unwrap(), Some(d), true);
    assert_eq!(
        daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &adapter).unwrap(),
        ClaimResult::Unavailable(Dependency::AuthFamilyUnverified)
    );
    let mut d = declaration(&f, &db);
    d.provider_id = "other-provider".into();
    let adapter =
        daily::http::synthetic_adapter("http://127.0.0.1:1".parse().unwrap(), Some(d), true);
    assert_eq!(
        daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &adapter).unwrap(),
        ClaimResult::Unavailable(Dependency::AuthFamilyUnverified)
    );
}
#[test]
fn arbitrary_409_never_becomes_already_claimed_on_the_next_click() {
    let f = Fixture::new();
    let db = f.open();
    let d = declaration(&f, &db);
    let server = LocalHttp::start(vec![
        exchange_reply(),
        bootstrap_reply(),
        list_reply(),
        (409, "{}".into(), vec![]),
        exchange_reply(),
        bootstrap_reply(),
        list_reply(),
    ]);
    let adapter = daily::http::synthetic_adapter(server.origin.clone(), Some(d), true);
    assert!(matches!(
        daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &adapter).unwrap(),
        ClaimResult::Observed {
            outcome: Outcome::Conflict,
            ..
        }
    ));
    let second =
        daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &adapter).unwrap();
    assert!(
        matches!(
            second,
            ClaimResult::Observed {
                action: Action::Get,
                outcome: Outcome::Unknown,
                added_credits: None,
                ..
            }
        ),
        "unproven conflict must remain GET-only unknown: {second:?}"
    );
    assert_eq!(
        server
            .finish()
            .iter()
            .filter(|r| r.starts_with("POST /sash/"))
            .count(),
        1
    );
}
#[test]
fn actual_http_conflicting_bootstrap_identity_never_calls_device_or_claim() {
    let f = Fixture::new();
    let db = f.open();
    let d = declaration(&f, &db);
    let server = LocalHttp::start(vec![
        exchange_reply(),
        reply(
            serde_json::json!({"uid":"foreign","data":{"uid":"synthetic-official-subject","campaigns":[]}}),
        ),
    ]);
    let adapter = daily::http::synthetic_adapter(server.origin.clone(), Some(d), true);
    assert_eq!(
        daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &adapter).unwrap(),
        ClaimResult::Unavailable(Dependency::IdentityUnavailable)
    );
    assert_eq!(server.finish().len(), 2);
}
#[test]
fn actual_http_changed_device_listing_subject_never_creates_intent() {
    let f = Fixture::new();
    let db = f.open();
    let d = declaration(&f, &db);
    let mut list: serde_json::Value = serde_json::from_str(&list_reply().1).unwrap();
    list["uid"] = serde_json::json!("another-subject");
    let server = LocalHttp::start(vec![exchange_reply(), bootstrap_reply(), reply(list)]);
    let adapter = daily::http::synthetic_adapter(server.origin.clone(), Some(d), true);
    assert_eq!(
        daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &adapter).unwrap(),
        ClaimResult::Unavailable(Dependency::IdentityMismatch)
    );
    assert_eq!(server.finish().len(), 3);
    assert!(db
        .qoder_operations(&identity("synthetic-provider"))
        .unwrap()
        .is_empty());
}
#[test]
fn actual_http_declared_oversized_receipt_is_unknown_and_never_adds_credits() {
    let f = Fixture::new();
    let db = f.open();
    let d = declaration(&f, &db);
    let server = LocalHttp::start(vec![
        exchange_reply(),
        bootstrap_reply(),
        list_reply(),
        (200, " ".repeat(1024 * 1024 + 1), vec![]),
    ]);
    let adapter = daily::http::synthetic_adapter(server.origin.clone(), Some(d), true);
    assert!(matches!(
        daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &adapter).unwrap(),
        ClaimResult::Observed {
            outcome: Outcome::Unknown,
            added_credits: None,
            ..
        }
    ));
    assert_eq!(server.finish().len(), 4);
}
#[test]
fn actual_http_backend_selects_unique_admitted_campaign_without_ui_key() {
    let f = Fixture::new();
    let db = f.open();
    let d = declaration(&f, &db);
    let server = LocalHttp::start(vec![
        exchange_reply(),
        bootstrap_reply(),
        list_reply(),
        claim_reply(false),
    ]);
    let adapter = daily::http::synthetic_adapter(server.origin.clone(), Some(d), true);
    assert!(matches!(
        daily::prepare_selected(&db, "synthetic-provider", None, &adapter).unwrap(),
        ClaimResult::Observed {
            action: Action::Post,
            added_credits: Some(100),
            ..
        }
    ));
    assert_eq!(server.finish().len(), 4);
}
#[test]
fn actual_http_ambiguous_eligible_campaigns_never_choose_first() {
    let f = Fixture::new();
    let db = f.open();
    let d = declaration(&f, &db);
    let mut listing: serde_json::Value = serde_json::from_str(&list_reply().1).unwrap();
    let mut second = listing["campaigns"][0].clone();
    second["campaignId"] = serde_json::json!("second-daily");
    listing["campaigns"].as_array_mut().unwrap().push(second);
    let server = LocalHttp::start(vec![exchange_reply(), bootstrap_reply(), reply(listing)]);
    let adapter = daily::http::synthetic_adapter(server.origin.clone(), Some(d), true);
    assert_eq!(
        daily::prepare_selected(&db, "synthetic-provider", None, &adapter).unwrap(),
        ClaimResult::Unavailable(Dependency::NoEligibleCampaign)
    );
    assert_eq!(server.finish().len(), 3);
}

#[test]
fn actual_http_keep_alive_cannot_reuse_a_destroyed_cli_runtime_connection() {
    let f = Fixture::new();
    let db = f.open();
    let d = declaration(&f, &db);
    let server = LocalHttp::start_mode(
        vec![
            exchange_reply(),
            bootstrap_reply(),
            list_reply(),
            claim_reply(false),
        ],
        true,
    );
    let adapter = daily::http::synthetic_adapter(server.origin.clone(), Some(d), true);
    assert!(matches!(
        daily::prepare_selected(&db, "synthetic-provider", None, &adapter).unwrap(),
        ClaimResult::Observed {
            action: Action::Post,
            added_credits: Some(100),
            ..
        }
    ));
    assert_eq!(server.finish().len(), 4);
}
// A real InstalledApp probe is intentionally not invoked by this suite:
// on a runner with Qoder installed it could reach a real exchange. Dependency
// admission is tested with synthetic_adapter above and device.rs fake Platform.
