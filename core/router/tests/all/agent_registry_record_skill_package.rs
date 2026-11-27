// SPDX-License-Identifier: CC0-1.0

//! The skill package a registry record of `agent-registry-control` carries, driven over
//! HTTP through the real router so every assertion goes through routing, deserialization,
//! the handler and serialization.
//!
//! The rule under test: a write that uploads a package stores it, and a read reports the
//! package the store holds as the SHA-256 checksum of its bytes — never the bytes
//! themselves. A write is refused where the request does not describe a package the service
//! can store: a checksum that is not the checksum of the content supplied with it, or an
//! archive over the documented size. An edit carries the model's patch semantics for it:
//! omitted leaves the package alone, an empty wrapper removes it, and a wrapper carrying a
//! package replaces it.
//!
//! Every wait here is a wait on the CONDITION — a poll loop up to a generous deadline — so
//! a loaded host only polls more times.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use smithy_server_runtime::MultiServiceRouter;

/// Percent-encode one URI path segment the way an SDK encodes a non-greedy `@httpLabel`.
/// This crate's one encoder, shared rather than copied.
use crate::cloudfront_key_value_store_handshake::segment as encode_label;

/// A SigV4 `Authorization` header whose credential scope names this service's signing name.
/// The signature is never checked: this router runs with request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTAGENTREGISTRY/20260101/us-west-2/agent-registry/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";

/// The host that names this service's endpoint — the `endpointPrefix` its model publishes.
const HOST: &str = "agent-registry-control.us-west-2.amazonaws.com";

/// How long a wait for a transition the clock owes may take before a test gives up.
/// Generous on purpose: the wait is on the CONDITION, so a loaded host only polls more
/// times.
const TRANSITION_DEADLINE: Duration = Duration::from_secs(30);

/// How long a poll loop pauses between two looks at a resource.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// The largest skill package a write admits: "the ZIP archive of the skill package, up to 5
/// MiB (5,242,880 bytes)", as `RegistryRecordArtifactUpload$content` documents it
/// (`smithy_query(query="shape", shape="RegistryRecordArtifactUpload")`).
const MAX_ARTIFACT_BYTES: usize = 5_242_880;

/// One HTTP answer, reduced to what these tests assert on.
struct Reply {
    status: u16,
    code: String,
    body: Value,
}

impl Reply {
    fn str_field(&self, name: &str) -> &str {
        self.body[name].as_str().unwrap_or_default()
    }

    fn field(&self, name: &str) -> &Value {
        &self.body[name]
    }
}

/// A router carrying the Agent Registry control plane over one shared engine.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_agentregistrycontrol_agentregistrycontrol::EmulatedService::new(
            engine,
        ),
    );
    router
}

async fn call(router: &MultiServiceRouter, method: &str, path: &str, body: Option<Value>) -> Reply {
    let payload = match &body {
        Some(value) => Bytes::from(serde_json::to_vec(value).expect("serializable body")),
        None => Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{HOST}{path}"))
        .header("host", HOST)
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(payload)
        .expect("a valid request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let header_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = response.into_body();
    let body: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    let code = if header_code.is_empty() {
        body.get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    } else {
        header_code
    };
    let code = code
        .rsplit(['#', '/'])
        .next()
        .unwrap_or_default()
        .to_string();
    Reply { status, code, body }
}

/// The id at the end of a minted ARN.
fn id_of(arn: &str) -> &str {
    arn.rsplit('/').next().unwrap_or(arn)
}

/// The SHA-256 checksum of `content`, "as 64 lowercase hexadecimal characters" — the form
/// `Sha256Hash` declares.
fn checksum(content: &[u8]) -> String {
    hex::encode(Sha256::digest(content))
}

/// A package's bytes as a JSON request carries them: base64, which the AWS SDKs encode for
/// the caller.
fn encoded(content: &[u8]) -> Value {
    json!(::aws_smithy_types::base64::encode(content))
}

/// Create a registry and wait for the creation the clock owes so it is `READY`. Returns its
/// id.
async fn ready_registry(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let created = call(router, "POST", "/registries", Some(json!({ "name": name }))).await;
    assert_eq!(
        created.status, 202,
        "case {case:?}: CreateRegistry answers 202, got {} {:?} with {:?}",
        created.status, created.code, created.body
    );
    let registry_id = id_of(created.str_field("registryArn")).to_string();

    let started = std::time::Instant::now();
    loop {
        let got = call(
            router,
            "GET",
            &format!("/registries/{}", encode_label(&registry_id)),
            None,
        )
        .await;
        if got.str_field("status") == "READY" {
            return registry_id;
        }
        assert!(
            started.elapsed() < TRANSITION_DEADLINE,
            "case {case:?}: a created registry becomes READY, last answer {} {:?} with {:?}",
            got.status,
            got.code,
            got.body
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Read a record back.
async fn read_record(router: &MultiServiceRouter, registry_id: &str, record_id: &str) -> Reply {
    call(
        router,
        "GET",
        &format!(
            "/registries/{}/records/{}",
            encode_label(registry_id),
            encode_label(record_id)
        ),
        None,
    )
    .await
}

/// Look at a record until it reports `wanted`, or give up at the deadline. The wait is on
/// the CONDITION: a record carries `CREATING` while its create is in flight and `UPDATING`
/// while an edit is, and a change of its own is not applied while one is.
async fn record_reaching(
    router: &MultiServiceRouter,
    case: &str,
    registry_id: &str,
    record_id: &str,
    wanted: &str,
) -> Reply {
    let started = std::time::Instant::now();
    loop {
        let got = read_record(router, registry_id, record_id).await;
        if got.str_field("status") == wanted {
            return got;
        }
        assert!(
            started.elapsed() < TRANSITION_DEADLINE,
            "case {case:?}: the record never reached {wanted}, last answer {} {:?} with {:?}",
            got.status,
            got.code,
            got.body
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The `descriptors` a `SKILL` record carries here: a custom descriptor, whose payload is
/// the JSON this service admits for one (`smithy_query(query="shape", shape="Descriptors")`).
fn descriptors() -> Value {
    json!({ "custom": { "data": "{\"skill\":\"checkout\"}" } })
}

#[tokio::test]
async fn a_stored_skill_package_is_read_back_as_the_checksum_of_its_bytes() {
    let case = "skill package round trip";
    let router = router();
    let registry_id = ready_registry(&router, case, "record-skill-packages").await;

    // The rows differ along the package's bytes and whether the request stated the checksum
    // itself, which is everything the upload varies over: the checksum a read reports is
    // computed over the content either way.
    let rows: Vec<(&str, Vec<u8>, bool)> = vec![
        ("a short archive, checksum stated", b"PK\x03\x04 a tiny skill".to_vec(), true),
        ("a short archive, no checksum stated", b"PK\x03\x04 another skill".to_vec(), false),
        ("bytes that are not text", (0_u8..=255).collect(), true),
        ("a kibibyte of content", vec![b'z'; 1024], false),
        ("one byte", vec![7], true),
    ];

    for (index, (row, content, state_checksum)) in rows.into_iter().enumerate() {
        let expected = checksum(&content);
        let mut artifact = json!({ "content": encoded(&content) });
        if state_checksum {
            artifact["sha256"] = json!(expected);
        }
        let created = call(
            &router,
            "POST",
            &format!("/registries/{}/records", encode_label(&registry_id)),
            Some(json!({
                "name": format!("packaged-skill-{index}"),
                "recordType": "SKILL",
                "descriptors": descriptors(),
                "artifact": artifact,
            })),
        )
        .await;
        assert_eq!(
            created.status, 202,
            "case {row:?}: CreateRegistryRecord answers 202 for a record carrying a package, got \
             {} {:?} with {:?}",
            created.status, created.code, created.body
        );
        let record_id = id_of(created.str_field("recordArn")).to_string();

        let got = read_record(&router, &registry_id, &record_id).await;
        assert_eq!(
            got.status, 200,
            "case {row:?}: GetRegistryRecord answers 200, got {} {:?} with {:?}",
            got.status, got.code, got.body
        );
        assert_eq!(
            got.field("artifact")["sha256"].as_str().unwrap_or_default(),
            expected,
            "case {row:?}: the read reports the checksum of the package that was stored, got {:?}",
            got.body
        );
        // "Responses never include the package bytes", and this emulator serves no location
        // the package can be downloaded from, so it claims no download URL either.
        assert!(
            got.field("artifact")["downloadUrl"].is_null(),
            "case {row:?}: the read claims no download location, got {:?}",
            got.body
        );
        assert!(
            got.field("artifact")["content"].is_null(),
            "case {row:?}: the read carries no package bytes, got {:?}",
            got.body
        );
    }

    // A record uploaded with no package has none to report.
    let created = call(
        &router,
        "POST",
        &format!("/registries/{}/records", encode_label(&registry_id)),
        Some(json!({
            "name": "packageless-skill",
            "recordType": "SKILL",
            "descriptors": descriptors(),
        })),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "case {case:?}: a record without a package is created, got {} {:?} with {:?}",
        created.status, created.code, created.body
    );
    let got = read_record(&router, &registry_id, id_of(created.str_field("recordArn"))).await;
    assert!(
        got.field("artifact").is_null(),
        "case {case:?}: a record that carries no package reports none, got {:?}",
        got.body
    );
}

#[tokio::test]
async fn a_package_the_request_does_not_describe_is_refused() {
    let case = "skill package refusals";
    let router = router();
    let registry_id = ready_registry(&router, case, "record-skill-package-refusals").await;

    // The rows span the two ways a request can fail to describe a storable package: a
    // checksum that is not the content's, and an archive past the documented size — at the
    // bound, which is admitted, and one byte past it, which is not.
    let content = b"PK\x03\x04 a skill".to_vec();
    let rows: Vec<(&str, Value, Option<u16>)> = vec![
        (
            "a checksum of other content",
            json!({ "content": encoded(&content), "sha256": checksum(b"something else") }),
            Some(400),
        ),
        (
            "a checksum of the content",
            json!({ "content": encoded(&content), "sha256": checksum(&content) }),
            None,
        ),
        (
            "an archive at the documented bound",
            json!({ "content": encoded(&vec![b'a'; MAX_ARTIFACT_BYTES]) }),
            None,
        ),
        (
            "an archive one byte past the bound",
            json!({ "content": encoded(&vec![b'a'; MAX_ARTIFACT_BYTES + 1]) }),
            Some(400),
        ),
    ];

    for (index, (row, artifact, refused)) in rows.into_iter().enumerate() {
        let name = format!("bounded-skill-{index}");
        let created = call(
            &router,
            "POST",
            &format!("/registries/{}/records", encode_label(&registry_id)),
            Some(json!({
                "name": name,
                "recordType": "SKILL",
                "descriptors": descriptors(),
                "artifact": artifact,
            })),
        )
        .await;
        match refused {
            Some(status) => {
                assert_eq!(
                    (created.status, created.code.as_str()),
                    (status, "ValidationException"),
                    "case {row:?}: the write is refused as a bad request, got {} {:?} with {:?}",
                    created.status,
                    created.code,
                    created.body
                );
                // A refused write stored nothing: the name it asked for is still free.
                let again = call(
                    &router,
                    "POST",
                    &format!("/registries/{}/records", encode_label(&registry_id)),
                    Some(json!({
                        "name": name,
                        "recordType": "SKILL",
                        "descriptors": descriptors(),
                    })),
                )
                .await;
                assert_eq!(
                    again.status, 202,
                    "case {row:?}: the refused write created no record, so the name is free, got \
                     {} {:?} with {:?}",
                    again.status,
                    again.code,
                    again.body
                );
            }
            None => assert_eq!(
                created.status, 202,
                "case {row:?}: the write is admitted, got {} {:?} with {:?}",
                created.status, created.code, created.body
            ),
        }
    }
}

#[tokio::test]
async fn an_edit_leaves_replaces_or_removes_the_stored_package() {
    let case = "skill package patch semantics";
    let router = router();
    let registry_id = ready_registry(&router, case, "record-skill-package-edits").await;

    let first = b"PK\x03\x04 the first package".to_vec();
    let second = b"PK\x03\x04 the replacement package".to_vec();
    let created = call(
        &router,
        "POST",
        &format!("/registries/{}/records", encode_label(&registry_id)),
        Some(json!({
            "name": "edited-skill",
            "recordType": "SKILL",
            "descriptors": descriptors(),
            "artifact": { "content": encoded(&first) },
        })),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "case {case:?}: the record is created with a package, got {} {:?} with {:?}",
        created.status, created.code, created.body
    );
    let record_id = id_of(created.str_field("recordArn")).to_string();

    // "Omit to leave the package unchanged; supply an empty object to remove an uploaded
    // package; supply optionalValue to upload a new package"
    // (`smithy_query(query="shape", shape="UpdatedRegistryRecordArtifact")`). The rows walk
    // all three, each asserting the checksum the record reports afterwards.
    let rows: Vec<(&str, Value, Option<String>)> = vec![
        (
            "an edit that names no package",
            json!({ "displayName": { "optionalValue": "Edited skill" } }),
            Some(checksum(&first)),
        ),
        (
            "an edit that uploads another package",
            json!({ "artifact": { "optionalValue": { "content": encoded(&second) } } }),
            Some(checksum(&second)),
        ),
        (
            "an edit that removes the package",
            json!({ "artifact": {} }),
            None,
        ),
    ];

    for (row, body, expected) in rows {
        // An edit applies to a record whose own asynchronous change has finished, so each
        // row waits for the record to settle before it edits again.
        record_reaching(&router, row, &registry_id, &record_id, "DRAFT").await;
        let edited = call(
            &router,
            "PATCH",
            &format!(
                "/registries/{}/records/{}",
                encode_label(&registry_id),
                encode_label(&record_id)
            ),
            Some(body),
        )
        .await;
        assert_eq!(
            edited.status, 202,
            "case {row:?}: UpdateRegistryRecord answers 202, got {} {:?} with {:?}",
            edited.status, edited.code, edited.body
        );
        let got = read_record(&router, &registry_id, &record_id).await;
        match expected {
            Some(expected) => assert_eq!(
                got.field("artifact")["sha256"].as_str().unwrap_or_default(),
                expected,
                "case {row:?}: the record reports the package the edit left on it, got {:?}",
                got.body
            ),
            None => assert!(
                got.field("artifact").is_null(),
                "case {row:?}: the record reports no package once the edit removed it, got {:?}",
                got.body
            ),
        }
    }
}
