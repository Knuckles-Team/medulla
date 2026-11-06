//! S3-compatible object API round trip with signed requests (MEDULLA-DURABLE-KERNEL-R860.4).
//!
//! Starts `medulla_wire_msg::s3::serve` (the function `spawn_s3_listener` runs) with
//! runtime-injected access and secret credentials, then drives create-bucket, put,
//! get, list and delete over HTTP. Each request is signed with `Authorization`
//! header SigV4 by the `aws-sigv4` signer and sent over `reqwest`. Unsigned and
//! presigned (query-authenticated) requests are refused with `AccessDenied`.

#![cfg(feature = "s3-api")]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use std::time::SystemTime;

use aws_credential_types::Credentials;
use aws_sigv4::http_request::{
    sign, PayloadChecksumKind, PercentEncodingMode, SignableBody, SignableRequest, SigningSettings,
    UriPathNormalizationMode,
};
use aws_sigv4::sign::v4;
use medulla_wire_msg::s3::{S3_ACCESS_KEY_ENV, S3_SECRET_KEY_ENV};

const ACCESS_KEY: &str = "medulla-s3-roundtrip-access";
const SECRET_KEY: &str = "medulla-s3-roundtrip-secret";

struct SignedClient {
    base: String,
    http: reqwest::Client,
}

impl SignedClient {
    async fn send(&self, method: &str, path_and_query: &str, body: &[u8]) -> (u16, Vec<u8>) {
        let url = format!("{}{}", self.base, path_and_query);
        let identity = Credentials::new(ACCESS_KEY, SECRET_KEY, None, None, "s3-roundtrip").into();
        let mut settings = SigningSettings::default();
        settings.payload_checksum_kind = PayloadChecksumKind::XAmzSha256;
        settings.percent_encoding_mode = PercentEncodingMode::Single;
        settings.uri_path_normalization_mode = UriPathNormalizationMode::Disabled;
        let params: aws_sigv4::http_request::SigningParams<'_> = v4::SigningParams::builder()
            .identity(&identity)
            .region("us-east-1")
            .name("s3")
            .time(SystemTime::now())
            .settings(settings)
            .build()
            .expect("sigv4 params")
            .into();
        let signable = SignableRequest::new(
            method,
            url.as_str(),
            std::iter::empty::<(&str, &str)>(),
            SignableBody::Bytes(body),
        )
        .expect("signable request");
        let (instructions, _signature) = sign(signable, &params).expect("sign").into_parts();
        let mut request = self.http.request(
            reqwest::Method::from_bytes(method.as_bytes()).unwrap(),
            url.as_str(),
        );
        for (name, value) in instructions.headers() {
            request = request.header(name, value);
        }
        let response = request
            .body(body.to_vec())
            .send()
            .await
            .expect("s3 request");
        let status = response.status().as_u16();
        let bytes = response.bytes().await.expect("s3 body").to_vec();
        (status, bytes)
    }
}

async fn spawn_s3() -> String {
    std::env::set_var(S3_ACCESS_KEY_ENV, ACCESS_KEY);
    std::env::set_var(S3_SECRET_KEY_ENV, SECRET_KEY);
    let dir = test_support::fresh_dir("medulla-s3-roundtrip");
    let state = test_support::seeded_wire_state(
        "test",
        "tester",
        Some(dir.to_string_lossy().into_owned()),
        test_support::durable_persistence("s3-roundtrip-recovery-key"),
    );
    let addr = test_support::ephemeral_listener_addr().await;
    let serve_addr = addr.clone();
    tokio::spawn(async move {
        let _ = medulla_wire_msg::s3::serve(&serve_addr, state).await;
    });
    test_support::wait_for_listener_ready(&addr).await;
    format!("http://{addr}")
}

/// Verifies MEDULLA-DURABLE-KERNEL-R860.4: S3-compatible object API.
// spec: MEDULLA-DURABLE-KERNEL-R860.4
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn put_get_list_delete() {
    let base = spawn_s3().await;
    let client = SignedClient {
        base: base.clone(),
        http: reqwest::Client::new(),
    };

    // Unsigned and presigned requests never reach the store.
    let unsigned = reqwest::Client::new()
        .get(format!("{base}/roundtrip/a.txt"))
        .send()
        .await
        .expect("unsigned request");
    assert_eq!(unsigned.status().as_u16(), 403);
    assert!(String::from_utf8_lossy(&unsigned.bytes().await.unwrap()).contains("AccessDenied"));
    let presigned = reqwest::Client::new()
        .get(format!(
            "{base}/roundtrip/a.txt?X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential={ACCESS_KEY}%2F20260101%2Fus-east-1%2Fs3%2Faws4_request&X-Amz-Signature=00"
        ))
        .send()
        .await
        .expect("presigned request");
    assert_eq!(presigned.status().as_u16(), 403);
    assert!(String::from_utf8_lossy(&presigned.bytes().await.unwrap()).contains("AccessDenied"));

    let (status, _) = client.send("PUT", "/roundtrip", b"").await;
    assert_eq!(status, 200, "create bucket");

    let payload = b"one store behind the S3 surface";
    let (status, _) = client.send("PUT", "/roundtrip/a.txt", payload).await;
    assert_eq!(status, 200, "put object");

    let (status, body) = client.send("GET", "/roundtrip/a.txt", b"").await;
    assert_eq!(status, 200, "get object");
    assert_eq!(body, payload);

    let (status, body) = client.send("GET", "/roundtrip?list-type=2", b"").await;
    assert_eq!(status, 200, "list objects");
    let listing = String::from_utf8_lossy(&body);
    assert!(
        listing.contains("<Key>a.txt</Key>"),
        "listing names the key: {listing}"
    );

    let (status, _) = client.send("DELETE", "/roundtrip/a.txt", b"").await;
    assert_eq!(status, 204, "delete object");

    let (status, body) = client.send("GET", "/roundtrip/a.txt", b"").await;
    assert_eq!(status, 404, "deleted object is gone");
    assert!(String::from_utf8_lossy(&body).contains("NoSuchKey"));

    let (_, body) = client.send("GET", "/roundtrip?list-type=2", b"").await;
    assert!(!String::from_utf8_lossy(&body).contains("<Key>a.txt</Key>"));
}
