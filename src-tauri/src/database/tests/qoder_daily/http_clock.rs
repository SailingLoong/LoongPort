fn fixture_http(r: reqwest::Response) -> daily::HttpResponse {
    crate::rt::block_on(daily::HttpResponse::read(r, std::time::Instant::now()))
}
fn http_response(url: &str, status: u16, headers: &[(&str, &str)]) -> reqwest::Response {
    http_response_body(url, status, headers, String::new())
}
fn http_response_body(
    url: &str,
    status: u16,
    headers: &[(&str, &str)],
    body: String,
) -> reqwest::Response {
    use reqwest::ResponseBuilderExt;
    let mut b = http::Response::builder()
        .status(status)
        .url(url.parse().unwrap());
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    b.body(body).unwrap().into()
}
#[test]
fn actual_reqwest_metadata_rejects_duplicate_age_and_untrusted_transport_context() {
    let c = VerifiedContext::synthetic(identity("synthetic-provider"), "generation".into(), true);
    let date = "Thu, 01 Jan 1970 00:02:30 GMT";
    for (url, status, headers) in [
        (
            "https://openapi.qoder.sh/api/v1/campaigns",
            200,
            vec![("date", date), ("age", "0"), ("age", "4")],
        ),
        (
            "http://openapi.qoder.sh/api/v1/campaigns",
            200,
            vec![("date", date)],
        ),
        (
            "https://openapi.qoder.cn/api/v1/campaigns",
            200,
            vec![("date", date)],
        ),
        (
            "https://user@openapi.qoder.sh/api/v1/campaigns",
            200,
            vec![("date", date)],
        ),
        (
            "https://openapi.qoder.sh:444/api/v1/campaigns",
            200,
            vec![("date", date)],
        ),
        (
            "https://openapi.qoder.sh/api/v1/campaigns",
            302,
            vec![("date", date)],
        ),
        (
            "https://openapi.qoder.sh/api/v1/campaigns",
            200,
            vec![("date", date), ("via", "cache")],
        ),
        (
            "https://openapi.qoder.sh/api/v1/campaigns",
            200,
            vec![("date", date), ("date", date)],
        ),
    ] {
        assert!(
            daily::clock::from_response(
                &http_response(url, status, &headers),
                &c,
                std::time::Instant::now()
            )
            .is_none(),
            "{url} {status} {headers:?}"
        );
    }
}
#[test]
fn actual_reqwest_metadata_requires_date_and_short_measured_request() {
    let c = VerifiedContext::synthetic(identity("synthetic-provider"), "generation".into(), true);
    let url = "https://openapi.qoder.sh/api/v1/campaigns";
    assert!(daily::clock::from_response(
        &http_response(url, 200, &[]),
        &c,
        std::time::Instant::now()
    )
    .is_none());
    let r = http_response(url, 200, &[("date", "Thu, 01 Jan 1970 00:02:30 GMT")]);
    assert_eq!(
        daily::clock::from_response(&r, &c, std::time::Instant::now())
            .unwrap()
            .for_context(&c),
        Some(150)
    );
    assert!(daily::clock::from_response(
        &r,
        &c,
        std::time::Instant::now() - std::time::Duration::from_secs(31)
    )
    .is_none());
}
#[test]
fn foreign_http_receipt_origin_cannot_record_new_credits_even_with_matching_json() {
    struct ForeignReceipt(Gateway);
    impl CampaignAdapter for ForeignReceipt {
        fn verify(&self, p: &Provider, g: &str) -> Result<VerifiedContext, Dependency> {
            self.0.verify(p, g)
        }
        fn list(&self, c: &VerifiedContext) -> Result<Listing, error::AppError> {
            self.0.list(c)
        }
        fn post(
            &self,
            c: &VerifiedContext,
            b: &BoundCampaign,
        ) -> Result<Response, error::AppError> {
            let mut r = self.0.post(c, b)?;
            r.http = fixture_http(http_response("https://foreign.invalid/claim", 200, &[]));
            Ok(r)
        }
        fn get_reward(
            &self,
            c: &VerifiedContext,
            b: &BoundCampaign,
        ) -> Result<Response, error::AppError> {
            let mut r = self.0.get_reward(c, b)?;
            r.http = fixture_http(http_response("https://foreign.invalid/reward", 200, &[]));
            Ok(r)
        }
    }
    let f = Fixture::new();
    let db = f.open();
    let g = ForeignReceipt(Gateway::new());
    let r = daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &g).unwrap();
    assert!(matches!(
        r,
        ClaimResult::Observed {
            outcome: Outcome::Unknown,
            added_credits: None,
            ..
        }
    ));
    assert_eq!(
        db.qoder_operations(&identity("synthetic-provider"))
            .unwrap()[0]
            .outcome,
        Outcome::Unknown
    );
    let again = daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &g).unwrap();
    assert!(matches!(
        again,
        ClaimResult::Observed {
            action: Action::Get,
            outcome: Outcome::Unknown,
            added_credits: None,
            ..
        }
    ));
    assert_eq!(
        db.qoder_operations(&identity("synthetic-provider"))
            .unwrap()[0]
            .outcome,
        Outcome::Unknown
    );
    assert_eq!(g.0.posts.load(Ordering::SeqCst), 1);
}
#[test]
fn receipt_json_must_come_from_the_same_http_response() {
    struct SplitBody(Gateway);
    impl CampaignAdapter for SplitBody {
        fn verify(&self, p: &Provider, g: &str) -> Result<VerifiedContext, Dependency> {
            self.0.verify(p, g)
        }
        fn list(&self, c: &VerifiedContext) -> Result<Listing, error::AppError> {
            self.0.list(c)
        }
        fn post(
            &self,
            c: &VerifiedContext,
            b: &BoundCampaign,
        ) -> Result<Response, error::AppError> {
            let mut r = self.0.post(c, b)?;
            r.http = fixture_http(http_response("https://openapi.qoder.sh/claim", 200, &[]));
            Ok(r)
        }
        fn get_reward(
            &self,
            c: &VerifiedContext,
            b: &BoundCampaign,
        ) -> Result<Response, error::AppError> {
            self.0.get_reward(c, b)
        }
    }
    let f = Fixture::new();
    let db = f.open();
    let g = SplitBody(Gateway::new());
    let r = daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &g).unwrap();
    assert!(
        matches!(
            r,
            ClaimResult::Observed {
                outcome: Outcome::Unknown,
                added_credits: None,
                ..
            }
        ),
        "empty actual body must not inherit separately supplied success JSON: {r:?}"
    );
    assert_eq!(g.0.posts.load(Ordering::SeqCst), 1);
}
