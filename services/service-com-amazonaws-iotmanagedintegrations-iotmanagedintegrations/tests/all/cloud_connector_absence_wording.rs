// SPDX-License-Identifier: CC0-1.0

//! The SENTENCE a cloud connector's absence is stated with, over the wire, from every
//! operation that resolves one.
//!
//! RULE: the `Message` of a modelled error is part of the error contract, and this
//! resource's absence is worded by the RESOURCE rather than by the path that resolved it —
//! so the read (`GetCloudConnector`) and the mutation (`UpdateCloudConnector`) of an
//! identifier the signing region holds no cloud connector under answer the same 404 with
//! byte-for-byte the same sentence, which interpolates nothing from the request (neither
//! the identifier in the URI label nor any member of the body), while the identifier the
//! request carried is reported on `ResourceId` and the resource's kind on `ResourceType`.
//!
//! The sentence is written out literally here, not imported from the crate, so this suite
//! pins the text a client reads rather than whatever the implementation's formatter
//! currently renders.
//!
//! The recorded units this was derived from are ONE row of the tables below: the
//! `GetCloudConnector` and the `UpdateCloudConnector` steps of the recorded cloud-connector
//! error paths, which carry the identical `Message` for the same unknown identifier. The
//! rows differ along what the rule generalises over: the operation that resolves the
//! connector, the form and length of the identifier inside `CloudConnectorId`'s modelled
//! `@length`/`@pattern`, which members the update's body names, and the CAUSE of the
//! absence (never created, created and then deleted, or held only by another region).
//!
//! `DeleteCloudConnector` is deliberately not a row: what its answer to an identifier the
//! account holds nothing under should be at all (a refusal or an idempotent success) is a
//! separate open question about that operation's STATUS, not about this resource's wording.
//!
//! Every case drives the real restJson1 request path (`MultiServiceRouter::dispatch`), so
//! the statuses, headers and bodies asserted here are the ones a client sees.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::{
    ComAmazonawsIotmanagedintegrationsIotmanagedintegrationsState as ServiceState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region the suite's requests are signed for unless a case names another.
const REGION: &str = "us-east-1";

/// Another region's endpoint, for the case that addresses one region's connector through
/// another's.
const OTHER_REGION: &str = "eu-west-1";

/// The collection route `CreateCloudConnector` is bound to, and the prefix of the instance
/// route the read and the update are bound to
/// (`smithy_query(query="operation",operation="UpdateCloudConnector")` -> PUT
/// `/cloud-connectors/{Identifier}`).
const COLLECTION: &str = "/cloud-connectors";

/// The `@length` maximum of `CloudConnectorId`, in characters (same call -> `Identifier`
/// len 1..64), which the longest absent-identifier row is built at.
const CLOUD_CONNECTOR_ID_MAX_LENGTH: usize = 64;

/// The wire error code the model's `@httpError` 404 shape is rendered under.
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The `ResourceType` token this resource's 404 reports.
const RESOURCE_TYPE: &str = "CloudConnector";

/// The one sentence an absent cloud connector is stated with, whichever operation resolved
/// it: fixed text, with no value of the request inside it.
const ABSENCE_REASON: &str = "Cloud connector not found.";

/// One HTTP answer, reduced to what the assertions read.
struct Reply {
    status: u16,
    /// The restJson1 error code, from `x-amzn-errortype`.
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    fn string(&self, name: &str) -> Option<&str> {
        self.body
            .get(name)
            .filter(|value| !value.is_null())
            .and_then(Value::as_str)
    }
}

/// A router holding one `iotmanagedintegrations` service over the engine the caller keeps a
/// handle to: the same assembly `core/router` performs, with one service mounted.
fn emulator_over(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    // The signature itself is not under test here, so requests are unauthenticated.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Send one request, signed for `region`, and reduce the answer. The `Authorization`
/// credential scope is what routes the request to this service and names its region.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Reply {
    let payload = body.map_or_else(bytes::Bytes::new, |value| {
        bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize the request body"))
    });
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://iotmanagedintegrations.{region}.amazonaws.com{path}"
        ))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTESTTEST/20260101/{region}/\
                 iotmanagedintegrations/aws4_request, SignedHeaders=host, Signature=0"
            ),
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Reply {
        status,
        error_type,
        body,
    }
}

/// [`call_in`] in [`REGION`].
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Reply {
    call_in(router, REGION, method, path, body).await
}

/// One way of resolving a cloud connector: the request a row sends for an identifier.
struct Path {
    /// What an assertion calls this row.
    label: &'static str,
    method: &'static str,
    /// The body the row sends, if any. The sentence must not depend on it.
    body: Option<Value>,
}

/// The paths that resolve a cloud connector and can find none. The update rows differ along
/// which of its two optional members the body names (`Name` alone, `Description` alone,
/// both, neither), because the sentence interpolates nothing from the body either.
fn paths() -> Vec<Path> {
    vec![
        Path {
            label: "GetCloudConnector",
            method: "GET",
            body: None,
        },
        Path {
            label: "UpdateCloudConnector name-only",
            method: "PUT",
            body: Some(json!({ "Name": "a name nothing takes" })),
        },
        Path {
            label: "UpdateCloudConnector description-only",
            method: "PUT",
            body: Some(json!({ "Description": "a description nothing takes" })),
        },
        Path {
            label: "UpdateCloudConnector both-members",
            method: "PUT",
            body: Some(json!({
                "Name": "a name nothing takes",
                "Description": "a description nothing takes",
            })),
        },
        Path {
            label: "UpdateCloudConnector neither-member",
            method: "PUT",
            body: Some(json!({})),
        },
    ]
}

/// Identifiers inside the modelled `CloudConnectorId` constraints (len 1..64,
/// `^[A-Za-z0-9-_]+$`) that no create stored: a plain one, the shortest admitted, one at
/// the `@length` maximum, one using both characters of punctuation the `@pattern` admits,
/// and an all-digit one.
fn absent_identifiers() -> Vec<(&'static str, String)> {
    vec![
        ("plain", "unheld-connector".to_string()),
        ("shortest", "x".to_string()),
        ("longest", "c".repeat(CLOUD_CONNECTOR_ID_MAX_LENGTH)),
        ("punctuated-and-mixed-case", "Connector_9-ID".to_string()),
        ("digits-only", "000073".to_string()),
    ]
}

/// Assert one answer is this resource's 404: the modelled status and error code, the one
/// sentence, and the two members the recorded body carries.
fn assert_absent(case: &str, reply: &Reply, identifier: &str) {
    assert_eq!(
        reply.status, 404,
        "case {case:?}: expected HTTP 404, got {} with body {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some(RESOURCE_NOT_FOUND),
        "case {case:?}: expected the modelled error code {RESOURCE_NOT_FOUND:?}, got {:?}",
        reply.error_type
    );
    assert_eq!(
        reply.string("Message"),
        Some(ABSENCE_REASON),
        "case {case:?}: expected the absent-connector sentence {ABSENCE_REASON:?}, got {:?}",
        reply.string("Message")
    );
    assert_eq!(
        reply.string("ResourceId"),
        Some(identifier),
        "case {case:?}: the 404 reports the identifier the request carried, got {:?}",
        reply.string("ResourceId")
    );
    assert_eq!(
        reply.string("ResourceType"),
        Some(RESOURCE_TYPE),
        "case {case:?}: expected ResourceType {RESOURCE_TYPE:?}, got {:?}",
        reply.string("ResourceType")
    );
    assert!(
        !reply.string("Message").unwrap_or_default().contains(identifier),
        "case {case:?}: the sentence interpolates no request value, got {:?}",
        reply.string("Message")
    );
}

/// RULE: every path that resolves a cloud connector and finds none states ONE sentence —
/// the read and the mutation alike, over the class of identifiers the model admits and
/// whichever members the mutation's body names.
#[tokio::test]
async fn the_read_and_the_update_state_one_sentence_for_an_absent_connector() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_over(Arc::clone(&engine));

    // Every sentence the rows produced, so the suite shows there is exactly ONE across
    // both halves of this resource's surface rather than one per path.
    let mut reasons: BTreeSet<String> = BTreeSet::new();

    for (form, identifier) in absent_identifiers() {
        for path in paths() {
            let case = format!("{} / {form}", path.label);
            let refused = call(
                &router,
                path.method,
                &format!("{COLLECTION}/{identifier}"),
                path.body.clone(),
            )
            .await;
            assert_absent(&case, &refused, &identifier);
            reasons.insert(refused.string("Message").unwrap_or_default().to_string());
            assert!(
                engine
                    .state::<ServiceState>()
                    .lock()
                    .cloud_connectors
                    .is_empty(),
                "case {case:?}: a refused call must store nothing"
            );
        }
    }

    assert_eq!(
        reasons,
        BTreeSet::from([ABSENCE_REASON.to_string()]),
        "the read and the mutation owe one sentence, got {reasons:?}"
    );
}

/// `CreateCloudConnector` in `region`, answering the `CloudConnectorId` it minted.
async fn connector(router: &MultiServiceRouter, region: &str, label: &str) -> String {
    let created = call_in(
        router,
        region,
        "POST",
        COLLECTION,
        Some(json!({
            "Name": "partner cloud connector",
            "Description": "Lamps and switches from the partner cloud",
            "EndpointType": "LAMBDA",
            "EndpointConfig": { "lambda": { "arn": "connector-handler" } },
        })),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case {label:?}: expected HTTP 201 from the create, got {} with body {:?}",
        created.status, created.body
    );
    created
        .string("Id")
        .unwrap_or_else(|| panic!("case {label:?}: the create must report an Id"))
        .to_string()
}

/// RULE: the sentence states the ABSENCE, so what made the connector absent does not change
/// it — a connector that was created and then deleted is stated exactly as one that never
/// existed, and so is one that lives only in another region, under identifiers the SERVICE
/// minted rather than ones the test chose. A connector this region does hold is not refused
/// at all: its update applies and the read reports it.
#[tokio::test]
async fn the_cause_of_the_absence_does_not_change_the_sentence() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_over(Arc::clone(&engine));

    // Created, then removed: the delete this emulator serves for a connector it holds.
    let deleted = connector(&router, REGION, "created-and-deleted").await;
    let removed = call(&router, "DELETE", &format!("{COLLECTION}/{deleted}"), None).await;
    assert_eq!(
        removed.status, 200,
        "case \"created-and-deleted\": expected HTTP 200 from the delete, got {} with \
         body {:?}",
        removed.status, removed.body
    );

    // Alive, but in another region: this region's endpoint holds none under that id.
    let elsewhere = connector(&router, OTHER_REGION, "another-region").await;

    for (cause, identifier) in [
        ("created-and-deleted", &deleted),
        ("another-region", &elsewhere),
    ] {
        for path in paths() {
            let case = format!("{} / {cause}", path.label);
            let refused = call(
                &router,
                path.method,
                &format!("{COLLECTION}/{identifier}"),
                path.body.clone(),
            )
            .await;
            assert_absent(&case, &refused, identifier);
        }
    }

    // The connector another region does hold is untouched by the refusals above, and its
    // own region's update applies and reads back.
    let applied = call_in(
        &router,
        OTHER_REGION,
        "PUT",
        &format!("{COLLECTION}/{elsewhere}"),
        Some(json!({ "Name": "renamed at home" })),
    )
    .await;
    assert_eq!(
        applied.status, 200,
        "case \"another-region\": its own region's update must apply, got {} with body {:?}",
        applied.status, applied.body
    );
    let read = call_in(
        &router,
        OTHER_REGION,
        "GET",
        &format!("{COLLECTION}/{elsewhere}"),
        None,
    )
    .await;
    assert_eq!(
        read.status, 200,
        "case \"another-region\": expected HTTP 200 from the read, got {} with body {:?}",
        read.status, read.body
    );
    assert_eq!(
        read.string("Name"),
        Some("renamed at home"),
        "case \"another-region\": expected the updated Name, got {:?}",
        read.string("Name")
    );
}
