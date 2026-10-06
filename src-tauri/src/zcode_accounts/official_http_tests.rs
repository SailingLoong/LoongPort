use super::super::official::SecretValue;
use super::*;
use std::{
    io::{Read, Write},
    net::TcpListener,
};

fn request(url: &str, method: OfficialMethod) -> OfficialRequest {
    OfficialRequest {
        method,
        url: url.into(),
        authorization: Some(SecretValue::new("synthetic-token").unwrap()),
        body: if method == OfficialMethod::Post {
            Some(Zeroizing::new(b"{}".to_vec()))
        } else {
            None
        },
    }
}

#[test]
fn fixed_official_routes_admit_only_expected_methods_and_origins() {
    for (url, method) in [
        (
            "https://zcode.z.ai/api/v1/oauth/cli/init",
            OfficialMethod::Post,
        ),
        (
            "https://zcode.z.ai/api/v1/oauth/cli/poll/test",
            OfficialMethod::Get,
        ),
        ("https://api.z.ai/api/auth/z/login", OfficialMethod::Post),
        (
            "https://bigmodel.cn/api/biz/customer/getCustomerInfo",
            OfficialMethod::Get,
        ),
        (
            "https://api.z.ai/api/biz/v1/organization/a/projects/b/api_keys",
            OfficialMethod::Post,
        ),
        (
            "https://api.z.ai/api/biz/v1/organization/a/projects/b/api_keys/copy/k",
            OfficialMethod::Get,
        ),
        (
            "https://zcode.z.ai/api/v1/zcode-plan/billing/balance?app_version=3.14.4",
            OfficialMethod::Get,
        ),
        (
            "https://api.z.ai/api/monitor/usage/quota/limit",
            OfficialMethod::Get,
        ),
    ] {
        assert_eq!(validate_request(&request(url, method)), Ok(()), "{url}");
    }
    for (url, method) in [
        (
            "https://api.z.ai.example.invalid/api/biz/customer/getCustomerInfo",
            OfficialMethod::Get,
        ),
        (
            "http://api.z.ai/api/biz/customer/getCustomerInfo",
            OfficialMethod::Get,
        ),
        (
            "https://user:secret@api.z.ai/api/biz/customer/getCustomerInfo",
            OfficialMethod::Get,
        ),
        (
            "https://api.z.ai:444/api/biz/customer/getCustomerInfo",
            OfficialMethod::Get,
        ),
        (
            "https://api.z.ai/api/biz/customer/getCustomerInfo#fragment",
            OfficialMethod::Get,
        ),
        (
            "https://api.z.ai/api/biz/customer/getCustomerInfo",
            OfficialMethod::Post,
        ),
        (
            "https://api.z.ai/api/paas/v4/chat/completions",
            OfficialMethod::Post,
        ),
        (
            "https://api.z.ai/api/biz/v1/organization/a/projects/b/api_keys/copy/k",
            OfficialMethod::Post,
        ),
        (
            "https://zcode.z.ai/api/v1/oauth/cli/init",
            OfficialMethod::Get,
        ),
    ] {
        assert_eq!(
            validate_request(&request(url, method)),
            Err(OfficialError::InvalidInput),
            "{url}"
        );
    }
}

// Only loopback HTTP with synthetic content is used. No official endpoint runs.
fn serve_once(response: Vec<u8>) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/synthetic", listener.local_addr().unwrap());
    let thread = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut bytes = [0u8; 4096];
        let _ = stream.read(&mut bytes);
        let _ = stream.write_all(&response);
    });
    (url, thread)
}

#[tokio::test]
async fn redirects_are_returned_without_following_the_location() {
    let (url,server)=serve_once(b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/do-not-follow\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec());
    let client = http_client_builder().no_proxy().build().unwrap();
    let result = execute_bounded(&client, client.get(url).build().unwrap())
        .await
        .unwrap();
    assert_eq!(result.status, 302);
    server.join().unwrap();
}

#[tokio::test]
async fn declared_and_chunked_response_sizes_are_bounded() {
    let oversized = vec![b'x'; MAX_RESPONSE_BYTES + 1];
    let mut chunked = format!(
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:X}\r\n",
        oversized.len()
    )
    .into_bytes();
    chunked.extend_from_slice(&oversized);
    chunked.extend_from_slice(b"\r\n0\r\n\r\n");
    let declared = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        MAX_RESPONSE_BYTES + 1
    )
    .into_bytes();
    for response in [declared, chunked] {
        let (url, server) = serve_once(response);
        let client = http_client_builder().no_proxy().build().unwrap();
        assert_eq!(
            execute_bounded(&client, client.get(url).build().unwrap())
                .await
                .unwrap_err(),
            OfficialError::ResponseTooLarge
        );
        server.join().unwrap();
    }
}

#[tokio::test]
async fn bounded_body_and_status_are_preserved() {
    let (url, server) =
        serve_once(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}".to_vec());
    let client = http_client_builder().no_proxy().build().unwrap();
    let response = execute_bounded(&client, client.get(url).build().unwrap())
        .await
        .unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.body.as_slice(), b"{}");
    server.join().unwrap();
}

#[tokio::test]
async fn timeouts_are_typed_and_never_include_request_secrets() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/synthetic-secret", listener.local_addr().unwrap());
    let client = http_client_builder()
        .no_proxy()
        .timeout(Duration::from_millis(20))
        .build()
        .unwrap();
    let error = execute_bounded(&client, client.get(url).build().unwrap())
        .await
        .unwrap_err();
    assert_eq!(error, OfficialError::Timeout);
    assert!(!format!("{error:?}").contains("synthetic-secret"));
}

#[tokio::test]
async fn invalid_origin_is_rejected_before_transport_and_post_is_never_retried() {
    let transport = ReqwestOfficialTransport::new().unwrap();
    assert_eq!(
        transport
            .send(request(
                "https://example.invalid/secret",
                OfficialMethod::Post
            ))
            .await
            .unwrap_err(),
        OfficialError::InvalidInput
    );
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/synthetic", listener.local_addr().unwrap());
    let client = http_client_builder().no_proxy().build().unwrap();
    let server = std::thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream
                        .set_read_timeout(Some(Duration::from_secs(1)))
                        .unwrap();
                    let mut bytes = [0; 4096];
                    let _ = stream.read(&mut bytes);
                    drop(stream);
                    return (listener, true);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if std::time::Instant::now() >= deadline {
                        return (listener, false);
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("loopback accept failed: {error}"),
            }
        }
    });
    let error = execute_bounded(
        &client,
        client.post(url).body("synthetic-body").build().unwrap(),
    )
    .await
    .unwrap_err();
    assert_eq!(error, OfficialError::Transport);
    let (listener, sent) = server.join().unwrap();
    assert!(sent, "the POST must actually reach the synthetic server");
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}
