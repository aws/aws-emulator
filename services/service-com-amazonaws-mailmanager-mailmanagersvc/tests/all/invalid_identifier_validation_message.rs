// SPDX-License-Identifier: CC0-1.0

//! How this service words a VALIDATION refusal of an identifier that resolves to nothing,
//! over the wire.
//!
//! Every assertion goes through the real request path — `MultiServiceRouter::dispatch` with
//! `awsJson1_0` bodies, `POST /` and `X-Amz-Target: MailManagerSvc.<Operation>` — so what it
//! pins is the emulator's HTTP behaviour, not an internal helper.
//!
//! RULE: where an operation reports an identifier it has NO resource for as a fault in the
//! REQUEST (the modelled `ValidationException`, HTTP 400), the message is the fixed text
//! `Invalid ` followed by the INPUT MEMBER's name as the model spells it, and it echoes
//! neither the value it was handed nor anything from the store.
//!
//! The table spans what that generalises over: the operations that resolve such an
//! identifier and the member each takes (`SearchId` on both archive-search reads, `JobId` on
//! the address-list import-job read), and the shape of the unresolvable value (both ends of
//! each member's modelled length, a plausible minted-looking value, and for the
//! pattern-constrained member a hyphenated form the pattern admits).
//!
//! A `SearchId` whose search this service DOES have a record of, but can no longer serve
//! because its archive went away, is a different condition and carries its own wording;
//! `search_read_not_found_message.rs` and `archive_search_reachability.rs` pin that one. This
//! suite is about the wording an identifier naming nothing carries. Nothing here waits for a
//! clock: an identifier that names nothing is refused whatever state the store is in.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_mailmanager_mailmanagersvc::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for.
const REGION: &str = "eu-west-1";

/// One emulator holding one MailManager service on one shared engine.
struct Harness {
    router: MultiServiceRouter,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The signature itself is not under test here, and the fixed test credential below
        // is not this process's signing key.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Call one operation and return `(http status, parsed body)`.
    async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("MailManagerSvc.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261021/{REGION}/ses/aws4_request, \
                     SignedHeaders=host;x-amz-target, Signature=0000"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a valid HTTP request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            json!({})
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }
}

/// The wire error code of a failure body: the `__type` member, without its namespace.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .split('#')
        .next_back()
        .unwrap_or_default()
        .to_string()
}

/// The `Message` of a failure body, whichever casing the protocol put it under.
fn error_message(body: &Value) -> Option<&str> {
    body["Message"]
        .as_str()
        .or_else(|| body["message"].as_str())
}

/// One row of the table: an operation, the input member it resolves, and the unresolvable
/// value handed to it.
struct Case {
    /// What makes this row different from the others.
    label: &'static str,
    operation: &'static str,
    /// The input member the operation resolves the identifier through — and therefore the
    /// name the refusal must state.
    member: &'static str,
    value: String,
}

/// Every operation that answers an unresolvable identifier with a validation failure states
/// the member and nothing else: HTTP 400, `ValidationException`, `Invalid <Member>`.
#[tokio::test]
async fn an_unresolvable_identifier_is_refused_as_an_invalid_member() {
    let harness = Harness::new();

    // The members' lengths and patterns are the modelled ones:
    // `smithy_query(query="shape",shape="SearchId,JobId")` — `SearchId` is a 1..64 string,
    // `JobId` a 1..255 string matching `^[a-zA-Z0-9-]+$`.
    let mut cases: Vec<Case> = Vec::new();
    for operation in ["GetArchiveSearch", "GetArchiveSearchResults"] {
        for (label, value) in [
            ("the shortest value the length admits", "s".to_string()),
            ("a plausible minted value", "0123456789abcdef".repeat(2)),
            ("the longest value the length admits", "s".repeat(64)),
        ] {
            cases.push(Case {
                label,
                operation,
                member: "SearchId",
                value,
            });
        }
    }
    for (label, value) in [
        ("the shortest value the length admits", "j".to_string()),
        ("a plausible minted value", "0123456789abcdef".repeat(2)),
        ("a hyphenated value the pattern admits", "no-such-job-0001".to_string()),
        ("the longest value the length admits", "j".repeat(255)),
    ] {
        cases.push(Case {
            label,
            operation: "GetAddressListImportJob",
            member: "JobId",
            value,
        });
    }

    for case in &cases {
        let Case {
            label,
            operation,
            member,
            value,
        } = case;
        // The member's NAME is part of the row, so the request body is built from it.
        let mut body = serde_json::Map::new();
        body.insert((*member).to_string(), Value::String(value.clone()));
        let (status, out) = harness.call(operation, Value::Object(body)).await;
        assert_eq!(
            status, 400,
            "case {label:?} ({operation}/{member}): an unresolvable identifier must be \
             refused with 400, got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ValidationException",
            "case {label:?} ({operation}/{member}): unexpected error code, got {out:?}"
        );
        let expected = format!("Invalid {member}");
        assert_eq!(
            error_message(&out),
            Some(expected.as_str()),
            "case {label:?} ({operation}/{member}): expected message {expected:?}, got {out:?}"
        );
        // The refusal is wholly fixed text: it must not echo the value it was handed.
        assert!(
            !error_message(&out).unwrap_or_default().contains(value.as_str()),
            "case {label:?} ({operation}/{member}): the message must echo no value, got {out:?}"
        );
    }
}

/// The same wording whatever else the store holds: an identifier that resolves to nothing is
/// refused identically beside a LIVE resource of the same kind, and reading the live one back
/// still succeeds.
#[tokio::test]
async fn a_live_resource_beside_it_does_not_change_the_refusal() {
    let harness = Harness::new();

    let (status, out) = harness
        .call("CreateArchive", json!({ "ArchiveName": "invalid-id-wording" }))
        .await;
    assert_eq!(status, 200, "CreateArchive must answer 200, got {status} {out:?}");
    let archive_id = out["ArchiveId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateArchive returned no ArchiveId: {out:?}"))
        .to_string();

    let (status, out) = harness
        .call(
            "StartArchiveSearch",
            json!({
                "ArchiveId": archive_id,
                // Any pair inside `Timestamp`'s range does; these are epoch seconds a day
                // apart.
                "FromTimestamp": 1_761_000_000.0,
                "ToTimestamp": 1_761_086_400.0,
                "MaxResults": 10,
            }),
        )
        .await;
    assert_eq!(status, 200, "StartArchiveSearch must answer 200, got {status} {out:?}");
    let live_search_id = out["SearchId"]
        .as_str()
        .unwrap_or_else(|| panic!("StartArchiveSearch returned no SearchId: {out:?}"))
        .to_string();

    // The live job reads back, so the refusal below is about the id and not about the store.
    let (status, out) = harness
        .call("GetArchiveSearch", json!({ "SearchId": live_search_id.clone() }))
        .await;
    assert_eq!(
        status, 200,
        "case \"the live job\": GetArchiveSearch must report the job it minted, got {status} \
         {out:?}"
    );

    // A value derived from the live id by changing its last character names no job.
    let mut unresolvable: String = live_search_id.clone();
    let last = unresolvable.pop().unwrap_or('0');
    unresolvable.push(if last == 'z' { 'y' } else { 'z' });
    assert_ne!(
        unresolvable, live_search_id,
        "the test's unresolvable id must differ from the live one"
    );

    let (status, out) = harness
        .call("GetArchiveSearch", json!({ "SearchId": unresolvable.clone() }))
        .await;
    assert_eq!(
        status, 400,
        "case \"a near-miss of a live id\": must be refused with 400, got {status} {out:?}"
    );
    assert_eq!(
        error_code(&out),
        "ValidationException",
        "case \"a near-miss of a live id\": unexpected error code, got {out:?}"
    );
    assert_eq!(
        error_message(&out),
        Some("Invalid SearchId"),
        "case \"a near-miss of a live id\": unexpected message, got {out:?}"
    );
}
